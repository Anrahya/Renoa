package live.renoa.app.push

import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.runBlocking
import kotlinx.coroutines.withTimeout
import kotlinx.serialization.json.JsonObject
import kotlinx.serialization.json.buildJsonObject
import kotlinx.serialization.json.put
import kotlinx.serialization.json.putJsonObject
import live.renoa.app.agents.AgentState
import live.renoa.app.agents.AgentSummary
import live.renoa.app.agents.agentId
import kotlinx.serialization.json.JsonNull
import live.renoa.rcp.Authentication
import live.renoa.rcp.CommandEnvelope
import live.renoa.rcp.CommandInput
import live.renoa.rcp.ExecutionEvent
import live.renoa.rcp.ExecutionEventKind
import live.renoa.rcp.ExecutionTerminal
import live.renoa.rcp.FakeCoordinator
import live.renoa.rcp.TaskEvent
import live.renoa.rcp.TaskEventKind
import live.renoa.rcp.TaskStatus
import live.renoa.rcp.TaskSummary
import live.renoa.rcp.WebSocketRcpSurface
import java.nio.file.Files
import kotlin.test.AfterTest
import kotlin.test.Test
import kotlin.test.assertEquals
import kotlin.test.assertNull
import kotlin.test.assertTrue
import kotlin.time.Duration.Companion.seconds
import kotlin.uuid.Uuid

/** A wake-up against a coordinator that speaks only the raw binding, over a real socket. */
class WakeUpTest {
    private val coordinator = FakeCoordinator()
    private val cursors = WakeCursors(Files.createTempDirectory("wake").toFile().resolve("wake"), owner = "renoa.test me")
    private val posted = mutableListOf<Notice>()
    private val shown = ShownChats()
    private var allows = true

    private val wake =
        WakeUp(
            cursors,
            connect = { state -> WebSocketRcpSurface(coordinator.endpoint, Authentication.Device(coordinator.credentials), state) },
            register = PushRegistration({ "fcm-token" }, { allows }, cursors)::register,
            agentName = { task -> agentName(task, listOf(AgentSummary(agentId(coordinator.nodeId, "agent:arcee"), "Arcee", "", AgentState.Resting))) },
            onScreen = { it in shown },
            post = { posted += it },
        )

    @AfterTest
    fun tearDown() = coordinator.close()

    private fun wakes(block: suspend () -> Unit) = runBlocking(Dispatchers.Default) { withTimeout(15.seconds) { block() } }

    private fun reply(text: String): JsonObject = coordinator.assistantMessage(Uuid.random(), text)

    private fun failed(): JsonObject =
        buildJsonObject {
            put("type", "execution_event")
            put("commandId", Uuid.random().toString())
            putJsonObject("event") {
                put("eventId", Uuid.random().toString())
                put("executionId", Uuid.random().toString())
                put("sequence", 2L)
                put("recordedAtMs", 1_786_137_600_000L)
                putJsonObject("kind") {
                    put("type", "execution_terminated")
                    putJsonObject("terminal") {
                        put("status", "failed")
                        put("error", "the model refused the request")
                    }
                }
            }
        }

    @Test
    fun theFirstWakeOnlyMarksWhereEachChatStandsAndLaterOnesShowWhatIsNewOnce() = wakes {
        val chat = coordinator.seedTask()
        coordinator.append(chat, reply("An old answer."))

        wake.run()
        assertEquals("fcm-token", coordinator.pushToken, "the token reaches Renoa on the wake-up's connection")
        assertTrue(posted.isEmpty(), "the history a phone signed in to is not news")

        coordinator.append(chat, reply("  Dinner is at seven.  "))
        wake.run()
        assertEquals(listOf(Notice(chat, "Arcee", News.Replied("Dinner is at seven."))), posted)

        wake.run()
        assertEquals(1, posted.size, "read once, shown once")
        assertTrue(coordinator.framesOfType("attach").size == 1, "a chat that did not move is not attached")
    }

    @Test
    fun aChatOnScreenIsReadButNotShown() = wakes {
        val chat = coordinator.seedTask()
        wake.run()
        coordinator.append(chat, reply("You are reading this already."))
        shown.show(chat)

        wake.run()
        assertTrue(posted.isEmpty())

        shown.hide(chat)
        wake.run()
        assertTrue(posted.isEmpty(), "and it is not shown later either")
    }

    @Test
    fun aRunThatFailedShowsAsStuckAndAChatFirstSeenAfterTheBaselineIsReadFromItsStart() = wakes {
        coordinator.seedTask()
        wake.run()

        val newer = coordinator.seedTask(target = "agent:unknown")
        coordinator.append(newer, reply("Started on it."))
        coordinator.append(newer, failed())
        wake.run()
        assertEquals(listOf(Notice(newer, null, News.Stuck)), posted, "an agent the app has not kept goes unnamed")
    }

    @Test
    fun aPhoneWithNotificationsOffTakesItsTokenBack() = wakes {
        wake.run()
        assertEquals("fcm-token", coordinator.pushToken)
        allows = false
        val chat = coordinator.seedTask()
        coordinator.append(chat, reply("Nobody will see this."))

        wake.run()
        assertNull(coordinator.pushToken, "Renoa stops waking a phone that shows nothing")
    }

    @Test
    fun onlyAReplyWithWordsAndAFailedRunAreNewsAndTheLaterOneWins() {
        val command = Uuid.random()
        var sequence = 0L
        fun event(kind: TaskEventKind) = TaskEvent(Uuid.random(), Uuid.random(), sequence++, kind)
        fun execution(kind: ExecutionEventKind) = event(TaskEventKind.ExecutionRecorded(command, ExecutionEvent(Uuid.random(), Uuid.random(), sequence, 0, kind)))
        val submitted = event(TaskEventKind.CommandSubmitted(CommandEnvelope(command, Uuid.random(), "android", "agent:arcee", CommandInput.Text("hi"))))
        val quiet =
            listOf(
                submitted,
                execution(ExecutionEventKind.ExecutionStarted),
                execution(ExecutionEventKind.TurnStarted),
                execution(ExecutionEventKind.ToolStarted("call", "search", JsonNull)),
                execution(ExecutionEventKind.ToolFinished("call", "found", isError = false)),
                execution(ExecutionEventKind.AssistantMessage(" \n ")),
                execution(ExecutionEventKind.ExecutionTerminated(ExecutionTerminal.Completed)),
                execution(ExecutionEventKind.ExecutionTerminated(ExecutionTerminal.Cancelled("stopped by the owner"))),
            )
        assertNull(news(quiet), "progress, an empty reply, a completed run and a stopped one are not news")

        val replied = execution(ExecutionEventKind.AssistantMessage("Done."))
        val failed = execution(ExecutionEventKind.ExecutionTerminated(ExecutionTerminal.Failed("boom")))
        assertEquals(News.Replied("Done."), news(quiet + replied))
        assertEquals(News.Stuck, news(listOf(replied, failed)))
        assertEquals(News.Replied("Done."), news(listOf(failed, replied)))
    }

    @Test
    fun anAgentIsNamedFromWhatTheAppKeptByItsTarget() {
        val task = TaskSummary(Uuid.random(), "agent:arcee", TaskStatus.Idle, 0)
        assertEquals("Arcee", agentName(task, listOf(AgentSummary(agentId(Uuid.random(), "agent:arcee"), "Arcee", "", AgentState.Resting))))
        assertNull(agentName(task, listOf(AgentSummary("tifa", "Tifa", "", AgentState.Resting))), "an id the app did not make names no one")
    }

    @Test
    fun onlyRenoasWakeStartsACatchUp() {
        assertTrue(isWake(mapOf("renoa" to "changed")))
        assertTrue(!isWake(mapOf("renoa" to "something else")) && !isWake(emptyMap()))
    }
}
