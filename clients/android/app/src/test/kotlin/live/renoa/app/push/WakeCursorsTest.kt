package live.renoa.app.push

import kotlinx.coroutines.test.runTest
import live.renoa.rcp.TaskStatus
import live.renoa.rcp.TaskSummary
import java.io.File
import java.nio.file.Files
import kotlin.test.Test
import kotlin.test.assertEquals
import kotlin.test.assertFalse
import kotlin.test.assertNull
import kotlin.test.assertTrue
import kotlin.uuid.Uuid

class WakeCursorsTest {
    private val dir = Files.createTempDirectory("wake").toFile()
    private val file = File(dir, "wake")
    private val chat = Uuid.random()
    private val empty = Uuid.random()

    private fun task(id: Uuid, through: Long?) = TaskSummary(id, "agent:arcee", TaskStatus.Idle, through)

    @Test
    fun theBaselineIsTakenOnceAndOutlivesTheApp() = runTest {
        val first = WakeCursors(file, "me")
        assertFalse(first.baselined())
        first.baseline(listOf(task(chat, 4), task(empty, null)))
        first.baseline(listOf(task(chat, 9)))

        val next = WakeCursors(file, "me")
        assertTrue(next.baselined(), "a new launch reads it back")
        assertEquals(4, next.cursor(chat), "a second baseline changes nothing")
        assertNull(next.cursor(empty), "an empty task has no cursor, so all of it is read once it has records")
        assertEquals(listOf("wake"), dir.list()?.toList(), "nothing is left beside it")
    }

    @Test
    fun aCursorOnlyMovesForward() = runTest {
        val cursors = WakeCursors(file, "me")
        cursors.baseline(emptyList())
        cursors.advance(chat, 7)
        cursors.advance(chat, 3)
        assertEquals(7, WakeCursors(file, "me").cursor(chat))
    }

    @Test
    fun anotherSignInsCursorsOrAnUnreadableFileCountAsNothingKept() = runTest {
        WakeCursors(file, "me").baseline(listOf(task(chat, 4)))
        assertFalse(WakeCursors(file, "someone else").baselined())

        file.writeText("not cursors")
        assertFalse(WakeCursors(file, "me").baselined())
    }

    @Test
    fun forgettingDropsTheFileAndTheBaseline() = runTest {
        val cursors = WakeCursors(file, "me")
        cursors.baseline(listOf(task(chat, 4)))
        cursors.forget()
        assertFalse(cursors.baselined())
        assertFalse(file.exists())
    }

    @Test
    fun aChatCountsAsShownUntilEveryScreenOfItHasGone() {
        val shown = ShownChats()
        shown.show(chat)
        shown.show(chat)
        shown.hide(chat)
        assertTrue(chat in shown, "the screen it moved to still shows it")
        shown.hide(chat)
        assertFalse(chat in shown)
        shown.hide(chat)
        assertFalse(chat in shown, "hiding what is not shown changes nothing")
    }
}
