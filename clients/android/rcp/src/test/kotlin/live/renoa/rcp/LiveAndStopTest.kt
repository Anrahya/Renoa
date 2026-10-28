package live.renoa.rcp

import kotlinx.serialization.json.buildJsonObject
import kotlinx.serialization.json.put
import java.util.Collections
import kotlin.test.Test
import kotlin.test.assertEquals
import kotlin.test.assertFailsWith
import kotlin.test.assertIs
import kotlin.uuid.Uuid

/** Task status and agent names, live activity beside the journal, and stopping a command. */
class LiveAndStopTest : CoordinatorTest() {
    private fun delta(sequence: Long, text: String) =
        buildJsonObject {
            put("type", "text_delta")
            put("sequence", sequence)
            put("text", text)
        }

    @Test
    fun aTasksStatusAndAnAgentsNameAreRead() = eventually {
        val working = coordinator.seedTask()
        coordinator.tasks.getValue(working).status = "working"
        val surface = client()
        surface.connect()

        assertEquals(TaskStatus.Working, surface.listTasks().single().status)
        assertEquals(listOf("Alpha", "Arcee"), surface.listTargets().map { it.name })
    }

    @Test
    fun aLiveAttachmentGetsFramesInOrderWithTheTasksEvents() = eventually {
        val taskId = coordinator.seedTask()
        val commandId = Uuid.random()
        val seen = Collections.synchronizedList(mutableListOf<String>())
        val surface = client()
        surface.connect()
        surface.attach(taskId, live = { command, activity ->
            assertEquals(commandId, command)
            seen += (activity as? LiveActivity.TextDelta)?.text ?: "reset"
        }) { seen += "record ${it.sequence}" }
        assertEquals("true", coordinator.framesOfType("attach").single().getValue("live").toString())

        coordinator.publishLive(taskId, commandId, delta(0, "Hel"))
        coordinator.publishLive(taskId, commandId, buildJsonObject { put("type", "text_reset") })
        coordinator.publishLive(taskId, commandId, delta(0, "Hello"))
        coordinator.publish(taskId, coordinator.assistantMessage(commandId, "Hello"))
        awaitCursor(taskId, 0)
        assertEquals(listOf("Hel", "reset", "Hello", "record 0"), seen)
    }

    @Test
    fun liveActivityIsAskedForAgainAfterAReconnect() = eventually {
        val taskId = coordinator.seedTask()
        val surface = client()
        surface.connect()
        surface.attach(taskId, live = { _, _ -> }) {}
        surface.disconnect()
        surface.connect()
        assertEquals(listOf("true", "true"), coordinator.framesOfType("attach").map { it.getValue("live").toString() })
    }

    @Test
    fun aDrawingFailureNeitherEndsTheConnectionNorHoldsBackTheRecord() = eventually {
        val taskId = coordinator.seedTask()
        val commandId = Uuid.random()
        val records = Collections.synchronizedList(mutableListOf<Long>())
        val surface = client()
        surface.connect()
        surface.attach(taskId, live = { _, _ -> error("the draft could not be drawn") }) { records += it.sequence }

        coordinator.publishLive(taskId, commandId, delta(0, "Hi"))
        coordinator.publish(taskId, coordinator.assistantMessage(commandId, "Hi"))
        awaitCursor(taskId, 0)
        assertEquals(listOf(0L), records)
        assertEquals(1, surface.listTasks().size)
    }

    @Test
    fun anAttachmentThatDidNotAskSaysSoAndLiveActivityForItEndsTheSession() = eventually {
        val taskId = coordinator.seedTask()
        val surface = client()
        surface.connect()
        surface.attach(taskId) {}
        assertEquals("false", coordinator.framesOfType("attach").single().getValue("live").toString())

        coordinator.injectRaw(
            """{"type":"live_activity","task_id":"$taskId","command_id":"${Uuid.random()}","activity":{"type":"text_reset"}}""",
        )
        assertIs<ProtocolViolation>(surface.awaitDisconnect())
    }

    @Test
    fun aStopIsAnsweredWithItsOutcomeAndRepeatsSafely() = eventually {
        val taskId = coordinator.seedTask()
        val surface = client()
        surface.connect()
        val running = surface.submitText(taskId, "write a long essay").also { it.accepted.await() }.commandId
        val finished = surface.submitText(taskId, "hello").also { it.accepted.await() }.commandId
        coordinator.tasks.getValue(taskId).ended += finished.toString()

        assertEquals(StopOutcome.Requested, surface.stopCommand(taskId, running))
        assertEquals(StopOutcome.Requested, surface.stopCommand(taskId, running), "a recorded stop answers requested again")
        assertEquals(StopOutcome.AlreadyEnded, surface.stopCommand(taskId, finished))
        val unknown = assertFailsWith<RcpException> { surface.stopCommand(taskId, Uuid.random()) }
        assertEquals(RcpErrorCode.NotFound, unknown.code)

        val frame = coordinator.framesOfType("stop_command").first()
        assertEquals(taskId.toString(), frame.getValue("task_id").toString().trim('"'))
        assertEquals(running.toString(), frame.getValue("command_id").toString().trim('"'))
    }

    @Test
    fun aStopAnswerForAnotherCommandIsNotTrusted() = eventually {
        val taskId = coordinator.seedTask()
        val surface = client()
        surface.connect()
        val commandId = surface.submitText(taskId, "hi").also { it.accepted.await() }.commandId
        coordinator.rewriteReply = { reply ->
            if (reply.getValue("type").toString() == "\"stop_accepted\"") {
                buildJsonObject {
                    reply.forEach { (key, value) -> put(key, value) }
                    put("command_id", Uuid.random().toString())
                }
            } else {
                reply
            }
        }
        assertFailsWith<ProtocolViolation> { surface.stopCommand(taskId, commandId) }
    }
}
