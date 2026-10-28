package live.renoa.app.chat

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
import live.renoa.app.agents.FakeAgentRepository
import live.renoa.app.agents.FakeWorld
import live.renoa.app.agents.Problem
import live.renoa.app.agents.Reply
import live.renoa.app.agents.ReplyScript
import live.renoa.app.agents.RepositoryException
import live.renoa.app.agents.Speaker
import org.junit.Rule
import kotlin.test.Test
import kotlin.test.assertEquals
import kotlin.test.assertNotNull
import kotlin.test.assertNull
import kotlin.test.assertTrue

@OptIn(ExperimentalCoroutinesApi::class)
class ChatViewModelTest {
    @get:Rule val main = MainDispatcherRule()

    private val oneTwoThree = ReplyScript { _, _ -> Reply.Words("one two three") }

    private class Flaky(private val inner: AgentRepository) : AgentRepository by inner {
        var refuseSend = false
        var refuseStop = false

        override suspend fun send(agentId: String, text: String) {
            if (refuseSend) throw RepositoryException(Problem.Invalid, "refused")
            inner.send(agentId, text)
        }

        override suspend fun stop(agentId: String) {
            if (refuseStop) throw RepositoryException(Problem.Invalid, "refused")
            inner.stop(agentId)
        }
    }

    private fun TestScope.model(agentId: String = "tifa", flaky: Boolean = false): Pair<ChatViewModel, Flaky> {
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
        assertEquals("one", vm.state.value.streaming)

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
        repo.refuseStop = true
        vm.stop()
        runCurrent()
        assertNotNull(vm.state.value.notice)
        assertTrue(vm.state.value.working)
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
