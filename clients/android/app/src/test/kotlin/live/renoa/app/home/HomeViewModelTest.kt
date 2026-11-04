package live.renoa.app.home

import kotlinx.coroutines.CompletableDeferred
import kotlinx.coroutines.ExperimentalCoroutinesApi
import kotlinx.coroutines.launch
import kotlinx.coroutines.test.advanceTimeBy
import kotlinx.coroutines.test.runCurrent
import kotlinx.coroutines.test.runTest
import live.renoa.app.MainDispatcherRule
import live.renoa.app.agents.AgentRepository
import live.renoa.app.agents.FakeAgentRepository
import live.renoa.app.agents.FakeWorld
import live.renoa.app.agents.Problem
import live.renoa.app.agents.RepositoryException
import org.junit.Rule
import kotlin.test.Test
import kotlin.test.assertEquals
import kotlin.test.assertFalse
import kotlin.test.assertNotNull
import kotlin.test.assertNull
import kotlin.test.assertTrue

@OptIn(ExperimentalCoroutinesApi::class)
class HomeViewModelTest {
    @get:Rule val main = MainDispatcherRule()

    /** Holds each routine switch until [release] completes, then refuses it with [refusal] if set. */
    private class Held(private val inner: AgentRepository) : AgentRepository by inner {
        var release = CompletableDeferred<Unit>()
        var refusal: Problem? = null
        val operations = mutableListOf<String>()

        override suspend fun setRoutine(routineId: String, on: Boolean, operationId: String) {
            operations += operationId
            release.await()
            refusal?.let { throw RepositoryException(it, "refused") }
            inner.setRoutine(routineId, on, operationId)
        }
    }

    private fun kotlinx.coroutines.test.TestScope.model(): Pair<HomeViewModel, Held> {
        val repo = Held(FakeAgentRepository(backgroundScope, FakeWorld.demo()))
        val vm = HomeViewModel(repo)
        backgroundScope.launch { vm.state.collect {} }
        return vm to repo
    }

    @Test
    fun aSwitchShowsWhatWasAskedUntilRenoaAnswersAndEachTapIsItsOwnChange() = runTest {
        val (vm, repo) = model()
        runCurrent()
        val lunch = vm.state.value.routines.first { it.id == "lunch" }
        assertFalse(lunch.on)

        vm.switch("lunch", on = true)
        runCurrent()
        assertEquals(mapOf("lunch" to true), vm.state.value.switching)
        assertTrue(vm.state.value.isOn(lunch), "shown on while Renoa has not answered")

        repo.release.complete(Unit)
        runCurrent()
        assertEquals(emptyMap(), vm.state.value.switching)
        assertTrue(vm.state.value.routines.first { it.id == "lunch" }.on)

        vm.switch("lunch", on = false)
        runCurrent()
        assertEquals(2, repo.operations.toSet().size, "a second tap is a second change, with its own operation id")
    }

    @Test
    fun aRefusedSwitchGoesBackAndSaysWhyAndClosingThePageClearsIt() = runTest {
        val (vm, repo) = model()
        runCurrent()
        vm.open(HomePage.Routines)
        val words =
            mapOf(
                Problem.Conflict to "That routine was changed somewhere else. Check it and try again.",
                Problem.NotFound to "That routine is gone.",
                Problem.Invalid to "That routine could not be changed. Try again.",
                Problem.Unavailable to Problem.Unavailable.ownerWords,
            )
        for ((problem, said) in words) {
            repo.release = CompletableDeferred(Unit)
            repo.refusal = problem
            vm.switch("lunch", on = true)
            runCurrent()
            assertEquals(said, vm.state.value.notice, "$problem")
            assertEquals(emptyMap(), vm.state.value.switching)
            assertFalse(vm.state.value.isOn(vm.state.value.routines.first { it.id == "lunch" }), "back to what Renoa last said")
        }

        vm.close()
        assertNull(vm.state.value.notice, "it does not follow the owner back to Home")
    }

    @Test
    fun anOwnerWithNoAgentIsSentToSignUpAndOneWithAgentsIsGreetedByName() = runTest {
        val empty = HomeViewModel(FakeAgentRepository(backgroundScope, seeds = emptyList()))
        backgroundScope.launch { empty.state.collect {} }
        runCurrent()
        assertTrue(empty.state.value.signUp)

        val repo = FakeAgentRepository(backgroundScope, FakeWorld.demo())
        repo.saveOwnerProfile(repo.ownerProfile().revision, "Name: Yash\n")
        val known = HomeViewModel(repo)
        backgroundScope.launch { known.state.collect {} }
        runCurrent()
        assertFalse(known.state.value.signUp)
        assertEquals("Yash", known.state.value.ownerName)
    }

    @Test
    fun notReachingRenoaOrABusyHostNeverSendsAnyoneToSignUpAndItAsksAgain() = runTest {
        // A Host with too many calls waiting answers with a conflict.
        for (problem in listOf(Problem.Unavailable, Problem.Conflict)) {
            var reachable = false
            val inner = FakeAgentRepository(backgroundScope, seeds = emptyList())
            val repo =
                object : AgentRepository by inner {
                    override suspend fun hasAgents(): Boolean = if (reachable) inner.hasAgents() else throw RepositoryException(problem, "not now")
                }
            val vm = HomeViewModel(repo)
            backgroundScope.launch { vm.state.collect {} }
            runCurrent()
            assertFalse(vm.state.value.signUp, "$problem")
            reachable = true
            advanceTimeBy(2_001)
            runCurrent()
            assertTrue(vm.state.value.signUp, "$problem")
        }
    }

    @Test
    fun openingARowPutsItsNotesAwayAndAFailedRoutineRunsAgainFromHome() = runTest {
        val (vm, _) = model()
        runCurrent()
        val optimus = vm.state.value.talks.talking.first { it.agent.id == "optimus" }
        vm.seen(assertNotNull(optimus.note).id)
        runCurrent()
        assertTrue(vm.state.value.feed.notes.none { it.id == "projects" })

        val nagisa = vm.state.value.talks.quiet.first { it.agent.id == "nagisa" }
        vm.runAgain(assertNotNull(nagisa.problem).id)
        runCurrent()
        assertEquals(emptyList(), vm.state.value.feed.notes)
        assertNull(vm.state.value.notice)

        // Already gone, as when it ran again elsewhere: nothing to tell the owner.
        vm.runAgain("prices")
        runCurrent()
        assertNull(vm.state.value.notice)
    }

    @Test
    fun aSecondTapWhileTheAnswerGoesSendsNothing() = runTest {
        val inner = FakeAgentRepository(backgroundScope, FakeWorld.demo())
        val release = CompletableDeferred<Unit>()
        val answers = mutableListOf<String>()
        val vm =
            HomeViewModel(
                object : AgentRepository by inner {
                    override suspend fun answer(agentId: String, choice: String) {
                        answers += choice
                        release.await()
                        inner.answer(agentId, choice)
                    }
                },
            )
        backgroundScope.launch { vm.state.collect {} }
        vm.answer("tifa", "Dal and a salad")
        vm.answer("tifa", "Dal and a salad")
        runCurrent()
        release.complete(Unit)
        runCurrent()
        assertEquals(listOf("Dal and a salad"), answers)
        vm.answer("tifa", "Not sure yet")
        runCurrent()
        assertEquals(2, answers.size, "once it went, the next answer goes")
    }

    @Test
    fun aRunAgainTheHostRefusesSaysSo() = runTest {
        val inner = FakeAgentRepository(backgroundScope, FakeWorld.demo())
        val vm =
            HomeViewModel(
                object : AgentRepository by inner {
                    override suspend fun runAgain(noteId: String) = throw RepositoryException(Problem.Invalid, "refused")
                },
            )
        backgroundScope.launch { vm.state.collect {} }
        vm.runAgain("prices")
        runCurrent()
        assertEquals("That routine could not run again. Try again.", vm.state.value.notice)
    }
}
