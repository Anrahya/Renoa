package live.renoa.app.agents

import kotlinx.coroutines.test.runTest
import live.renoa.rcp.PendingTextCommand
import java.io.File
import java.nio.file.Files
import kotlin.test.Test
import kotlin.test.assertEquals
import kotlin.test.assertFailsWith
import kotlin.test.assertNull
import kotlin.test.assertTrue
import kotlin.uuid.Uuid

class SavedOutboxTest {
    private val dir = Files.createTempDirectory("outbox").toFile()
    private val file = File(dir, "outbox.json")
    private val chat = Uuid.random()

    private fun command(text: String) = PendingTextCommand(Uuid.random(), chat, text)

    @Test
    fun messagesWaitingToGoOutliveTheAppInTheOrderTheyWereSent() = runTest {
        val first = command("first")
        val second = command("second")
        SavedOutbox(file).apply {
            enqueueCommand(first)
            enqueueCommand(second)
        }
        assertEquals(listOf(first, second), SavedOutbox(file).pendingCommands(), "a new launch reads them back")

        SavedOutbox(file).removeCommand(first.commandId)
        assertEquals(listOf(second), SavedOutbox(file).pendingCommands())
        assertEquals(listOf("outbox.json"), dir.list()?.toList(), "nothing is left beside it")
    }

    @Test
    fun theChatIsReplayedInFullOnEveryLaunch() = runTest {
        SavedOutbox(file).advanceCursor(chat, 7)
        assertNull(SavedOutbox(file).cursor(chat), "a launch starts with no cursor, so the whole chat comes back")
    }

    @Test
    fun anOutboxThatCannotBeReadStartsEmptyAndIsWrittenAfresh() = runTest {
        file.writeText("not an outbox")
        val outbox = SavedOutbox(file)
        assertTrue(outbox.pendingCommands().isEmpty())
        val next = command("hello")
        outbox.enqueueCommand(next)
        assertEquals(listOf(next), SavedOutbox(file).pendingCommands())
    }

    @Test
    fun aMessageThatCannotBeKeptIsRefusedAndLeavesNothingBehind() = runTest {
        val nowhere = SavedOutbox(File(File(dir, "missing"), "outbox.json"))
        assertFailsWith<IllegalStateException> { nowhere.enqueueCommand(command("hello")) }
        assertTrue(nowhere.pendingCommands().isEmpty(), "so it is never sent unkept")
    }
}
