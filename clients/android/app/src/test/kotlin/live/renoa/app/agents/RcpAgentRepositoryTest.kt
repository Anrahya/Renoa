package live.renoa.app.agents

import java.security.MessageDigest
import kotlinx.coroutines.CompletableDeferred
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
import kotlinx.coroutines.test.TestScope
import kotlinx.coroutines.test.advanceTimeBy
import kotlinx.coroutines.test.runCurrent
import kotlinx.coroutines.test.runTest
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

    /** When set, the next observation ends with it instead of reaching the Host. */
    var observationFailure: Exception? = null

    /** When set, the next observation reads the automations, then answers only once this completes. */
    var observationHeld: CompletableDeferred<Unit>? = null

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

    override suspend fun listTasks(): List<TaskSummary> = error("unused")

    override suspend fun listTargets(): List<TargetSummary> = targets.also { check(online) }

    override suspend fun openTask(taskId: Uuid, nodeId: Uuid, target: String) {
        check(online)
        if (target in unopenable) throw RcpException(RcpErrorCode.NodeOffline, "offline", 1)
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
            return HostAnswer(200, buildJsonObject { put("automations", JsonArray(listed)); put("agents", JsonArray(hostAgents.values.toList())) }.toString())
        }
        if (path == "/v1/host/profile") return profileCall(method, body)
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

    override suspend fun retryPendingCommands(): List<Uuid> =
        state.pendingCommands().map { command ->
            deliver(command)
            command.commandId
        }

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

    private fun TestScope.started() =
        RcpAgentRepository(surface, state, backgroundScope, retryAfter = { 5.seconds }, rosterEvery = 1.minutes, here = { KOLKATA }).also {
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
            mapOf("operation_id" to kcal, "preset_id" to "renoa.general.v1", "name" to "Kcal", "purpose" to "Count my calories."),
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

    private fun automation(id: String, agent: Uuid, expression: String, enabled: Boolean = true, revision: Long = 1, lastRun: JsonObject? = null) =
        buildJsonObject {
            put("id", id)
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
}

private const val NEXT = 1_793_293_200_000L
private val KOLKATA = ZoneId.of("Asia/Kolkata")

private fun revisionOf(content: String): String = MessageDigest.getInstance("SHA-256").digest(content.toByteArray()).joinToString("") { "%02x".format(it) }
