package live.renoa.app.agents

import kotlinx.coroutines.CompletableDeferred
import kotlinx.coroutines.ExperimentalCoroutinesApi
import kotlinx.coroutines.async
import kotlinx.coroutines.flow.first
import kotlinx.coroutines.test.TestScope
import kotlinx.coroutines.test.advanceTimeBy
import kotlinx.coroutines.test.runCurrent
import kotlinx.coroutines.test.runTest
import live.renoa.rcp.ApplyLiveActivity
import live.renoa.rcp.CommandSubmission
import live.renoa.rcp.Disconnected
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

    override suspend fun connect() {
        connects++
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

    override suspend fun retryPendingCommands(): List<Uuid> =
        state.pendingCommands().map { command ->
            deliver(command)
            command.commandId
        }

    override fun close() = drop()
}

@OptIn(ExperimentalCoroutinesApi::class)
class RcpAgentRepositoryTest {
    private val state = InMemoryRcpState()
    private val surface = FakeSurface(state)
    private val nagisa get() = "${surface.node}/nagisa"
    private val chat get() = surface.journal("nagisa")

    private fun TestScope.started() =
        RcpAgentRepository(surface, state, backgroundScope, retryAfter = { 5.seconds }, rosterEvery = 1.minutes).also {
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
}
