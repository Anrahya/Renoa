package live.renoa.app.agents

import kotlinx.coroutines.ExperimentalCoroutinesApi
import kotlinx.coroutines.flow.flowOf
import kotlinx.coroutines.test.UnconfinedTestDispatcher
import kotlinx.coroutines.test.runTest
import java.io.File
import java.nio.file.Files
import java.time.Instant
import kotlin.test.Test
import kotlin.test.assertEquals
import kotlin.test.assertNull
import kotlin.test.assertTrue

@OptIn(ExperimentalCoroutinesApi::class)
class SavedHomeTest {
    private val dir = Files.createTempDirectory("home").toFile()
    private val file = File(dir, "home")
    private val routine = Routine("brief", "renoa", "Morning brief", "Every day at 7:30 AM", on = true, next = Instant.ofEpochMilli(1_800_000_000_000), lastRun = LastRun(Instant.ofEpochMilli(1_799_990_000_000), RunOutcome.Done))
    private val home = HomeSnapshot(listOf(AgentSummary("renoa", "Renoa", "Helps.", AgentState.Wants, Glance(line = "Dinner?", unread = true, ask = Ask("Dinner?", listOf("Dal", "Pasta"))), primary = true)), listOf(routine))

    @Test
    fun whatHomeShowedIsReadBackForTheSameSignInOnly() = runTest {
        SavedHome(file, "a", UnconfinedTestDispatcher(testScheduler)).keep(flowOf(home))
        assertEquals(home, SavedHome(file, "a").load())
        assertNull(SavedHome(file, "b").load(), "another sign-in reads nothing")
        assertEquals(listOf("home"), dir.list()?.toList(), "nothing is left beside it")
    }

    @Test
    fun aFileThatCannotBeReadIsNothingKept() {
        file.writeText("{\"owner\":\"a\",\"home\":")
        assertNull(SavedHome(file, "a").load())
        assertNull(SavedHome(File(dir, "missing"), "a").load())
    }

    @Test
    fun forgettingLeavesNothingBehind() = runTest {
        val saved = SavedHome(file, "a", UnconfinedTestDispatcher(testScheduler))
        saved.keep(flowOf(home))
        file.replacement.writeText("half written")
        saved.forget()
        assertNull(saved.load())
        assertTrue(dir.list().isNullOrEmpty())
    }
}
