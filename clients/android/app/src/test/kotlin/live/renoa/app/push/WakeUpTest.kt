package live.renoa.app.push

import kotlinx.coroutines.CompletableDeferred
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.coroutineScope
import kotlinx.coroutines.delay
import kotlinx.coroutines.joinAll
import kotlinx.coroutines.launch
import kotlinx.coroutines.runBlocking
import kotlinx.coroutines.withTimeout
import kotlinx.serialization.json.JsonNull
import kotlinx.serialization.json.JsonObject
import kotlinx.serialization.json.buildJsonObject
import kotlinx.serialization.json.put
import kotlinx.serialization.json.putJsonObject
import live.renoa.app.agents.AgentState
import live.renoa.app.agents.AgentSummary
import live.renoa.app.agents.agentId
import live.renoa.app.agents.chatTaskId
import live.renoa.rcp.Authentication
import live.renoa.rcp.CommandEnvelope
import live.renoa.rcp.CommandInput
import live.renoa.rcp.ExecutionEvent
import live.renoa.rcp.ExecutionEventKind
import live.renoa.rcp.ExecutionTerminal
import live.renoa.rcp.FakeCoordinator
import live.renoa.rcp.InMemoryRcpState
import live.renoa.rcp.TaskEvent
import live.renoa.rcp.TaskEventKind
import live.renoa.rcp.TaskStatus
import live.renoa.rcp.TaskSummary
import live.renoa.rcp.WebSocketRcpSurface
import java.io.File
import java.io.IOException
import java.nio.file.Files
import kotlin.test.AfterTest
import kotlin.test.Test
import kotlin.test.assertEquals
import kotlin.test.assertFalse
import kotlin.test.assertNull
import kotlin.test.assertTrue
import kotlin.time.Duration.Companion.seconds
import kotlin.uuid.Uuid

/** A wake-up against a coordinator that speaks only the raw binding, over a real socket. */
class WakeUpTest {
    private val coordinator = FakeCoordinator()
    private val dir: File = Files.createTempDirectory("wake").toFile()
    private val posted = mutableListOf<Notice>()
    private val shown = ShownChats()
    private var allows = true
    private var token: suspend () -> String? = { "fcm-token" }

    /** Set to make the next notices fail to post, as when the process dies on the way. */
    private var postFails: (Notice) -> Boolean = { false }
    private val arcee get() = AgentSummary(agentId(coordinator.nodeId, "agent:arcee"), "Arcee", "", AgentState.Resting)
    private val arceesChat get() = chatTaskId(coordinator.nodeId, "agent:arcee")

    private fun wakeUp(cursors: WakeCursors) =
        WakeUp(
            cursors,
            connect = { state -> WebSocketRcpSurface(coordinator.endpoint, Authentication.Device(coordinator.credentials), state) },
            register = PushRegistration({ token() }, { allows }, cursors)::register,
            sender = { task -> sender(task, listOf(arcee)) },
            onScreen = { it in shown },
            post = {
                if (postFails(it)) throw IllegalStateException("the process died")
                posted += it
            },
        )

    private val cursors = WakeCursors(File(dir, "wake"), owner = "renoa.test me")
    private val wake = wakeUp(cursors)

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
        val chat = coordinator.seedTask(arceesChat)
        coordinator.append(chat, reply("An old answer."))

        assertTrue(wake.run())
        assertEquals("fcm-token", coordinator.pushToken, "the token reaches Renoa on the wake-up's connection")
        assertTrue(posted.isEmpty(), "the history a phone signed in to is not news")

        coordinator.append(chat, reply("  Dinner is at seven.  "))
        assertTrue(wake.run())
        assertEquals(listOf(Notice(chat, "Arcee", arcee.id, News.Replied("Dinner is at seven."))), posted, "a tap opens Arcee's chat")

        assertTrue(wake.run())
        assertEquals(1, posted.size, "read once, shown once")
        assertEquals(1, coordinator.framesOfType("attach").size, "a chat that did not move is not attached")
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
    fun whatTheOwnerReadInTheAppIsNotShownByALaterWake() = wakes {
        val chat = coordinator.seedTask(arceesChat)
        wake.run()
        coordinator.append(chat, reply("Read in the app before any wake came."))

        // The app's own connection follows the chat, and the owner had it on screen.
        val applied = InMemoryRcpState()
        val app = WebSocketRcpSurface(coordinator.endpoint, Authentication.Device(coordinator.credentials), applied)
        app.connect()
        app.attach(chat) {}
        app.close()
        markRead(chat, applied, cursors)

        assertTrue(wake.run())
        assertTrue(posted.isEmpty(), "a wake that arrives late shows nothing the owner has read")
    }

    @Test
    fun aRunThatFailedShowsAsStuckAndAChatFirstSeenAfterTheBaselineIsReadFromItsStart() = wakes {
        coordinator.seedTask()
        wake.run()

        val newer = coordinator.seedTask(target = "agent:unknown")
        coordinator.append(newer, reply("Started on it."))
        coordinator.append(newer, failed())
        wake.run()
        assertEquals(listOf(Notice(newer, null, null, News.Stuck)), posted, "an agent the app has not kept goes unnamed, and a tap opens the app")
    }

    @Test
    fun aNoticeThatDidNotPostIsShownByTheNextWakeAndTheOtherChatsStillAre() = wakes {
        val first = coordinator.seedTask(Uuid.parse("00000000-0000-0000-0000-000000000001"))
        val second = coordinator.seedTask(Uuid.parse("00000000-0000-0000-0000-000000000002"))
        wake.run()
        coordinator.append(first, reply("First."))
        coordinator.append(second, reply("Second."))
        postFails = { it.taskId == first }

        assertFalse(wake.run(), "a chat that failed asks for the wake-up again")
        assertEquals(listOf(second), posted.map { it.taskId }, "the chat listed after it is still shown")

        postFails = { false }
        assertTrue(wake.run())
        assertEquals(listOf(second, first), posted.map { it.taskId }, "its cursor moved only once its notice was up")
    }

    @Test
    fun aCursorThatCannotBeKeptAsksForTheWakeAgain() = wakes {
        val chat = coordinator.seedTask()
        wake.run()
        coordinator.append(chat, reply("Kept nowhere."))
        dir.setWritable(false)
        try {
            assertFalse(wake.run())
        } finally {
            dir.setWritable(true)
        }
        assertTrue(wake.run())
        assertEquals(2, posted.size, "shown again, under the same notice, until the cursor is kept")
    }

    @Test
    fun aBaselineThatCannotBeKeptShowsNothingAndAsksForTheWakeAgain() = wakes {
        val blocked = File(dir, "not-a-directory").apply { writeText("") }
        val nowhere = wakeUp(WakeCursors(File(blocked, "wake"), owner = "renoa.test me"))
        coordinator.append(coordinator.seedTask(), reply("Before the baseline."))
        assertFalse(nowhere.run())
        assertTrue(posted.isEmpty())
    }

    @Test
    fun aTokenThatCouldNotBeHadAsksForTheWakeAgainButStillCatchesUp() = wakes {
        val chat = coordinator.seedTask()
        wake.run()
        coordinator.append(chat, reply("Still shown."))
        token = { throw IOException("Firebase did not answer") }

        assertFalse(wake.run(), "the new token must still reach Renoa")
        assertEquals(1, posted.size)
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
    fun anOlderRegistrationNeverLandsAfterANewerOne() = wakes {
        val asked = CompletableDeferred<Unit>()
        val issuing = CompletableDeferred<Unit>()
        val registration =
            PushRegistration({
                asked.complete(Unit)
                issuing.await()
                "fcm-token"
            }, { allows }, cursors)
        val app = WebSocketRcpSurface(coordinator.endpoint, Authentication.Device(coordinator.credentials), InMemoryRcpState())
        app.connect()
        coroutineScope {
            val older = launch { registration.register(app) }
            asked.await()
            // The owner turned notifications off while Firebase was still issuing the token.
            allows = false
            val newer = launch { registration.register(app) }
            delay(200)
            issuing.complete(Unit)
            joinAll(older, newer)
        }
        app.close()
        assertNull(coordinator.pushToken, "the clear that came last is what Renoa keeps")
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
    fun aTaskIsNamedByItsTargetAndOpensTheChatOnlyWhenItIsThatChat() {
        val node = Uuid.random()
        val kept = listOf(AgentSummary(agentId(node, "agent:arcee"), "Arcee", "", AgentState.Resting))
        val chat = TaskSummary(chatTaskId(node, "agent:arcee"), "agent:arcee", TaskStatus.Idle, 0)
        val routine = TaskSummary(Uuid.random(), "agent:arcee", TaskStatus.Idle, 0)
        assertEquals(Sender("Arcee", agentId(node, "agent:arcee")), sender(chat, kept))
        assertEquals(Sender("Arcee", null), sender(routine, kept), "a routine's task is not the chat a tap could open")
        assertEquals(Sender(null, null), sender(chat, listOf(AgentSummary("tifa", "Tifa", "", AgentState.Resting))), "an id the app did not make names no one")
    }

    @Test
    fun onlyRenoasWakesStartACatchUp() {
        assertTrue(isWake(mapOf("renoa" to "changed")))
        assertTrue(isWake(mapOf("renoa" to "ring")), "what needs the owner now reads the same way")
        assertTrue(!isWake(mapOf("renoa" to "something else")) && !isWake(emptyMap()))
    }

    @Test
    fun aQueuedWakeThatALaterCatchUpCoveredHasNothingLeftToDo() {
        var clock = 0L
        val coverage = Coverage { clock }
        val first = coverage.now().also { clock++ }
        val second = coverage.now().also { clock++ }
        assertFalse(coverage.covers(first) || coverage.covers(second), "nothing has caught up yet")

        // The first wake's catch-up started after both were queued, and finished.
        coverage.finished(coverage.now())
        assertTrue(coverage.covers(second), "so the one queued behind it reads nothing again")

        val third = coverage.now().also { clock++ }
        assertFalse(coverage.covers(third), "one queued since must still read")
    }
}
