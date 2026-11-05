package live.renoa.app.agents

import kotlinx.coroutines.test.runTest
import live.renoa.rcp.Attachment
import live.renoa.rcp.PendingTextCommand
import java.io.File
import java.io.IOException
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
    fun theFilesAMessageNamesAreKeptWithItAndAnOlderOutboxStillLoads() = runTest {
        val photo = PendingTextCommand(Uuid.random(), chat, "", listOf(Attachment(Uuid.random(), "fridge.jpg", "image/jpeg")))
        SavedOutbox(file).enqueueCommand(photo)
        assertEquals(listOf(photo), SavedOutbox(file).pendingCommands())

        val older = Uuid.random()
        file.writeText("""[{"commandId":"$older","taskId":"$chat","text":"from before files"}]""")
        assertEquals(listOf(PendingTextCommand(older, chat, "from before files")), SavedOutbox(file).pendingCommands())
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
    fun anOutboxThatCannotBeReadAtAllIsLeftAloneAndRefusesNewMessages() = runTest {
        file.mkdir()
        assertFailsWith<IOException>("not taken for an empty outbox") { SavedOutbox(file).pendingCommands() }
        assertFailsWith<IllegalStateException> { SavedOutbox(file).enqueueCommand(command("hello")) }
        assertTrue(file.isDirectory, "not written over")
    }

    @Test
    fun forgettingClearsTheOutboxOnDiskAndAnyHalfWrittenOne() = runTest {
        val outbox = SavedOutbox(file)
        outbox.enqueueCommand(command("hello"))
        File(dir, "outbox.json.new").writeText("[]")
        outbox.forget()
        assertTrue(outbox.pendingCommands().isEmpty())
        assertTrue(SavedOutbox(file).pendingCommands().isEmpty())
        assertEquals(emptyList(), dir.list()?.toList())
    }

    @Test
    fun aMessageThatCannotBeKeptIsRefusedAndLeavesNothingBehind() = runTest {
        val nowhere = SavedOutbox(File(File(dir, "missing"), "outbox.json"))
        assertFailsWith<IllegalStateException> { nowhere.enqueueCommand(command("hello")) }
        assertTrue(nowhere.pendingCommands().isEmpty(), "so it is never sent unkept")
    }
}
