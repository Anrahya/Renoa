package live.renoa.app.chat

import kotlinx.coroutines.CompletableDeferred
import kotlinx.coroutines.ExperimentalCoroutinesApi
import kotlinx.coroutines.launch
import kotlinx.coroutines.test.TestScope
import kotlinx.coroutines.test.advanceTimeBy
import kotlinx.coroutines.test.runCurrent
import kotlinx.coroutines.test.runTest
import live.renoa.app.MainDispatcherRule
import live.renoa.app.agents.AgentRepository
import live.renoa.app.agents.AgentState
import live.renoa.app.agents.AgentSummary
import live.renoa.app.agents.CreateAgent
import live.renoa.app.agents.FakeAgentRepository
import live.renoa.app.agents.FakeWorld
import live.renoa.app.badge.AgentHue
import live.renoa.app.agents.Problem
import live.renoa.app.agents.Reply
import live.renoa.app.agents.ReplyScript
import live.renoa.app.agents.RepositoryException
import live.renoa.app.agents.Speaker
import org.junit.Rule
import kotlin.test.Test
import kotlin.test.assertEquals
import kotlin.test.assertFalse
import kotlin.test.assertNotNull
import kotlin.test.assertNull
import kotlin.test.assertTrue

@OptIn(ExperimentalCoroutinesApi::class)
class ChatViewModelTest {
    @get:Rule val main = MainDispatcherRule()

    private val oneTwoThree = ReplyScript { _, _ -> Reply.Words("one two three") }

    private class Flaky(private val inner: AgentRepository) : AgentRepository by inner {
        var refuseSend = false
        var refuseStop: Problem? = null

        override suspend fun send(agentId: String, text: String) {
            if (refuseSend) throw RepositoryException(Problem.Invalid, "refused")
            inner.send(agentId, text)
        }

        override suspend fun stop(agentId: String) {
            refuseStop?.let { throw RepositoryException(it, "refused") }
            inner.stop(agentId)
        }
    }

    private fun TestScope.model(agentId: String = "nagisa", flaky: Boolean = false): Pair<ChatViewModel, Flaky> {
        val repo = Flaky(FakeAgentRepository(backgroundScope, FakeWorld.demo(), wordDelayMs = 100, script = oneTwoThree))
        val vm = ChatViewModel(repo, agentId)
        backgroundScope.launch { vm.state.collect {} }
        return vm to repo.also { it.refuseSend = flaky }
    }

    // --- the composer button ---

    private val working = AgentSummary("a", "A", "", AgentState.Working)
    private val resting = working.copy(state = AgentState.Resting)

    @Test
    fun theButtonSendsWhenThereAreWordsStopsWhenWorkingAndRestsOtherwise() {
        assertEquals(ComposerAction.Send, ChatUiState(agent = resting, draft = "hi").action)
        assertEquals(ComposerAction.Send, ChatUiState(agent = working, draft = "hi").action, "typing wins over stop")
        assertEquals(ComposerAction.Stop, ChatUiState(agent = working, draft = "").action)
        assertEquals(ComposerAction.Idle, ChatUiState(agent = resting, draft = "").action)
        assertEquals(ComposerAction.Idle, ChatUiState(agent = resting, draft = "   ").action, "spaces are not words")
        assertEquals(ComposerAction.Stop, ChatUiState(agent = working, draft = "  ").action)
    }

    @Test
    fun onlyWorkingMeansWorking() {
        for (state in AgentState.entries) {
            assertEquals(state == AgentState.Working, ChatUiState(agent = working.copy(state = state)).working, "$state")
        }
        assertTrue(!ChatUiState(agent = null).working)
    }

    // --- sending ---

    @Test
    fun aChatOpensOnItsAgentAndComposerBeforeRenoaAnswersAndASendFailsCleanly() = runTest {
        val inner = FakeAgentRepository(backgroundScope, FakeWorld.demo())
        val waiting =
            object : AgentRepository by inner {
                override fun conversation(agentId: String) = kotlinx.coroutines.flow.flow<live.renoa.app.agents.Conversation> { kotlinx.coroutines.awaitCancellation() }

                override suspend fun send(agentId: String, text: String) = throw RepositoryException(Problem.Unavailable, "not reached")
            }
        val vm = ChatViewModel(waiting, "nagisa").also { vm -> backgroundScope.launch { vm.state.collect {} } }
        runCurrent()
        assertEquals("Nagisa", vm.state.value.agent?.name, "the header shows")
        assertFalse(vm.state.value.loaded)
        assertFalse(vm.state.value.empty, "not offered a start: the chat is not known yet")
        vm.onDraft("hello")
        assertEquals("hello", vm.state.value.draft)
        vm.send()
        runCurrent()
        assertEquals("hello", vm.state.value.draft, "the words come back")
        assertEquals(Problem.Unavailable.ownerWords, vm.state.value.notice)
    }

    @Test
    fun aChatWhoseAgentIsGoneIsEmptyRatherThanStuck() = runTest {
        val inner = FakeAgentRepository(backgroundScope, FakeWorld.demo())
        val gone =
            object : AgentRepository by inner {
                override fun conversation(agentId: String) = kotlinx.coroutines.flow.flow<live.renoa.app.agents.Conversation> { throw RepositoryException(Problem.NotFound, "gone") }
            }
        val vm = ChatViewModel(gone, "nagisa").also { vm -> backgroundScope.launch { vm.state.collect {} } }
        runCurrent()
        assertTrue(vm.state.value.loaded)
        assertTrue(vm.state.value.messages.isEmpty())
    }

    @Test
    fun aChatWearsItsAgentsColourFromHome() = runTest {
        val seen =
            listOf("renoa", "kaisa", "nagisa").map { id ->
                val (vm, _) = model(id)
                runCurrent()
                Triple(id, vm.state.value.hue, vm.state.value.main)
            }
        assertEquals(listOf(Triple("renoa", AgentHue.Ink, true), Triple("kaisa", AgentHue.Cobalt, false), Triple("nagisa", AgentHue.Clay, false)), seen)
    }

    @Test
    fun sendingTrimsTheWordsEmptiesTheComposerAndKeepsTheMessage() = runTest {
        val (vm, _) = model()
        runCurrent()
        vm.onDraft("  hello there  ")
        vm.send()
        assertEquals("", vm.state.value.draft, "the composer empties at once")
        runCurrent()
        assertEquals(listOf(Speaker.Owner to "hello there"), vm.state.value.messages.map { it.speaker to it.text })
    }

    @Test
    fun aNewChatOpensOnItsStartAndLeavesItTheMomentTheFirstMessageIsSent() = runTest {
        val held = CompletableDeferred<Unit>()
        val inner = FakeAgentRepository(backgroundScope, seeds = emptyList())
        val repo =
            object : AgentRepository by inner {
                override suspend fun send(agentId: String, text: String) {
                    held.await()
                    inner.send(agentId, text)
                }
            }
        val tifa = inner.create(CreateAgent("op", "Tifa", "Food", emptySet())).id
        val vm = ChatViewModel(repo, tifa).also { vm -> backgroundScope.launch { vm.state.collect {} } }
        runCurrent()
        assertTrue(vm.state.value.empty)

        vm.onDraft("hello")
        vm.send()
        runCurrent()
        assertEquals(emptyList(), vm.state.value.messages, "not in the chat yet")
        assertFalse(vm.state.value.empty, "but the start does not come back meanwhile")

        held.complete(Unit)
        runCurrent()
        assertFalse(vm.state.value.sending)
        assertEquals(listOf("hello"), vm.state.value.messages.map { it.text })
    }

    @Test
    fun aRefusedFirstMessageLeavesTheChatOnItsStartWithTheWordsBack() = runTest {
        val inner = FakeAgentRepository(backgroundScope, seeds = emptyList())
        val repo = Flaky(inner).apply { refuseSend = true }
        val tifa = inner.create(CreateAgent("op", "Tifa", "Food", emptySet())).id
        val vm = ChatViewModel(repo, tifa).also { vm -> backgroundScope.launch { vm.state.collect {} } }
        runCurrent()
        vm.onDraft("hello")
        vm.send()
        runCurrent()
        assertEquals("hello", vm.state.value.draft)
        assertFalse(vm.state.value.sending)
        assertTrue(vm.state.value.empty)
    }

    @Test
    fun sendingNothingDoesNothing() = runTest {
        val (vm, _) = model()
        runCurrent()
        vm.onDraft("   ")
        vm.send()
        runCurrent()
        assertEquals(emptyList(), vm.state.value.messages)
        assertEquals("   ", vm.state.value.draft)
    }

    @Test
    fun wordsThatWereNotKeptComeBackWithANoticeNotInventedGratitude() = runTest {
        val (vm, _) = model(flaky = true)
        runCurrent()
        vm.onDraft("hello")
        vm.send()
        runCurrent()
        assertEquals("hello", vm.state.value.draft)
        assertNotNull(vm.state.value.notice)
        assertEquals(emptyList(), vm.state.value.messages)
        vm.onDraft("hello!")
        assertNull(vm.state.value.notice, "typing again clears the notice")
    }

    @Test
    fun aRefusedSendDoesNotOverwriteWhatTheOwnerTypedMeanwhile() = runTest {
        val (vm, repo) = model(flaky = true)
        runCurrent()
        vm.onDraft("first")
        repo.refuseSend = true
        vm.send()
        vm.onDraft("second")
        runCurrent()
        assertEquals("second", vm.state.value.draft)
    }

    // --- working, streaming and stop ---

    @Test
    fun theChatShowsWorkingThenWordsArrivingThenTheWholeReply() = runTest {
        val (vm, _) = model()
        runCurrent()
        assertTrue(!vm.state.value.working)

        vm.onDraft("hello")
        vm.send()
        runCurrent()
        assertTrue(vm.state.value.working)
        assertEquals(ComposerAction.Stop, vm.state.value.action)

        advanceTimeBy(750)
        assertEquals("one", vm.state.value.streaming?.text)

        settle()
        val state = vm.state.value
        assertTrue(!state.working)
        assertNull(state.streaming)
        assertEquals("one two three", state.messages.last().text)
    }

    @Test
    fun stopHaltsTheAgentAndKeepsItStopped() = runTest {
        val (vm, _) = model()
        runCurrent()
        vm.onDraft("hello")
        vm.send()
        advanceTimeBy(750)

        vm.stop()
        runCurrent()
        assertTrue(!vm.state.value.working)
        assertNull(vm.state.value.streaming)
        advanceTimeBy(60_000)
        assertEquals(1, vm.state.value.messages.size, "only the owner's message; the reply never lands")
    }

    @Test
    fun aRefusedStopSaysSoAndLeavesTheAgentWorking() = runTest {
        val (vm, repo) = model()
        runCurrent()
        vm.onDraft("hello")
        vm.send()
        runCurrent()
        repo.refuseStop = Problem.Invalid
        vm.stop()
        runCurrent()
        assertNotNull(vm.state.value.notice)
        assertTrue(vm.state.value.working)
    }

    @Test
    fun aStopThatMayHaveLandedDoesNotClaimItDidNot() = runTest {
        val (vm, repo) = model()
        runCurrent()
        vm.onDraft("hello")
        vm.send()
        runCurrent()
        repo.refuseStop = Problem.Unavailable
        vm.stop()
        runCurrent()
        assertEquals("It may not have stopped. Try again.", vm.state.value.notice)
    }

    @Test
    fun anAgentThatIsGoneGivesAnEmptyChatNotACrash() = runTest {
        val (vm, _) = model(agentId = "nobody")
        runCurrent()
        assertNull(vm.state.value.agent)
        assertEquals(emptyList(), vm.state.value.messages)
    }
}

/** Lets every pending delay run. `advanceUntilIdle` would skip them: the fake's work lives in the background scope. */
@OptIn(ExperimentalCoroutinesApi::class)
private suspend fun TestScope.settle() {
    advanceTimeBy(60_000)
    runCurrent()
}
