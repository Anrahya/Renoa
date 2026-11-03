package live.renoa.app.agents

import java.security.MessageDigest
import kotlinx.coroutines.CompletableDeferred
import kotlinx.coroutines.launch
import kotlinx.coroutines.ExperimentalCoroutinesApi
import kotlinx.coroutines.async
import kotlinx.coroutines.flow.first
import kotlinx.serialization.json.Json
import kotlinx.serialization.json.JsonArray
import kotlinx.serialization.json.JsonNull
import kotlinx.serialization.json.JsonObject
import kotlinx.serialization.json.JsonPrimitive
import kotlinx.serialization.json.boolean
import kotlinx.serialization.json.buildJsonObject
import kotlinx.serialization.json.jsonObject
import kotlinx.serialization.json.jsonPrimitive
import kotlinx.serialization.json.long
import kotlinx.serialization.json.put
import kotlinx.coroutines.flow.flowOf
import kotlinx.coroutines.test.TestScope
import kotlinx.coroutines.test.UnconfinedTestDispatcher
import kotlinx.coroutines.test.advanceTimeBy
import kotlinx.coroutines.test.runCurrent
import kotlinx.coroutines.test.runTest
import live.renoa.app.badge.mainAgent
import live.renoa.rcp.DeviceKeyRequired
import live.renoa.rcp.ApplyLiveActivity
import live.renoa.rcp.CommandSubmission
import live.renoa.rcp.Disconnected
import live.renoa.rcp.HostAnswer
import live.renoa.rcp.HostMethod
import live.renoa.rcp.InMemoryRcpState
import live.renoa.rcp.LiveActivity
import live.renoa.rcp.PendingTextCommand
import live.renoa.rcp.RcpErrorCode
import live.renoa.rcp.RcpException
import live.renoa.rcp.RcpSurface
import live.renoa.rcp.StopOutcome
import live.renoa.rcp.TargetSummary
import live.renoa.rcp.TaskEvent
import live.renoa.rcp.TaskSummary
import java.time.Instant
import java.time.ZoneId
import kotlin.test.Test
import kotlin.test.assertEquals
import kotlin.test.assertFailsWith
import kotlin.test.assertFalse
import kotlin.test.assertNull
import kotlin.test.assertTrue
import kotlin.time.Duration.Companion.minutes
import kotlin.time.Duration.Companion.seconds
import kotlin.uuid.Uuid

/** What the coordinator says to a submission. */
private sealed interface Answer {
    data object Admit : Answer

    data class Refuse(val code: RcpErrorCode, val requestId: Long? = 1) : Answer

    /** The connection drops before the answer arrives. */
    data object Lose : Answer
}

/**
 * A coordinator behind [RcpSurface] with the client's contract: an outbox in [state], journals
 * replayed in full on every attach and reconnect, and live frames to the attached.
 */
private class FakeSurface(val state: InMemoryRcpState) : RcpSurface {
    val node = Uuid.random()
    var targets = listOf(TargetSummary(node, "nagisa", "Nagisa"))
    val journals = HashMap<Uuid, Journal>()
    val opened = mutableListOf<Uuid>()
    val admitted = mutableListOf<PendingTextCommand>()
    val stopped = mutableListOf<Uuid>()
    var connects = 0
    var refuseConnects = 0

    /** Targets whose open is refused, as for an agent whose node is offline. */
    val unopenable = mutableSetOf<String>()

    /** When set, every open waits for it, as a slow node does. */
    var openGate: CompletableDeferred<Unit>? = null

    /** The Host's agents by id, and the agent each operation made, under an id of its own as on the Host. */
    val hostAgents = LinkedHashMap<String, JsonObject>()
    val made = LinkedHashMap<String, String>()

    /** When set, opening this target drops the connection, once, as when it drops partway through the roster. */
    var dropOnOpen: String? = null

    /** When set, the next open fails with it, as when the connection drops mid-open. */
    var openFailure: Exception? = null
    val hostCalls = mutableListOf<Triple<HostMethod, String, String?>>()

    /** When set, the next Host call ends with it instead of reaching the Host. */
    var hostFailure: Exception? = null

    /** When set, the Host gives this answer to the next call. */
    var hostAnswer: HostAnswer? = null

    /**
     * The automations the Host's observation lists, kept apart from [hostCalls]: the app reads them on
     * every roster check. Null answers the observation 404, as before the Host had any.
     */
    var automations: MutableList<JsonObject>? = null
    var observations = 0

    /** The Host agent the observation names as main, or none, as for a Host with no agent. */
    var mainAgent: Uuid? = null

    /** When set, the next observation ends with it instead of reaching the Host. */
    var observationFailure: Exception? = null

    /** When set, the next observation reads the automations, then answers only once this completes. */
    var observationHeld: CompletableDeferred<Unit>? = null

    /** The owner's feed on the Host, newest first, and its line about the day. Its calls are kept apart from [hostCalls] in [feedCalls]. */
    val feed = mutableListOf<JsonObject>()
    var today = "All quiet."
    val feedCalls = mutableListOf<Pair<HostMethod, String>>()

    /** When set, the next feed call ends with it instead of reaching the Host. */
    var feedFailure: Exception? = null

    /** When set, a run asked for now is refused as already running. */
    var running = false

    /** How many items a page of the feed holds. */
    var feedPage = 50

    /** When set, every older page of the feed ends with it. */
    var olderPageFailure: Exception? = null

    /** When set, an older page names itself as the next, as a Host with a cursor that never moves back would. */
    var stuckCursor = false

    /** When set, the next submission waits for this before it is kept. */
    var submitHeld: CompletableDeferred<Unit>? = null

    /** When set, the next read of the feed's first page takes the feed as it stands, then answers only once this completes. */
    var feedHeld: CompletableDeferred<Unit>? = null

    /** When set, the next switch is applied at once, but its receipt arrives only once this completes. */
    var switchHeld: CompletableDeferred<Unit>? = null

    /** When set, the next attach drops the connection after applying this many events. */
    var dropAttachAfter: Int? = null

    /** The answers to the next submissions, in order; a submission past them is admitted. */
    val answers = ArrayDeque<Answer>()
    private val attached = LinkedHashMap<Uuid, Pair<ApplyLiveActivity?, suspend (TaskEvent) -> Unit>>()
    private var ended = CompletableDeferred<Throwable>()
    private var online = false

    fun journal(target: String) = journals.getOrPut(chatTaskId(node, target)) { Journal(chatTaskId(node, target)) }

    /** Records [event] and delivers it, as the coordinator does for an attached task. */
    suspend fun record(event: TaskEvent) {
        val apply = attached[event.taskId]?.second ?: return
        apply(event)
        state.advanceCursor(event.taskId, event.sequence)
    }

    fun live(taskId: Uuid, commandId: Uuid, activity: LiveActivity) = attached[taskId]?.first?.invoke(commandId, activity)

    fun drop() {
        online = false
        ended.complete(Disconnected("dropped"))
    }

    /** Set when the coordinator no longer accepts this phone's sign-in. */
    var signInRefused: Exception? = null
    var closed = false

    override suspend fun connect() {
        connects++
        signInRefused?.let { throw it }
        if (refuseConnects-- > 0) throw Disconnected("refused")
        ended = CompletableDeferred()
        online = true
        for ((taskId, attachment) in attached) replay(taskId, attachment.second)
    }

    /** Applies what follows the stored cursor and advances it after each event, as the client does. */
    private suspend fun replay(taskId: Uuid, apply: suspend (TaskEvent) -> Unit, limit: Int = Int.MAX_VALUE) {
        val after = state.cursor(taskId) ?: 0
        journals.getValue(taskId).events.filter { it.sequence > after }.take(limit).forEach {
            apply(it)
            state.advanceCursor(taskId, it.sequence)
        }
    }

    override suspend fun disconnect() = drop()

    override suspend fun awaitDisconnect(): Throwable = ended.await()

    var probes = 0

    /** When set, a probe goes unanswered, so the connection ends as the client ends it. */
    var probeUnanswered = false

    override suspend fun probe() {
        check(online)
        probes++
        if (probeUnanswered) {
            drop()
            throw Disconnected("probe unanswered")
        }
    }

    override suspend fun listTasks(): List<TaskSummary> = error("unused")

    override suspend fun listTargets(): List<TargetSummary> = targets.also { check(online) }

    override suspend fun openTask(taskId: Uuid, nodeId: Uuid, target: String) {
        check(online)
        if (target in unopenable) throw RcpException(RcpErrorCode.NodeOffline, "offline", 1)
        if (target == dropOnOpen) {
            dropOnOpen = null
            drop()
            throw Disconnected("dropped mid-roster")
        }
        openGate?.await()
        openFailure?.let {
            openFailure = null
            throw it
        }
        opened += taskId
        journals.getOrPut(taskId) { Journal(taskId) }
    }

    override suspend fun deleteTask(taskId: Uuid) = error("unused")

    override suspend fun attach(taskId: Uuid, live: ApplyLiveActivity?, apply: suspend (TaskEvent) -> Unit) {
        check(online)
        checkNotNull(live) { "the app always asks for live text" }
        dropAttachAfter?.let { limit ->
            dropAttachAfter = null
            replay(taskId, apply, limit)
            drop()
            throw Disconnected("dropped during replay")
        }
        attached[taskId] = live to apply
        replay(taskId, apply)
    }

    private suspend fun deliver(command: PendingTextCommand) {
        when (val answer = answers.removeFirstOrNull() ?: Answer.Admit) {
            Answer.Admit -> {
                admitted += command
                state.removeCommand(command.commandId)
                record(journals.getValue(command.taskId).submitted(command.commandId, command.text))
            }
            is Answer.Refuse -> throw RcpException(answer.code, "refused", answer.requestId)
            Answer.Lose -> {
                drop()
                throw Disconnected("lost")
            }
        }
    }

    override suspend fun submitText(taskId: Uuid, text: String): CommandSubmission {
        check(online) { "RCP client is not connected" }
        submitHeld?.let {
            submitHeld = null
            it.await()
        }
        val command = PendingTextCommand(Uuid.random(), taskId, text)
        state.enqueueCommand(command)
        val accepted = CompletableDeferred<Unit>()
        runCatching { deliver(command) }.fold({ accepted.complete(Unit) }, { accepted.completeExceptionally(it) })
        return CommandSubmission(command.commandId, accepted)
    }

    override suspend fun stopCommand(taskId: Uuid, commandId: Uuid): StopOutcome {
        check(online)
        stopped += commandId
        return StopOutcome.Requested
    }

    /** The node advertises the agent [operation] made as `agent:<id>`, as it does once it next polls the Host. */
    fun advertise(operation: String) {
        val agentId = made.getValue(operation)
        targets += TargetSummary(node, "agent:$agentId", hostAgents.getValue(agentId).getValue("name").jsonPrimitive.content)
    }

    /** The owner's USER.md and its revision, which the Host derives from the content: 64 lowercase hex characters. */
    var profile = ""
    val profileEdits = mutableListOf<JsonObject>()

    private fun profileCall(method: HostMethod, body: String?): HostAnswer {
        val answer = { HostAnswer(200, buildJsonObject { put("content", profile); put("revision", revisionOf(profile)) }.toString()) }
        if (method == HostMethod.Get) return answer()
        val edit = Json.parseToJsonElement(checkNotNull(body)).jsonObject.also { profileEdits += it }
        val content = edit.getValue("content").jsonPrimitive.content
        if (content != profile) {
            if (edit.getValue("expected_revision").jsonPrimitive.content != revisionOf(profile)) {
                return HostAnswer(409, "{\"error\":\"revision_conflict\"}")
            }
            profile = content
        }
        return answer()
    }

    override suspend fun callHost(method: HostMethod, path: String, body: String?): HostAnswer {
        check(online) { "RCP client is not connected" }
        if (method == HostMethod.Get && path == "/v1/host") {
            observations++
            observationFailure?.let {
                observationFailure = null
                throw it
            }
            val listed = automations?.toList() ?: return HostAnswer(404, "{\"error\":\"not found\"}")
            observationHeld?.let {
                observationHeld = null
                it.await()
            }
            val main = mainAgent?.let { buildJsonObject { put("main_agent_id", it.toString()); put("revision", 1) } } ?: JsonNull
            return HostAnswer(
                200,
                buildJsonObject {
                    put("automations", JsonArray(listed))
                    put("agents", JsonArray(hostAgents.values.toList()))
                    put("main_agent", main)
                }.toString(),
            )
        }
        if (path == "/v1/host/profile") return profileCall(method, body)
        if (path == "/v1/host/feed" || path.startsWith("/v1/host/feed/") || path.endsWith("/run")) return feedCall(method, path)
        hostCalls += Triple(method, path, body)
        hostFailure?.let {
            hostFailure = null
            throw it
        }
        hostAnswer?.let {
            hostAnswer = null
            return it
        }
        if (method == HostMethod.Post && path.startsWith("/v1/host/automations/") && path.endsWith("/enabled")) {
            val receipt = enable(path, checkNotNull(body))
            switchHeld?.let {
                switchHeld = null
                it.await()
            }
            return receipt
        }
        if (method == HostMethod.Post && path == "/v1/host/agents") {
            val request = Json.parseToJsonElement(checkNotNull(body)).jsonObject
            val operation = request.getValue("operation_id").jsonPrimitive.content
            val id = made.getOrPut(operation) { Uuid.random().toString() }
            if (mainAgent == null) mainAgent = Uuid.parse(id)
            val record =
                hostAgents.getOrPut(id) {
                    buildJsonObject {
                        put("id", id)
                        put("name", request.getValue("name").jsonPrimitive.content)
                        put("created_at_ms", 1)
                        put(
                            "operational",
                            buildJsonObject {
                                put("instructions", "Help.")
                                request["purpose"]?.let { put("purpose", it.jsonPrimitive.content) }
                            },
                        )
                    }
                }
            val created =
                buildJsonObject {
                    put("operation_id", operation)
                    put("record", record)
                }
            return HostAnswer(201, created.toString())
        }
        val record = hostAgents[path.removePrefix("/v1/host/agents/")]
        return if (method == HostMethod.Get && record != null) HostAnswer(200, record.toString()) else HostAnswer(404, "{\"error\":\"not found\"}")
    }

    /** Lists the feed, dismisses an item, or runs a routine now, as the Host does. */
    private suspend fun feedCall(method: HostMethod, path: String): HostAnswer {
        feedCalls += method to path
        feedFailure?.let {
            feedFailure = null
            throw it
        }
        if (method == HostMethod.Get) {
            // As the Host numbers them, the newest item has the highest sequence, and a page holds those before its cursor.
            val before = path.removePrefix("/v1/host/feed").removePrefix("/before/").toIntOrNull()
            if (before != null) olderPageFailure?.let { throw it }
            val from = before?.let { feed.size - it + 1 } ?: 0
            val items = feed.drop(from).take(feedPage)
            val last = feed.size - (from + items.size - 1)
            val answer =
                buildJsonObject {
                    put("today", today)
                    put("items", JsonArray(items))
                    if (from + feedPage < feed.size) put("next_before", if (stuckCursor && before != null) before else last)
                }.toString()
            if (before == null) {
                feedHeld?.let {
                    feedHeld = null
                    it.await()
                }
            }
            return HostAnswer(200, answer)
        }
        if (path.endsWith("/run")) return if (running) HostAnswer(409, "{\"error\":\"run_in_progress\"}") else HostAnswer(200, "{}")
        val id = path.removePrefix("/v1/host/feed/items/").removeSuffix("/dismissal")
        val at = feed.indexOfFirst { it.getValue("id").jsonPrimitive.content == id }
        if (at < 0) return HostAnswer(404, "{\"error\":\"not_found\"}")
        feed[at] = JsonObject(feed[at] + ("done" to JsonPrimitive(true)))
        return HostAnswer(200, feed[at].toString())
    }

    /** Pauses or resumes an automation from the revision the request expects, as the Host does. */
    private fun enable(path: String, body: String): HostAnswer {
        val id = path.removePrefix("/v1/host/automations/").removeSuffix("/enabled")
        val listed = automations.orEmpty().toMutableList()
        val at = listed.indexOfFirst { it.getValue("id").jsonPrimitive.content == id }
        if (at < 0) return HostAnswer(404, "{\"error\":\"not_found\"}")
        val request = Json.parseToJsonElement(body).jsonObject
        val revision = listed[at].getValue("revision").jsonPrimitive.long
        if (request.getValue("expected_revision").jsonPrimitive.long != revision) return HostAnswer(409, "{\"error\":\"revision_conflict\"}")
        val enabled = request.getValue("enabled").jsonPrimitive.boolean
        val changed = JsonObject(listed[at] + mapOf("enabled" to JsonPrimitive(enabled), "revision" to JsonPrimitive(revision + 1)))
        automations?.set(at, changed)
        val receipt =
            buildJsonObject {
                put("operation_id", request.getValue("operation_id"))
                put("id", id)
                put("revision", revision + 1)
                put("enabled", enabled)
                put("next_due_ms", changed.getValue("next_due_ms"))
            }
        return HostAnswer(200, receipt.toString())
    }

    override suspend fun resendPending(command: PendingTextCommand) {
        check(online)
        if (state.pendingCommands().any { it.commandId == command.commandId }) deliver(command)
    }

    override suspend fun setPushToken(token: String) = error("unused")

    override suspend fun clearPushToken() = error("unused")

    override fun close() {
        closed = true
        drop()
    }
}

@OptIn(ExperimentalCoroutinesApi::class)
class RcpAgentRepositoryTest {
    private val state = InMemoryRcpState()
    private val surface = FakeSurface(state)
    private val nagisa get() = "${surface.node}/nagisa"
    private val chat get() = surface.journal("nagisa")

    private val homeFile = java.nio.file.Files.createTempDirectory("home").toFile().resolve("home")

    /** What this phone keeps of Home for [owner], written as soon as it is asked to. */
    private fun TestScope.kept(owner: String = "renoa.test me") = SavedHome(homeFile, owner, io = UnconfinedTestDispatcher(testScheduler))

    private fun TestScope.started(home: SavedHome? = null) =
        RcpAgentRepository(
            surface, state, backgroundScope, retryAfter = { 5.seconds }, rosterEvery = 1.minutes, here = { KOLKATA }, clock = testScheduler.timeSource, home = home,
        ).also {
            it.start()
            runCurrent()
        }

    private suspend fun RcpAgentRepository.chat(id: String) = conversation(id).first()

    @Test
    fun everyAgentIsFollowedInItsOneChatWithLiveText() = runTest {
        surface.targets += TargetSummary(surface.node, "kaede", "Kaede")
        val repo = started()
        assertEquals(listOf("Nagisa", "Kaede"), repo.agents.value.map { it.name })
        assertEquals(listOf(chatTaskId(surface.node, "nagisa"), chatTaskId(surface.node, "kaede")), surface.opened)
        assertTrue(repo.agents.value.all { it.state == AgentState.Resting })
    }

    @Test
    fun theJournalAndLiveTextDriveTheChatAndTheRoster() = runTest {
        val command = Uuid.random()
        chat.submitted(command, "hello")
        val repo = started()
        assertEquals(AgentState.Working, repo.agents.value.single().state, "a replayed command that has not ended is running")

        surface.live(chat.taskId, command, LiveActivity.TextDelta(0, "Hi"))
        assertEquals("Hi", repo.chat(nagisa).streaming?.text)
        surface.record(chat.said(command, "Hi there"))
        surface.record(chat.ended(command))
        assertEquals(listOf("hello", "Hi there"), repo.chat(nagisa).messages.map { it.text })
        assertNull(repo.chat(nagisa).streaming)
        val summary = repo.agents.value.single()
        assertEquals(AgentState.Resting, summary.state)
        assertTrue(summary.glance.unread)
        assertEquals("Hi there", summary.glance.line)

        repo.markSeen(nagisa)
        assertFalse(repo.agents.value.single().glance.unread)
    }

    @Test
    fun aSentMessageIsShownOnceAndAdmittedOnce() = runTest {
        val repo = started()
        repo.send(nagisa, "  hello  ")
        assertEquals(listOf("hello"), surface.admitted.map { it.text })
        assertEquals(listOf(surface.admitted.single().commandId.toString()), repo.chat(nagisa).messages.map { it.id })
        assertEquals(AgentState.Working, repo.agents.value.single().state)
        assertTrue(state.pendingCommands().isEmpty())
    }

    @Test
    fun aRefusedMessageIsTakenBackAndTheScreenToldWhy() = runTest {
        val repo = started()
        surface.answers += Answer.Refuse(RcpErrorCode.NotFound)
        val refused = assertFailsWith<RepositoryException> { repo.send(nagisa, "hello") }
        assertEquals(Problem.NotFound, refused.problem)
        assertTrue(repo.chat(nagisa).messages.isEmpty())
        assertTrue(state.pendingCommands().isEmpty(), "a refused message never goes again")
    }

    @Test
    fun aMessageTheNodeCouldNotTakeYetIsKeptAndGoesOnTheNextCheck() = runTest {
        val repo = started()
        surface.answers += listOf(Answer.Refuse(RcpErrorCode.NodeOffline), Answer.Refuse(RcpErrorCode.NotFound, requestId = null))
        repo.send(nagisa, "hello")
        assertEquals(listOf("hello"), repo.chat(nagisa).messages.map { it.text }, "shown while it waits")
        advanceTimeBy(1.minutes)
        runCurrent()
        assertEquals(1, state.pendingCommands().size, "an error for no request ends the connection; it refuses nothing")
        advanceTimeBy(1.minutes + 5.seconds)
        runCurrent()
        assertTrue(state.pendingCommands().isEmpty())
        assertEquals(listOf("hello"), surface.admitted.map { it.text })
        assertEquals(listOf("hello"), repo.chat(nagisa).messages.map { it.text })
    }

    @Test
    fun anAttachCutShortKeepsWhatItAppliedAndTheRestFollows() = runTest {
        val command = Uuid.random()
        chat.submitted(command, "hello")
        chat.said(command, "hi")
        surface.dropAttachAfter = 1
        val repo = started()
        assertTrue(repo.agents.value.isEmpty(), "not listed until attached")
        advanceTimeBy(5.seconds)
        runCurrent()
        assertEquals(listOf("hello", "hi"), repo.chat(nagisa).messages.map { it.text })
        assertEquals(AgentState.Working, repo.agents.value.single().state)
    }

    @Test
    fun anAgentThatCannotBeOpenedHoldsNoOneElseUpAndIsTriedAgain() = runTest {
        surface.targets = listOf(TargetSummary(surface.node, "kaede", "Kaede")) + surface.targets
        surface.unopenable += "kaede"
        val repo = started()
        assertEquals(listOf("Nagisa"), repo.agents.value.map { it.name })
        assertEquals(1, surface.connects, "the connection stays up")
        surface.unopenable.clear()
        advanceTimeBy(1.minutes)
        runCurrent()
        assertEquals(setOf("Nagisa", "Kaede"), repo.agents.value.map { it.name }.toSet())
    }

    @Test
    fun aMessageWhoseAnswerWasLostIsSentAgainWithItsIdentityAfterTheReconnect() = runTest {
        val repo = started()
        surface.answers += Answer.Lose
        repo.send(nagisa, "hello")
        val lost = state.pendingCommands().single()
        assertEquals(listOf(lost.commandId.toString() to "hello"), repo.chat(nagisa).messages.map { it.id to it.text }, "still shown while it waits")

        advanceTimeBy(5.seconds)
        runCurrent()
        assertEquals(listOf(lost.commandId), surface.admitted.map { it.commandId })
        assertEquals(listOf("hello"), repo.chat(nagisa).messages.map { it.text }, "and shown once")
    }

    @Test
    fun aMessageLeftInTheOutboxWhenTheAppClosedShowsOnceAndGoesAfterTheLaunch() = runTest {
        val left = PendingTextCommand(Uuid.random(), chat.taskId, "hello")
        state.enqueueCommand(left)
        surface.answers += Answer.Refuse(RcpErrorCode.NodeOffline)
        val repo = started()
        assertEquals(listOf(left.commandId.toString() to "hello"), repo.chat(nagisa).messages.map { it.id to it.text }, "shown while it waits")

        advanceTimeBy(1.minutes)
        runCurrent()
        assertEquals(listOf(left.commandId), surface.admitted.map { it.commandId }, "under the identity it was first sent with")
        assertEquals(listOf("hello"), repo.chat(nagisa).messages.map { it.text }, "and shown once")
    }

    @Test
    fun aMessageRefusedOnTheRetryIsDroppedAndTheRestStillGo() = runTest {
        val repo = started()
        surface.answers += listOf(Answer.Lose, Answer.Refuse(RcpErrorCode.Conflict))
        repo.send(nagisa, "first")
        state.enqueueCommand(PendingTextCommand(Uuid.random(), chat.taskId, "second"))
        advanceTimeBy(5.seconds)
        runCurrent()
        // The retry's refusal was for "first", so only "first" is dropped.
        assertTrue(state.pendingCommands().isEmpty())
        assertEquals(listOf("second"), surface.admitted.map { it.text })
        assertEquals(listOf("second"), repo.chat(nagisa).messages.map { it.text })
    }

    private val offline = Answer.Refuse(RcpErrorCode.NodeOffline)

    /** Sends "hello" to Nagisa, whose node cannot take it, and lets a minute of answers give it up. */
    private suspend fun TestScope.givenUp(repo: RcpAgentRepository): PendingTextCommand {
        surface.answers += listOf(offline, offline)
        repo.send(nagisa, "hello")
        val hello = state.pendingCommands().single()
        advanceTimeBy(1.minutes)
        runCurrent()
        assertEquals(listOf(true), repo.chat(nagisa).messages.map { it.notSent })
        return hello
    }

    @Test
    fun aMessageRenoaCannotTakeForAMinuteIsGivenUpAndAnotherChatDoesNotWaitBehindIt() = runTest {
        surface.targets += TargetSummary(surface.node, "kaede", "Kaede")
        val repo = started()
        advanceTimeBy(1.seconds)
        // In turn: the send, Nagisa's message at the next check, Kaede's at the same check, Nagisa's at the one after.
        surface.answers += listOf(offline, offline, Answer.Admit, offline)
        repo.send(nagisa, "hello")
        val hello = state.pendingCommands().single()
        state.enqueueCommand(PendingTextCommand(Uuid.random(), surface.journal("kaede").taskId, "hi"))

        advanceTimeBy(59.seconds)
        runCurrent()
        assertEquals(listOf("hi"), surface.admitted.map { it.text }, "Kaede's message did not wait behind Nagisa's")
        assertEquals(listOf(hello), state.pendingCommands(), "59 seconds of answers is not enough to give up")
        assertEquals(listOf(false), repo.chat(nagisa).messages.map { it.notSent })

        advanceTimeBy(1.minutes)
        runCurrent()
        assertEquals(listOf(ChatMessage(hello.commandId.toString(), Speaker.Owner, "hello", notSent = true)), repo.chat(nagisa).messages)
        assertEquals(listOf(hello), state.pendingCommands(), "kept, so a relaunch tries it again")

        advanceTimeBy(1.minutes)
        runCurrent()
        assertEquals(listOf("hi"), surface.admitted.map { it.text }, "given up, so not sent again")
    }

    @Test
    fun aMessageThatDidNotSendGoesAgainUnderItsOwnIdentityWhenRetried() = runTest {
        val repo = started()
        val hello = givenUp(repo)
        surface.answers += offline

        repo.retry(nagisa, hello.commandId.toString())
        assertEquals(listOf(hello), state.pendingCommands(), "the node still cannot take it, so it waits again")
        assertEquals(listOf(false), repo.chat(nagisa).messages.map { it.notSent }, "shown as sending, not as given up")

        advanceTimeBy(1.minutes)
        runCurrent()
        assertEquals(listOf(hello), surface.admitted, "under the identity it was first sent with")
        assertEquals(listOf(hello.commandId.toString() to false), repo.chat(nagisa).messages.map { it.id to it.notSent }, "and shown once")
        assertTrue(state.pendingCommands().isEmpty())
    }

    @Test
    fun aRetryRenoaRefusesSaysWhyAndTakesTheMessageAway() = runTest {
        val repo = started()
        val hello = givenUp(repo)
        surface.answers += Answer.Refuse(RcpErrorCode.NotFound)

        val refused = assertFailsWith<RepositoryException> { repo.retry(nagisa, hello.commandId.toString()) }

        assertEquals(Problem.NotFound, refused.problem)
        assertTrue(repo.chat(nagisa).messages.isEmpty())
        assertTrue(state.pendingCommands().isEmpty())
    }

    @Test
    fun aMessageThatDidNotSendCanBeDeletedAndIsNeverSent() = runTest {
        val repo = started()
        val hello = givenUp(repo)

        repo.discard(nagisa, hello.commandId.toString())

        assertTrue(repo.chat(nagisa).messages.isEmpty())
        assertTrue(state.pendingCommands().isEmpty())
        advanceTimeBy(1.minutes)
        runCurrent()
        assertTrue(surface.admitted.isEmpty())
    }

    @Test
    fun timeThePhoneCannotReachRenoaDoesNotCountTowardGivingUp() = runTest {
        val repo = started()
        surface.answers += listOf(offline, offline, offline)
        repo.send(nagisa, "hello")
        surface.drop()
        // Five minutes of reconnects refused, one every five seconds.
        surface.refuseConnects = 60
        advanceTimeBy(5.minutes + 5.seconds)
        runCurrent()
        assertEquals(listOf("hello"), state.pendingCommands().map { it.text }, "answered again once back, which starts the wait over")
        assertEquals(listOf(false), repo.chat(nagisa).messages.map { it.notSent })

        advanceTimeBy(1.minutes)
        runCurrent()
        assertEquals(listOf(true), repo.chat(nagisa).messages.map { it.notSent }, "a minute of answers on one connection")
    }

    @Test
    fun aMessageIsGivenUpOnlyWhereTheOwnerCanSeeItsChat() = runTest {
        val kaedeChat = surface.journal("kaede").taskId
        val hi = PendingTextCommand(Uuid.random(), kaedeChat, "hi")
        state.enqueueCommand(hi)
        repeat(6) { surface.answers += offline }
        val repo = started()
        advanceTimeBy(3.minutes)
        runCurrent()
        assertEquals(listOf(hi), state.pendingCommands(), "Kaede is not advertised, so there is no chat to say it did not go")

        surface.targets += TargetSummary(surface.node, "kaede", "Kaede")
        advanceTimeBy(2.minutes)
        runCurrent()
        assertEquals(listOf(hi.commandId.toString() to true), repo.chat("${surface.node}/kaede").messages.map { it.id to it.notSent })
    }

    @Test
    fun onlyTheHandshakeRefusingTheSignInTwiceRunningSignsThePhoneOut() = runTest {
        val repo = started()
        surface.signInRefused = RcpException(RcpErrorCode.AuthenticationFailed, "glitch", null)
        surface.drop()
        advanceTimeBy(5.seconds)
        runCurrent()
        surface.signInRefused = null
        advanceTimeBy(5.seconds)
        runCurrent()
        assertEquals(Link.Connected, repo.link.value, "a single refusal, then a connection")
        surface.signInRefused = RcpException(RcpErrorCode.AuthenticationFailed, "another glitch", null)
        surface.drop()
        advanceTimeBy(5.seconds)
        runCurrent()
        assertEquals(Link.Lost, repo.link.value, "a connection in between starts the count again")
        surface.signInRefused = null
        advanceTimeBy(5.seconds)
        runCurrent()

        for (other in listOf(RcpException(RcpErrorCode.Internal, "store busy", null), RcpException(RcpErrorCode.AuthenticationFailed, "a reply, not the handshake", 7))) {
            surface.signInRefused = other
            surface.drop()
            advanceTimeBy(30.seconds)
            runCurrent()
            assertEquals(Link.Lost, repo.link.value, "$other")
            surface.signInRefused = null
            advanceTimeBy(5.seconds)
            runCurrent()
            assertEquals(Link.Connected, repo.link.value, "$other keeps trying")
        }
    }

    @Test
    fun changesTheHostCannotMakeFromTheAppSaySoAndAreNotSent() = runTest {
        val repo = started()
        val calls = surface.hostCalls.size
        for (refused in listOf(
            assertFailsWith<RepositoryException> { repo.edit(nagisa, EditProfile("Nagisa", "Nagi", "", "")) },
            assertFailsWith<RepositoryException> { repo.setAbility(nagisa, "calendar", true) },
        )) {
            assertEquals(Problem.NotYet, refused.problem)
        }
        assertEquals("That can't be changed from the app yet.", Problem.NotYet.ownerWords)
        assertEquals(calls, surface.hostCalls.size)
    }

    @Test
    fun theOwnersProfileAndWhetherTheyHaveAnAgentComeFromTheHost() = runTest {
        surface.automations = mutableListOf()
        val repo = started()
        assertFalse(repo.hasAgents())
        surface.hostAgents["a"] = buildJsonObject { put("id", "a"); put("name", "Renoa") }
        assertTrue(repo.hasAgents())

        val empty = repo.ownerProfile()
        assertEquals(OwnerProfile("", revisionOf("")), empty)
        val saved = repo.saveOwnerProfile(empty.revision, "Name: Yash\n")
        assertEquals(OwnerProfile("Name: Yash\n", revisionOf("Name: Yash\n")), saved)
        assertEquals(setOf("expected_revision", "content"), surface.profileEdits.single().keys)
        assertEquals(saved, repo.saveOwnerProfile(empty.revision, "Name: Yash\n"), "a repeat of a save that landed")
        assertEquals(Problem.Conflict, assertFailsWith<RepositoryException> { repo.saveOwnerProfile(empty.revision, "Name: Asha\n") }.problem)
    }

    /** Two Host agents listed in id order, as the Host lists them, with [mainSortsFirst] saying which the Host names main. */
    private fun TestScope.theHostsMainAgentIsMarkedMain(mainSortsFirst: Boolean) {
        val (low, high) = listOf(Uuid.random(), Uuid.random()).sortedBy { it.toString() }
        surface.targets = listOf(TargetSummary(surface.node, "agent:$low", "Low"), TargetSummary(surface.node, "agent:$high", "High"))
        surface.automations = mutableListOf()
        surface.mainAgent = if (mainSortsFirst) low else high
        val repo = started()
        assertEquals(listOf(mainSortsFirst, !mainSortsFirst), repo.agents.value.map { it.primary })
        assertEquals("${surface.node}/agent:${surface.mainAgent}", mainAgent(repo.agents.value)?.id)
        assertEquals(1, surface.observations, "one read of the observation serves the routines and the main agent")
    }

    @Test
    fun theHostsMainAgentIsMainWhenItsIdSortsFirst() = runTest { theHostsMainAgentIsMarkedMain(mainSortsFirst = true) }

    @Test
    fun theHostsMainAgentIsMainWhenItsIdSortsLast() = runTest { theHostsMainAgentIsMarkedMain(mainSortsFirst = false) }

    @Test
    fun untilTheHostNamesAnAgentOnTheRosterTheFirstOnTheRosterStandsIn() = runTest {
        val other = Uuid.random()
        surface.targets += TargetSummary(surface.node, "agent:$other", "Other")
        val repo = started()
        assertEquals(listOf(false, false), repo.agents.value.map { it.primary }, "a Host that has not answered marks no one")
        assertEquals(nagisa, mainAgent(repo.agents.value)?.id)

        surface.automations = mutableListOf()
        surface.mainAgent = Uuid.random()
        advanceTimeBy(1.minutes)
        runCurrent()
        assertEquals(listOf(false, false), repo.agents.value.map { it.primary })
        assertEquals(nagisa, mainAgent(repo.agents.value)?.id, "the agent the Host named is not on this roster, so Home is not left empty")

        surface.mainAgent = other
        advanceTimeBy(1.minutes)
        runCurrent()
        assertEquals("${surface.node}/agent:$other", mainAgent(repo.agents.value)?.id, "a change on the Host shows on the next roster check")

        surface.automations = null
        advanceTimeBy(1.minutes)
        runCurrent()
        assertEquals("${surface.node}/agent:$other", mainAgent(repo.agents.value)?.id, "kept while the Host cannot be read")
    }

    @Test
    fun theFirstAgentMadeOnAnEmptyHostIsMainAtOnce() = runTest {
        surface.targets = emptyList()
        surface.automations = mutableListOf()
        val repo = started()
        assertTrue(repo.agents.value.isEmpty())
        val made = making(repo)
        runCurrent()
        surface.advertise(kcal)
        advanceTimeBy(1.seconds)
        runCurrent()
        assertEquals(kcalId, made.await().id)
        assertEquals(listOf(true), repo.agents.value.map { it.primary }, "marked before the next roster check")
        assertEquals(kcalId, mainAgent(repo.agents.value)?.id)
    }

    @Test
    fun theLastLaunchsMainAgentStaysMainUntilTheHostSays() = runTest {
        val kaede = "${surface.node}/kaede"
        surface.targets += TargetSummary(surface.node, "kaede", "Kaede")
        val last = listOf(AgentSummary(nagisa, "Nagisa", "", AgentState.Resting), AgentSummary(kaede, "Kaede", "", AgentState.Resting, primary = true))
        kept().keep(flowOf(HomeSnapshot(last, emptyList())))
        val repo = started(kept())
        assertEquals(listOf(false, true), repo.agents.value.map { it.primary }, "a Host that cannot be read leaves the kept mark")
        assertEquals(kaede, mainAgent(repo.agents.value)?.id)
        assertEquals(listOf(false, true), kept().load()?.agents?.map { it.primary }, "and the kept mark is what is saved")
    }

    @Test
    fun theLinkSaysWhetherRenoaCanBeReached() = runTest {
        surface.refuseConnects = 1
        val repo = RcpAgentRepository(surface, state, backgroundScope, retryAfter = { 5.seconds }, rosterEvery = 1.minutes, here = { KOLKATA })
        assertEquals(Link.Connecting, repo.link.value)
        repo.start()
        runCurrent()
        assertEquals(Link.Lost, repo.link.value, "a connection that failed")
        advanceTimeBy(5.seconds)
        runCurrent()
        assertEquals(Link.Connected, repo.link.value)
        surface.drop()
        runCurrent()
        assertEquals(Link.Lost, repo.link.value, "a connection that dropped")
        advanceTimeBy(5.seconds)
        runCurrent()
        assertEquals(Link.Connected, repo.link.value)
    }

    @Test
    fun everyConnectionRenoaAcceptsRunsTheHookAndItsFailureDropsNothing() = runTest {
        var connections = 0
        val repo =
            RcpAgentRepository(surface, state, backgroundScope, retryAfter = { 5.seconds }, here = { KOLKATA }, onConnected = {
                connections++
                throw IllegalStateException("registering failed")
            })
        surface.refuseConnects = 1
        repo.start()
        runCurrent()
        assertEquals(0, connections, "not for a connection Renoa refused")
        advanceTimeBy(5.seconds)
        runCurrent()
        assertEquals(1, connections)
        assertEquals(Link.Connected, repo.link.value, "its failure is its own")

        surface.drop()
        runCurrent()
        advanceTimeBy(5.seconds)
        runCurrent()
        assertEquals(2, connections, "again on the next connection")
    }

    @Test
    fun comingBackToTheAppOrANetworkComingUpRetriesWithoutWaitingOutTheBackoff() = runTest {
        surface.refuseConnects = 2
        val repo = RcpAgentRepository(surface, state, backgroundScope, retryAfter = { 1.minutes }, rosterEvery = 1.minutes, here = { KOLKATA })
        repo.start()
        runCurrent()
        assertEquals(Link.Lost, repo.link.value)

        repo.retryNow()
        runCurrent()
        assertEquals(2, surface.connects, "tried again at once")
        assertEquals(Link.Lost, repo.link.value, "refused again, so it waits again")

        repo.retryNow()
        runCurrent()
        assertEquals(Link.Connected, repo.link.value, "no time passed")

        repo.retryNow()
        surface.refuseConnects = 1
        surface.drop()
        runCurrent()
        assertEquals(3, surface.connects, "a nudge while connected does not cut the wait after the next drop")
    }

    @Test
    fun comingBackWhileTheLinkLooksUpChecksItAndADeadOneIsMadeAgain() = runTest {
        val repo = started()
        repo.retryNow()
        runCurrent()
        assertEquals(1, surface.probes)
        assertEquals(Link.Connected, repo.link.value, "it answered")

        surface.probeUnanswered = true
        repo.retryNow()
        runCurrent()
        assertEquals(Link.Lost, repo.link.value, "found dead in the probe's time, not the keepalive's")
        surface.probeUnanswered = false
        advanceTimeBy(5.seconds)
        runCurrent()
        assertEquals(Link.Connected, repo.link.value)
    }

    @Test
    fun aNudgeNeverHurriesTheRetryAfterARefusedSignIn() = runTest {
        val repo = started()
        surface.signInRefused = RcpException(RcpErrorCode.AuthenticationFailed, "glitch", null)
        surface.drop()
        advanceTimeBy(5.seconds)
        runCurrent()
        val tries = surface.connects

        repo.retryNow()
        runCurrent()
        assertEquals(tries, surface.connects, "a second refusal milliseconds later would sign the phone out")
        assertEquals(Link.Lost, repo.link.value)

        surface.signInRefused = null
        surface.refuseConnects = 1
        advanceTimeBy(5.seconds)
        runCurrent()
        assertEquals(tries + 1, surface.connects, "the nudge spent in the refusal's wait does not cut the next one")
    }

    @Test
    fun aNudgeDuringAnAttemptThatConnectsIsNotKeptForTheNextDrop() = runTest {
        val opening = CompletableDeferred<Unit>()
        surface.openGate = opening
        val repo = RcpAgentRepository(surface, state, backgroundScope, retryAfter = { 1.minutes }, rosterEvery = 1.minutes, here = { KOLKATA })
        repo.start()
        runCurrent()
        repo.retryNow()
        opening.complete(Unit)
        runCurrent()
        assertEquals(Link.Connected, repo.link.value)

        surface.refuseConnects = 1
        surface.drop()
        runCurrent()
        assertEquals(1, surface.connects, "the drop waits out its backoff")
    }

    @Test
    fun aRefusedSignInSignsThePhoneOutAndNothingRetriesIt() = runTest {
        for (refusal in listOf(RcpException(RcpErrorCode.AuthenticationFailed, "unknown device", null), DeviceKeyRequired("no key"))) {
            surface.signInRefused = null
            surface.closed = false
            val repo = started()
            assertEquals(Link.Connected, repo.link.value)
            surface.signInRefused = refusal
            surface.drop()
            advanceTimeBy(5.seconds)
            runCurrent()
            assertEquals(Link.Lost, repo.link.value, "refused once could be a glitch")
            advanceTimeBy(5.seconds)
            runCurrent()
            assertEquals(Link.SignedOut, repo.link.value, "$refusal")
            assertTrue(surface.closed, "the client is closed")
            val tries = surface.connects
            advanceTimeBy(5.minutes)
            runCurrent()
            assertEquals(tries, surface.connects, "no retry can fix a refused sign-in")
        }
    }

    @Test
    fun withoutAConnectionNothingIsKeptAndTheScreenSaysSo() = runTest {
        surface.refuseConnects = 1
        val repo = started()
        assertTrue(repo.agents.value.isEmpty())
        advanceTimeBy(5.seconds)
        runCurrent()
        surface.drop()
        assertEquals(Problem.Unavailable, assertFailsWith<RepositoryException> { repo.send(nagisa, "hello") }.problem)
        assertTrue(state.pendingCommands().isEmpty())
        assertEquals(Problem.Unavailable, assertFailsWith<RepositoryException> { repo.create(CreateAgent("op", "A", "B", emptySet())) }.problem)
    }

    @Test
    fun aReconnectReplaysWithoutDoublingAndAnAgentMadeElsewhereJoins() = runTest {
        val command = Uuid.random()
        chat.submitted(command, "hello")
        chat.said(command, "hi")
        val repo = started()
        surface.live(chat.taskId, command, LiveActivity.TextDelta(0, "And "))
        surface.drop()
        advanceTimeBy(5.seconds)
        runCurrent()
        assertEquals(2, surface.connects)
        surface.live(chat.taskId, command, LiveActivity.TextDelta(1, "then"))
        assertNull(repo.chat(nagisa).streaming, "a draft cut by the drop is not glued to what came after it")
        assertEquals(listOf("hello", "hi"), repo.chat(nagisa).messages.map { it.text })

        surface.targets += TargetSummary(surface.node, "kaede", "Kaede")
        advanceTimeBy(1.minutes)
        runCurrent()
        assertEquals(listOf("Nagisa", "Kaede"), repo.agents.value.map { it.name })
        assertEquals(1, surface.opened.count { it == chat.taskId }, "a followed chat is not opened again")
    }

    @Test
    fun stopEndsTheRunningCommandAndIsQuietWhenNothingRuns() = runTest {
        val command = Uuid.random()
        chat.submitted(command, "hello")
        val repo = started()
        repo.stop(nagisa)
        assertEquals(listOf(command), surface.stopped)

        surface.record(chat.ended(command))
        repo.stop(nagisa)
        assertEquals(listOf(command), surface.stopped)
    }

    @Test
    fun whatHomeLastShowedShowsUntilRenoaAnswersThenTheLiveRosterReplacesItAndIsKept() = runTest {
        val gone = "${surface.node}/gone"
        kept().keep(flowOf(HomeSnapshot(listOf(AgentSummary(gone, "Kept", "", AgentState.Working, Glance(line = "See you", doing = "Cooking", with = nagisa))), emptyList())))
        surface.refuseConnects = 1
        val repo = started(kept())
        assertTrue(repo.rosterKnown.value)
        assertEquals(listOf(AgentSummary(gone, "Kept", "", AgentState.Resting, Glance(line = "See you"))), repo.agents.value, "kept, and no one shown mid-task")
        assertEquals(Problem.Unavailable, assertFailsWith<RepositoryException> { repo.send(gone, "hello") }.problem, "a kept agent waits for Renoa")

        advanceTimeBy(5.seconds)
        runCurrent()
        assertEquals(listOf("Nagisa"), repo.agents.value.map { it.name })
        assertEquals(listOf("Nagisa"), kept().load()?.agents?.map { it.name }, "kept for the next launch")
        assertEquals(Problem.NotFound, assertFailsWith<RepositoryException> { repo.send(gone, "hello") }.problem, "once Renoa has answered, it is gone")
    }

    @Test
    fun aRosterCheckCutShortLeavesWhatHomeLastShowedInPlace() = runTest {
        kept().keep(flowOf(HomeSnapshot(listOf(AgentSummary(nagisa, "Nagisa", "", AgentState.Resting), AgentSummary("${surface.node}/kaede", "Kaede", "", AgentState.Resting)), emptyList())))
        surface.targets += TargetSummary(surface.node, "kaede", "Kaede")
        surface.dropOnOpen = "kaede"
        val repo = started(kept())
        assertEquals(listOf("Nagisa", "Kaede"), repo.agents.value.map { it.name }, "not the half of the roster that attached")
        advanceTimeBy(5.seconds)
        runCurrent()
        assertEquals(listOf("Nagisa", "Kaede"), repo.agents.value.map { it.name })
        assertEquals(Link.Connected, repo.link.value)
    }

    @Test
    fun anAgentWhoseChatCannotBeOpenedKeepsItsPlaceAndAnAgentRenoaNoLongerListsLeaves() = runTest {
        val kaede = "${surface.node}/kaede"
        val gone = "${surface.node}/gone"
        kept().keep(flowOf(HomeSnapshot(listOf(AgentSummary(kaede, "Kaede", "", AgentState.Resting), AgentSummary(nagisa, "Nagisa", "", AgentState.Resting), AgentSummary(gone, "Gone", "", AgentState.Resting)), emptyList())))
        surface.targets = listOf(TargetSummary(surface.node, "kaede", "Kaede"), TargetSummary(surface.node, "nagisa", "Nagisa"))
        surface.unopenable += "kaede"
        val repo = started(kept())
        assertEquals(listOf("Kaede", "Nagisa"), repo.agents.value.map { it.name }, "refused, Kaede keeps her place; Gone is not listed")
        assertEquals(listOf("Kaede", "Nagisa"), kept().load()?.agents?.map { it.name }, "and the kept roster does not shrink")
        assertEquals(Problem.Unavailable, assertFailsWith<RepositoryException> { repo.send(kaede, "hello") }.problem, "her chat waits for her node")
        val chat = backgroundScope.async { repo.chat(kaede) }
        runCurrent()
        assertFalse(chat.isCompleted)
        assertEquals(Problem.NotFound, assertFailsWith<RepositoryException> { repo.chat(gone) }.problem, "a chat Renoa no longer lists is gone, not waiting")

        surface.unopenable -= "kaede"
        advanceTimeBy(1.minutes)
        runCurrent()
        assertTrue(chat.isCompleted, "her chat opens once her node takes it")
        assertEquals(listOf("Kaede", "Nagisa"), repo.agents.value.map { it.name })
    }

    @Test
    fun withNothingKeptNoOneIsKnownUntilRenoaAnswers() = runTest {
        surface.refuseConnects = 1
        val repo = started(kept())
        assertFalse(repo.rosterKnown.value)
        assertTrue(repo.agents.value.isEmpty())
        advanceTimeBy(5.seconds)
        runCurrent()
        assertTrue(repo.rosterKnown.value)
        assertEquals(listOf("Nagisa"), repo.agents.value.map { it.name })
    }

    @Test
    fun anotherSignInsHomeIsNeverShown() = runTest {
        kept(owner = "renoa.test someone-else").keep(flowOf(HomeSnapshot(listOf(AgentSummary("theirs", "Theirs", "", AgentState.Resting)), emptyList())))
        surface.refuseConnects = 1
        val repo = started(kept())
        assertFalse(repo.rosterKnown.value)
        assertTrue(repo.agents.value.isEmpty())
    }

    @Test
    fun aChatReopenedBeforeTheFirstConnectWaitsForItsAgent() = runTest {
        surface.refuseConnects = 1
        val repo = started()
        val opened = backgroundScope.async { repo.chat(nagisa) }
        runCurrent()
        assertFalse(opened.isCompleted)
        advanceTimeBy(5.seconds)
        runCurrent()
        assertTrue(opened.isCompleted)
    }

    private val kcal = Uuid.random().toString()
    private val agent get() = surface.made.getValue(kcal)
    private val kcalId get() = "${surface.node}/agent:$agent"

    private fun TestScope.making(repo: RcpAgentRepository, purpose: String = "Count my calories.") =
        backgroundScope.async { repo.create(CreateAgent(kcal, "Kcal", purpose, emptySet())) }

    private fun sent() = Json.parseToJsonElement(checkNotNull(surface.hostCalls.last().third)).jsonObject

    @Test
    fun aNewAgentIsMadeOnTheHostAndFollowedOnceItsNodeOffersIt() = runTest {
        val repo = started()
        val made = making(repo, purpose = "  Count my calories.  ")
        runCurrent()
        assertEquals(HostMethod.Post to "/v1/host/agents", surface.hostCalls.single().let { it.first to it.second })
        assertEquals(
            mapOf("operation_id" to kcal, "preset_id" to "renoa.app.v3", "name" to "Kcal", "purpose" to "Count my calories."),
            sent().mapValues { it.value.jsonPrimitive.content },
        )
        advanceTimeBy(3.seconds)
        assertFalse(made.isCompleted, "it waits until the node offers the agent")

        surface.advertise(kcal)
        advanceTimeBy(1.seconds)
        runCurrent()
        assertEquals(AgentProfile(kcalId, "Kcal", "Count my calories.", emptyList()), made.await())
        assertEquals(listOf("Nagisa" to "", "Kcal" to "Count my calories."), repo.agents.value.map { it.name to it.purpose }, "with its purpose at once")
        assertEquals(1, surface.opened.count { it == chatTaskId(surface.node, "agent:$agent") })
    }

    @Test
    fun eachHostAgentCarriesItsPurposeReadOnceAndAgainWhenTheHostCouldNotAnswer() = runTest {
        val id = Uuid.random().toString()
        surface.hostAgents[id] =
            buildJsonObject {
                put("id", id)
                put("name", "Kcal")
                put("operational", buildJsonObject { put("purpose", "Count my calories.") })
            }
        surface.targets += TargetSummary(surface.node, "agent:$id", "Kcal")
        surface.hostFailure = IllegalStateException("dropped")
        val repo = started()
        assertEquals(listOf("Nagisa" to "", "Kcal" to ""), repo.agents.value.map { it.name to it.purpose }, "not read yet")

        advanceTimeBy(1.minutes)
        runCurrent()
        assertEquals(listOf("Nagisa" to "", "Kcal" to "Count my calories."), repo.agents.value.map { it.name to it.purpose })
        assertEquals(listOf("/v1/host/agents/$id", "/v1/host/agents/$id"), surface.hostCalls.map { it.second }, "only Host agents are asked about")

        advanceTimeBy(1.minutes)
        runCurrent()
        assertEquals(2, surface.hostCalls.size, "read once")
    }

    @Test
    fun anAgentTheHostNoLongerHasIsAskedAboutOnce() = runTest {
        val gone = Uuid.random()
        surface.targets += TargetSummary(surface.node, "agent:$gone", "Kcal")
        val repo = started()
        assertEquals(listOf("Nagisa" to "", "Kcal" to ""), repo.agents.value.map { it.name to it.purpose })
        advanceTimeBy(2.minutes)
        runCurrent()
        assertEquals(listOf("/v1/host/agents/$gone"), surface.hostCalls.map { it.second })
    }

    @Test
    fun aBlankPurposeIsLeftOutOfTheRequest() = runTest {
        val repo = started()
        val made = making(repo, purpose = " \n ")
        runCurrent()
        assertFalse("purpose" in sent())
        surface.advertise(kcal)
        advanceTimeBy(1.seconds)
        assertEquals("", made.await().purpose)
    }

    @Test
    fun anAgentItsNodeNeverOffersIsUnavailableAndARetryFindsTheSameOne() = runTest {
        val repo = started()
        val request = CreateAgent(kcal, "Kcal", "Count my calories.", emptySet())
        val waited = testScheduler.currentTime
        assertEquals(Problem.Unavailable, assertFailsWith<RepositoryException> { repo.create(request) }.problem)
        assertEquals(30_000, testScheduler.currentTime - waited, "it gives the node thirty seconds")

        surface.advertise(kcal)
        assertEquals(kcalId, making(repo).await().id)
        assertEquals(listOf(kcal, kcal), surface.hostCalls.map { Json.parseToJsonElement(it.third!!).jsonObject.getValue("operation_id").jsonPrimitive.content })
        assertEquals(1, surface.hostAgents.size)
    }

    @Test
    fun aNewAgentAndTheRosterCheckOpenItsChatOnce() = runTest {
        val repo = started()
        surface.openGate = CompletableDeferred()
        val made = making(repo)
        runCurrent()
        surface.advertise(kcal)
        advanceTimeBy(1.seconds)
        runCurrent()
        advanceTimeBy(1.minutes)
        runCurrent()
        surface.openGate!!.complete(Unit)
        runCurrent()
        assertEquals(kcalId, made.await().id)
        assertEquals(1, surface.opened.count { it == chatTaskId(surface.node, "agent:$agent") })
    }

    @Test
    fun abilitiesCannotBeSwitchedOnANewAgentYet() = runTest {
        val repo = started()
        assertTrue(repo.starterAbilities().isEmpty())
        val refused = assertFailsWith<RepositoryException> { repo.create(CreateAgent(kcal, "Kcal", "", setOf("calendar"))) }
        assertEquals(Problem.Invalid, refused.problem)
        assertTrue(surface.hostCalls.isEmpty(), "nothing reached the Host")
    }

    @Test
    fun aProfileIsReadFromTheHost() = runTest {
        val repo = started()
        making(repo)
        runCurrent()
        surface.advertise(kcal)
        advanceTimeBy(1.seconds)
        runCurrent()
        surface.hostCalls.clear()

        assertEquals(AgentProfile(kcalId, "Kcal", "Count my calories.", emptyList()), repo.profile(kcalId))
        assertEquals(HostMethod.Get to "/v1/host/agents/$agent", surface.hostCalls.single().let { it.first to it.second })
        assertEquals(Problem.NotFound, assertFailsWith<RepositoryException> { repo.profile("${surface.node}/agent:${Uuid.random()}") }.problem)
        surface.hostCalls.clear()
        for (notHost in listOf(nagisa, "${surface.node}/agent:nope", "${surface.node}/$agent", "agent:$agent")) {
            assertEquals(Problem.NotFound, assertFailsWith<RepositoryException> { repo.profile(notHost) }.problem, notHost)
        }
        assertTrue(surface.hostCalls.isEmpty(), "an id that names no Host agent is not asked about")
    }

    @Test
    fun theHostsRefusalsReachTheScreenAsProblems() = runTest {
        val repo = started()
        val stranger = "${surface.node}/agent:${Uuid.random()}"
        val answers =
            listOf(
                HostAnswer(409, "{}") to Problem.Conflict,
                HostAnswer(400, "{}") to Problem.Invalid,
                HostAnswer(422, "{}") to Problem.Invalid,
                HostAnswer(404, "{}") to Problem.NotFound,
                HostAnswer(503, "{}") to Problem.Unavailable,
                HostAnswer(200, "not json") to Problem.Unavailable,
                HostAnswer(200, "{\"id\":\"x\"}") to Problem.Unavailable,
            )
        for ((answer, problem) in answers) {
            surface.hostAnswer = answer
            assertEquals(problem, assertFailsWith<RepositoryException> { repo.profile(stranger) }.problem, answer.toString())
        }
        val failures =
            listOf(
                RcpException(RcpErrorCode.InvalidMessage, "bad path", 1) to Problem.Invalid,
                RcpException(RcpErrorCode.NodeOffline, "Host offline", 1) to Problem.Unavailable,
                Disconnected("lost") to Problem.Unavailable,
            )
        for ((failure, problem) in failures) {
            surface.hostFailure = failure
            assertEquals(problem, assertFailsWith<RepositoryException> { repo.profile(stranger) }.problem, failure.toString())
        }
    }

    @Test
    fun aNewAgentWhoseChatCannotBeOpenedIsUnavailableAndTheAppKeepsRunning() = runTest {
        val repo = started()
        val request = CreateAgent(kcal, "Kcal", "", emptySet())
        val made = backgroundScope.async { runCatching { repo.create(request) } }
        runCurrent()
        surface.advertise(kcal)
        surface.unopenable += "agent:$agent"
        advanceTimeBy(1.seconds)
        runCurrent()
        val refused = made.await().exceptionOrNull()
        assertTrue(refused is RepositoryException && refused.problem == Problem.Unavailable, "a refused open is not a success: $refused")

        surface.openFailure = Disconnected("dropped mid-open")
        surface.unopenable.clear()
        assertEquals(Problem.Unavailable, assertFailsWith<RepositoryException> { repo.create(request) }.problem, "a lost connection is a problem, not a crash")

        assertEquals(kcalId, repo.create(request).id, "the same request finds the same agent")
        assertEquals(1, surface.hostAgents.size)
    }

    @Test
    fun theHostRefusingACreateIsAProblemForTheScreen() = runTest {
        val repo = started()
        surface.hostAnswer = HostAnswer(409, "{\"error\":\"operation_conflict\"}")
        assertEquals(Problem.Conflict, assertFailsWith<RepositoryException> { repo.create(CreateAgent(kcal, "Kcal", "", emptySet())) }.problem)
        surface.hostFailure = RcpException(RcpErrorCode.NodeOffline, "Host offline", 1)
        assertEquals(Problem.Unavailable, assertFailsWith<RepositoryException> { repo.create(CreateAgent(kcal, "Kcal", "", emptySet())) }.problem)
        assertTrue(surface.hostAgents.isEmpty())
    }

    private fun automation(id: String, agent: Uuid, expression: String, enabled: Boolean = true, revision: Long = 1, lastRun: JsonObject? = null, plugin: String? = null) =
        buildJsonObject {
            put("id", id)
            put("plugin", plugin)
            put("agent_id", agent.toString())
            put("name", "Dinner plan")
            put("schedule", buildJsonObject { put("kind", "cron"); put("expression", expression); put("timezone", "Asia/Kolkata") })
            put("enabled", enabled)
            put("revision", revision)
            put("next_due_ms", NEXT)
            put("pending_runs", 0)
            put("last_run", lastRun ?: JsonNull)
        }

    @Test
    fun routinesAreReadFromTheHostWithTheirAgentAndInPlainWords() = runTest {
        val tifa = Uuid.random()
        surface.targets += TargetSummary(surface.node, "agent:$tifa", "Tifa")
        val ran = buildJsonObject { put("id", Uuid.random().toString()); put("status", "failed"); put("due_ms", NEXT - 86_400_000); put("finished_at_ms", NEXT - 86_000_000) }
        // The Host sends null for a finish time it did not keep.
        val skipped = buildJsonObject { put("id", Uuid.random().toString()); put("status", "skipped"); put("due_ms", NEXT - 3_600_000); put("finished_at_ms", JsonNull) }
        surface.automations = mutableListOf(automation("dinner", tifa, "30 19 * * *", lastRun = ran), automation("brief", Uuid.random(), "0 7 * * 1-5", enabled = false, lastRun = skipped))
        val repo = started()

        val (dinner, brief) = repo.routines.value
        assertEquals(Routine("dinner", "${surface.node}/agent:$tifa", "Dinner plan", "Every day at 7:30 PM", true, Instant.ofEpochMilli(NEXT), LastRun(Instant.ofEpochMilli(NEXT - 86_000_000), RunOutcome.Failed)), dinner)
        assertNull(brief.agentId, "an agent that is not on the roster is not guessed")
        assertFalse(brief.on)
        assertNull(brief.next, "a routine that is off runs at no time")
        assertEquals(LastRun(Instant.ofEpochMilli(NEXT - 3_600_000), RunOutcome.Skipped), brief.lastRun, "without a finish time, it is when the run was due")
    }

    @Test
    fun aHostPluginsOwnTriggersAreNotRoutines() = runTest {
        surface.automations =
            mutableListOf(
                automation("reminder", Uuid.random(), "0 9 * * *", plugin = "renoa.memory"),
                automation("dinner", Uuid.random(), "30 19 * * *"),
            )
        val repo = started()

        assertEquals(listOf("dinner"), repo.routines.value.map { it.id }, "a reminder is the Host's, not a routine to switch")
    }

    @Test
    fun routinesAreReadAgainOnEveryRosterCheckAndKeptWhileTheHostCannotBeRead() = runTest {
        val repo = started()
        assertTrue(repo.routines.value.isEmpty(), "a Host that lists none shows none, and the chats still open")
        assertEquals(1, repo.agents.value.size)

        val optimus = Uuid.random()
        surface.automations = mutableListOf(automation("a", optimus, "0 9 * * *"))
        advanceTimeBy(1.minutes)
        runCurrent()
        assertEquals(listOf("a"), repo.routines.value.map { it.id })
        assertNull(repo.routines.value.single().agentId)

        surface.targets += TargetSummary(surface.node, "agent:$optimus", "Optimus")
        advanceTimeBy(1.minutes)
        runCurrent()
        assertEquals("${surface.node}/agent:$optimus", repo.routines.value.single().agentId, "the agent's id once it is on the roster")

        surface.automations?.add(automation("b", Uuid.random(), "0 10 * * *"))
        surface.observationFailure = RcpException(RcpErrorCode.NodeOffline, "Host offline", 1)
        advanceTimeBy(1.minutes)
        runCurrent()
        assertEquals(listOf("a"), repo.routines.value.map { it.id }, "kept while the Host cannot be read")
        assertEquals(1, surface.connects, "a Host that cannot be read does not end the connection")

        advanceTimeBy(1.minutes)
        runCurrent()
        assertEquals(listOf("a", "b"), repo.routines.value.map { it.id })
    }

    @Test
    fun aRoutineIsSwitchedFromTheRevisionReadAndAChangeElsewhereIsAConflict() = runTest {
        surface.automations = mutableListOf(automation("a", Uuid.random(), "0 9 * * *", revision = 4))
        val repo = started()
        val operation = Uuid.random().toString()

        repo.setRoutine("a", on = false, operation)
        val (method, path, body) = surface.hostCalls.single()
        assertEquals(HostMethod.Post to "/v1/host/automations/a/enabled", method to path)
        val sent = Json.parseToJsonElement(checkNotNull(body)).jsonObject
        assertEquals(listOf(operation, "4", "false"), listOf("operation_id", "expected_revision", "enabled").map { sent.getValue(it).jsonPrimitive.content })
        assertFalse(repo.routines.value.single().on)
        assertNull(repo.routines.value.single().next)

        repo.setRoutine("a", on = false, operation)
        assertEquals(1, surface.hostCalls.size, "a routine already off is not asked again")

        surface.automations?.set(0, automation("a", Uuid.random(), "0 9 * * *", enabled = true, revision = 9))
        val conflict = assertFailsWith<RepositoryException> { repo.setRoutine("a", on = true, Uuid.random().toString()) }
        assertEquals(Problem.Conflict, conflict.problem)
        assertTrue(repo.routines.value.single().on, "the conflict read the routine again")

        assertEquals(Problem.NotFound, assertFailsWith<RepositoryException> { repo.setRoutine("gone", on = true, Uuid.random().toString()) }.problem)
    }

    @Test
    fun aSwitchsReceiptThatArrivesLateDoesNotUndoANewerRead() = runTest {
        surface.automations = mutableListOf(automation("a", Uuid.random(), "0 9 * * *", revision = 4), automation("b", Uuid.random(), "0 10 * * *", revision = 4))
        val repo = started()

        // Switched off, then changed back elsewhere before the receipt arrives: the newer read stands.
        val first = CompletableDeferred<Unit>()
        surface.switchHeld = first
        val switchingOff = async { repo.setRoutine("a", on = false, Uuid.random().toString()) }
        runCurrent()
        assertTrue(surface.switchHeld == null, "the Host has applied revision 5 and its receipt waits")
        surface.automations?.set(0, automation("a", Uuid.random(), "0 9 * * *", enabled = true, revision = 6))
        advanceTimeBy(1.minutes)
        runCurrent()
        first.complete(Unit)
        switchingOff.await()
        assertTrue(repo.routines.value.first { it.id == "a" }.on, "the receipt for revision 5 does not replace revision 6")

        // Switched off, then removed elsewhere before the receipt arrives: it stays gone.
        val second = CompletableDeferred<Unit>()
        surface.switchHeld = second
        val switchingB = async { repo.setRoutine("b", on = false, Uuid.random().toString()) }
        runCurrent()
        assertTrue(surface.switchHeld == null)
        surface.automations?.removeAt(1)
        advanceTimeBy(1.minutes)
        runCurrent()
        second.complete(Unit)
        switchingB.await()
        assertEquals(listOf("a"), repo.routines.value.map { it.id }, "a routine a newer read dropped is not brought back")
    }

    @Test
    fun anObservationThatAnswersAfterASwitchDoesNotUndoIt() = runTest {
        surface.automations = mutableListOf(automation("a", Uuid.random(), "0 9 * * *", revision = 4))
        val repo = started()
        val held = CompletableDeferred<Unit>()
        surface.observationHeld = held
        advanceTimeBy(1.minutes)
        runCurrent()
        assertTrue(surface.observationHeld == null, "the roster check has read revision 4 and waits")

        repo.setRoutine("a", on = false, Uuid.random().toString())
        held.complete(Unit)
        runCurrent()
        assertFalse(repo.routines.value.single().on, "the older answer does not turn it back on")

        repo.setRoutine("a", on = true, Uuid.random().toString())
        val sent = Json.parseToJsonElement(checkNotNull(surface.hostCalls.last().third)).jsonObject
        assertEquals("5", sent.getValue("expected_revision").jsonPrimitive.content, "the next change starts from the newer revision")
        assertTrue(repo.routines.value.single().on)
    }

    /** An item of the Host's feed from the Host agent [agent], made in [subject]. */
    private fun item(id: String, kind: String, agent: Uuid, body: String, subject: Uuid, atMs: Long = 1_000, done: Boolean = false, choices: List<String>? = null) =
        buildJsonObject {
            put("id", id)
            put("kind", kind)
            put("agent", buildJsonObject { put("id", agent.toString()); put("name", "Kcal") })
            put("title", if (kind == "problem") "\u201cPrices\u201d didn\u2019t finish" else "Wants you to know")
            put("body", body)
            put("at_ms", atMs)
            put("done", done)
            put("subject_id", subject.toString())
            put("urgency", "soon")
            choices?.let { put("choices", JsonArray(it.map(::JsonPrimitive))) }
        }

    /** A Host agent on the roster, and its roster id. */
    private fun hostAgent(): Pair<Uuid, String> {
        val id = Uuid.random()
        surface.targets += TargetSummary(surface.node, "agent:$id", "Kcal")
        return id to "${surface.node}/agent:$id"
    }

    @Test
    fun theFeedsNotesReachHomeAndAnOpenQuestionIsItsAgentsAsk() = runTest {
        val (kcal, rosterId) = hostAgent()
        val stranger = Uuid.random()
        val routine = Uuid.random()
        surface.today = "Kcal has a question."
        surface.feed +=
            listOf(
                item("ask", "ask", kcal, "Dinner?", chatTaskId(surface.node, "agent:$kcal"), atMs = 5_000, choices = listOf("Dal")),
                item("older-ask", "ask", kcal, "Lunch?", Uuid.random(), atMs = 4_000),
                item("note", "update", kcal, "Prices fell.", Uuid.random()),
                item("problem", "problem", kcal, "Run it again.", routine),
                item("answered", "ask", kcal, "Breakfast?", Uuid.random(), atMs = 9_000, done = true),
                item("read", "update", kcal, "Already put away.", Uuid.random(), done = true),
                item("stray", "update", stranger, "From no one here.", Uuid.random()),
            )
        val repo = started()
        val feed = repo.feed.value
        assertEquals("Kcal has a question.", feed.today)
        assertEquals(listOf("note" to rosterId, "problem" to rosterId, "stray" to null), feed.notes.map { it.id to it.agentId }, "asks are no notes, and an agent not here is named by none")
        assertEquals(NoteKind.Problem, feed.notes[1].kind)
        val kcalHere = repo.agents.value.first { it.id == rosterId }
        assertEquals(AgentState.Wants, kcalHere.state)
        assertEquals(Ask("Dinner?", listOf("Dal"), Instant.ofEpochMilli(5_000)), kcalHere.glance.ask, "the newest open question; one answered is no question")
        assertNull(repo.agents.value.first { it.id == nagisa }.glance.ask)

        surface.feed.clear()
        surface.today = "All quiet."
        advanceTimeBy(1.minutes)
        runCurrent()
        assertEquals(Feed("All quiet.", emptyList()), repo.feed.value, "each roster check reads it again")
        assertNull(repo.agents.value.first { it.id == rosterId }.glance.ask)
    }

    @Test
    fun whileTheFeedCannotBeReadTheLastOneStays() = runTest {
        val (kcal, _) = hostAgent()
        surface.feed += item("note", "update", kcal, "Prices fell.", Uuid.random())
        val repo = started()
        surface.feed.clear()
        surface.feedFailure = IllegalStateException("dropped")
        advanceTimeBy(1.minutes)
        runCurrent()
        assertEquals(listOf("note"), repo.feed.value.notes.map { it.id })
    }

    @Test
    fun anAnswerToAQuestionAskedInItsChatIsSentAsIsAndOneAskedElsewhereNamesTheQuestionAndIsPutAway() = runTest {
        val (kcal, rosterId) = hostAgent()
        val chatTask = chatTaskId(surface.node, "agent:$kcal")
        surface.feed += item("here", "ask", kcal, "Dinner?", chatTask, choices = listOf("Dal"))
        val repo = started()
        repo.answer(rosterId, "Dal")
        runCurrent()
        assertEquals(chatTask to "Dal", surface.admitted.last().let { it.taskId to it.text })
        assertNull(repo.agents.value.first { it.id == rosterId }.glance.ask, "answered here, it goes at once")
        assertTrue(surface.feedCalls.none { it.first == HostMethod.Post }, "the Host marks it answered when it takes the message")

        val elsewhere = Uuid.random()
        surface.feed.clear()
        surface.feed += item("there", "ask", kcal, "Gift?", elsewhere)
        advanceTimeBy(1.minutes)
        runCurrent()
        repo.answer(rosterId, "  The scarf ")
        runCurrent()
        assertEquals(chatTask to "About your question \u201cGift?\u201d: The scarf", surface.admitted.last().let { it.taskId to it.text }, "its chat never saw the question")
        assertEquals(HostMethod.Post to "/v1/host/feed/items/there/dismissal", surface.feedCalls.last())
        assertNull(repo.agents.value.first { it.id == rosterId }.glance.ask)
    }

    @Test
    fun aDismissedNoteGoesAndOneTheHostCouldNotTakeShowsAgain() = runTest {
        val (kcal, _) = hostAgent()
        surface.feed += listOf(item("one", "update", kcal, "One.", Uuid.random()), item("two", "update", kcal, "Two.", Uuid.random()))
        val repo = started()
        repo.dismiss("one")
        assertEquals(listOf("two"), repo.feed.value.notes.map { it.id })
        assertEquals(HostMethod.Post to "/v1/host/feed/items/one/dismissal", surface.feedCalls.last())

        surface.feedFailure = IllegalStateException("dropped")
        assertEquals(Problem.Unavailable, assertFailsWith<RepositoryException> { repo.dismiss("two") }.problem)
        assertEquals(listOf("two"), repo.feed.value.notes.map { it.id }, "shown again")

        surface.feed.removeAt(1)
        repo.dismiss("two")
        assertEquals(emptyList(), repo.feed.value.notes, "one the Host no longer has is gone all the same")
    }

    @Test
    fun aProblemsRoutineRunsAgainAndARunAlreadyWaitingCountsAsRun() = runTest {
        val (kcal, _) = hostAgent()
        val routine = Uuid.random()
        val other = Uuid.random()
        surface.feed += listOf(item("p1", "problem", kcal, "Again.", routine), item("p2", "problem", kcal, "Again.", other), item("note", "update", kcal, "Hi.", Uuid.random()))
        val repo = started()
        repo.runAgain("p1")
        assertEquals(HostMethod.Post to "/v1/host/automations/$routine/run", surface.feedCalls.last())
        assertEquals(listOf("p2", "note"), repo.feed.value.notes.map { it.id })

        surface.running = true
        repo.runAgain("p2")
        assertEquals(listOf("note"), repo.feed.value.notes.map { it.id })

        assertEquals(Problem.NotFound, assertFailsWith<RepositoryException> { repo.runAgain("note") }.problem, "only a problem runs again")
        surface.running = false
        surface.feedFailure = IllegalStateException("dropped")
        surface.feed += item("p3", "problem", kcal, "Again.", Uuid.random())
        advanceTimeBy(1.minutes)
        runCurrent()
        surface.feedFailure = IllegalStateException("dropped")
        assertEquals(Problem.NotFound, assertFailsWith<RepositoryException> { repo.runAgain("p3") }.problem, "not yet read, so not here")
    }

    @Test
    fun aQuestionPastTheFirstPageOfTheFeedStillShows() = runTest {
        val (kcal, rosterId) = hostAgent()
        surface.feedPage = 2
        surface.feed += (1..4).map { item("n$it", "update", kcal, "Note $it.", Uuid.random()) }
        surface.feed += item("old-ask", "ask", kcal, "Still there?", Uuid.random())
        val repo = started()
        assertEquals("Still there?", repo.agents.value.first { it.id == rosterId }.glance.ask?.question)
        assertEquals(listOf("n1", "n2", "n3", "n4"), repo.feed.value.notes.map { it.id })
        assertEquals(listOf("/v1/host/feed", "/v1/host/feed/before/4", "/v1/host/feed/before/2"), surface.feedCalls.map { it.second })
    }

    @Test
    fun aReadThatAnswersAfterANewerOneIsDropped() = runTest {
        val (kcal, _) = hostAgent()
        surface.feed += item("x", "update", kcal, "X.", Uuid.random())
        val repo = started()
        surface.feedHeld = CompletableDeferred()
        val held = surface.feedHeld!!
        advanceTimeBy(1.minutes)
        runCurrent()
        repo.dismiss("x")
        repo.retryNow()
        runCurrent()
        assertEquals(emptyList(), repo.feed.value.notes)
        held.complete(Unit)
        runCurrent()
        assertEquals(emptyList(), repo.feed.value.notes, "the older read, which still had it open, does not bring it back")
    }

    @Test
    fun anAnswerTheCoordinatorRefusesShowsTheQuestionAgain() = runTest {
        val (kcal, rosterId) = hostAgent()
        surface.feed += item("ask", "ask", kcal, "Dinner?", chatTaskId(surface.node, "agent:$kcal"), choices = listOf("Dal"))
        val repo = started()
        surface.answers += Answer.Refuse(RcpErrorCode.NotFound)
        assertFailsWith<RepositoryException> { repo.answer(rosterId, "Dal") }
        assertEquals("Dinner?", repo.agents.value.first { it.id == rosterId }.glance.ask?.question)
    }

    @Test
    fun anOlderPageThatCannotBeReadKeepsTheLastFeedAndACursorThatDoesNotMoveEndsTheRead() = runTest {
        val (kcal, _) = hostAgent()
        surface.feedPage = 2
        surface.feed += (1..5).map { item("n$it", "update", kcal, "Note $it.", Uuid.random()) }
        val repo = started()
        surface.feed.add(0, item("new", "update", kcal, "New.", Uuid.random()))
        surface.olderPageFailure = IllegalStateException("dropped")
        advanceTimeBy(1.minutes)
        runCurrent()
        assertEquals(listOf("n1", "n2", "n3", "n4", "n5"), repo.feed.value.notes.map { it.id }, "half a feed would hide what sits further back")
        surface.feed.removeAt(0)

        surface.olderPageFailure = null
        surface.stuckCursor = true
        surface.feedCalls.clear()
        advanceTimeBy(1.minutes)
        runCurrent()
        assertEquals(listOf("n1", "n2", "n3", "n4"), repo.feed.value.notes.map { it.id })
        assertEquals(listOf("/v1/host/feed", "/v1/host/feed/before/4"), surface.feedCalls.map { it.second }, "a page that names itself again is not read again")
    }

    @Test
    fun anAnswerGoesEvenIfTheScreenThatTookItGoesFirst() = runTest {
        val (kcal, rosterId) = hostAgent()
        surface.feed += item("ask", "ask", kcal, "Dinner?", Uuid.random(), choices = listOf("Dal"))
        val repo = started()
        val held = CompletableDeferred<Unit>()
        surface.submitHeld = held
        val answering = backgroundScope.launch { repo.answer(rosterId, "Dal") }
        runCurrent()
        assertNull(repo.agents.value.first { it.id == rosterId }.glance.ask, "hidden while it goes")
        answering.cancel()
        held.complete(Unit)
        runCurrent()
        assertEquals("About your question \u201cDinner?\u201d: Dal", surface.admitted.last().text)
        assertEquals(HostMethod.Post to "/v1/host/feed/items/ask/dismissal", surface.feedCalls.last(), "and the question is put away")
        assertNull(repo.agents.value.first { it.id == rosterId }.glance.ask)
    }
}

private const val NEXT = 1_793_293_200_000L
private val KOLKATA = ZoneId.of("Asia/Kolkata")

private fun revisionOf(content: String): String = MessageDigest.getInstance("SHA-256").digest(content.toByteArray()).joinToString("") { "%02x".format(it) }
