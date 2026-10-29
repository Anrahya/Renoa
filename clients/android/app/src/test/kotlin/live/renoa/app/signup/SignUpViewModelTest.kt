package live.renoa.app.signup

import kotlinx.coroutines.ExperimentalCoroutinesApi
import kotlinx.coroutines.launch
import kotlinx.coroutines.test.TestScope
import kotlinx.coroutines.test.runCurrent
import kotlinx.coroutines.test.runTest
import live.renoa.app.MainDispatcherRule
import live.renoa.app.agents.AgentProfile
import live.renoa.app.agents.AgentRepository
import live.renoa.app.agents.CreateAgent
import live.renoa.app.agents.FakeAgentRepository
import live.renoa.app.agents.OwnerProfile
import live.renoa.app.agents.Problem
import live.renoa.app.agents.RepositoryException
import org.junit.Rule
import kotlin.test.Test
import kotlin.test.assertEquals
import kotlin.test.assertNull
import kotlin.test.assertTrue

@OptIn(ExperimentalCoroutinesApi::class)
class SignUpViewModelTest {
    @get:Rule val main = MainDispatcherRule()

    /** Counts what reaches the repository, and can lose answers or fail the way a dropped connection does. */
    private class Recording(private val inner: AgentRepository) : AgentRepository by inner {
        val creates = mutableListOf<CreateAgent>()
        var loseNextCreateAnswer = false
        var unreachable = false

        /** Something else edits the profile just before the app's next save. */
        var editFirst: String? = null

        override suspend fun ownerProfile(): OwnerProfile {
            if (unreachable) throw RepositoryException(Problem.Unavailable, "no connection")
            return inner.ownerProfile()
        }

        override suspend fun saveOwnerProfile(expectedRevision: String, content: String): OwnerProfile {
            editFirst?.let { other ->
                editFirst = null
                inner.saveOwnerProfile(inner.ownerProfile().revision, other)
            }
            return inner.saveOwnerProfile(expectedRevision, content)
        }

        override suspend fun create(request: CreateAgent): AgentProfile {
            creates += request
            val made = inner.create(request)
            if (loseNextCreateAnswer) {
                loseNextCreateAnswer = false
                throw RepositoryException(Problem.Unavailable, "no answer")
            }
            return made
        }
    }

    private class Memory : SignUpMemory {
        override var operationId: String? = null
    }

    private fun TestScope.repo() = Recording(FakeAgentRepository(backgroundScope, seeds = emptyList()))

    private fun TestScope.signUp(repo: Recording, memory: SignUpMemory = Memory()): SignUpViewModel =
        SignUpViewModel(repo, memory).also { vm -> backgroundScope.launch { vm.state.collect {} } }

    @Test
    fun aNewOwnerGivesTheirNameAndDayThenMeetsRenoaWhoIsMadeOnce() = runTest {
        val repo = repo()
        val memory = Memory()
        val vm = signUp(repo, memory)
        runCurrent()
        assertEquals(SignUpStep.Name, vm.state.value.step)
        assertTrue(!vm.state.value.canName)

        vm.setName("  Yash ")
        vm.saveName()
        runCurrent()
        assertEquals(SignUpStep.Day, vm.state.value.step)
        assertEquals("Yash", vm.state.value.name)

        vm.setDay("Up at 7")
        vm.togglePhrase("I cook at home")
        assertEquals("Up at 7. I cook at home.", vm.state.value.day)
        assertEquals(setOf("I cook at home"), vm.state.value.phrases)
        vm.saveDay()
        runCurrent()

        assertEquals("Name: Yash\nA usual day: Up at 7. I cook at home.\n", repo.ownerProfile().content)
        assertEquals(SignUpStep.Meet, vm.state.value.step)
        assertTrue(vm.state.value.ready)
        assertEquals(listOf(RENOA), repo.agents.value.map { it.name })
        assertEquals(RENOA_PURPOSE, repo.agents.value.single().purpose)
        assertEquals(repo.creates.single().operationId, memory.operationId, "kept until sign-up is over")
        vm.finish()
        assertNull(memory.operationId)
    }

    @Test
    fun skippingTheDaySavesNothingForIt() = runTest {
        val repo = repo()
        val vm = signUp(repo)
        runCurrent()
        vm.setName("Yash")
        vm.saveName()
        runCurrent()
        vm.setDay("Half written")
        vm.saveDay(skip = true)
        runCurrent()
        assertEquals("Name: Yash\n", repo.ownerProfile().content)
        assertTrue(vm.state.value.ready)
    }

    @Test
    fun aSignUpStoppedPartwayResumesAtTheFirstScreenWithNothingSaved() = runTest {
        val repo = repo()
        val first = repo.ownerProfile()
        repo.saveOwnerProfile(first.revision, "Lives in Kolkata.\n")
        assertEquals(SignUpStep.Name, signUp(repo).also { runCurrent() }.state.value.step, "a profile without a name")

        repo.saveOwnerProfile(repo.ownerProfile().revision, ProfileLines.withName(repo.ownerProfile().content, "Yash"))
        val day = signUp(repo).also { runCurrent() }
        assertEquals(SignUpStep.Day, day.state.value.step)
        assertEquals("Yash", day.state.value.name)

        repo.saveOwnerProfile(repo.ownerProfile().revision, ProfileLines.withDay(repo.ownerProfile().content, "Busy"))
        val meet = signUp(repo).also { runCurrent() }
        assertEquals(SignUpStep.Meet, meet.state.value.step)
        assertTrue(meet.state.value.ready)
        assertEquals("Name: Yash\nA usual day: Busy\nLives in Kolkata.\n", repo.ownerProfile().content, "what was there stays")
    }

    @Test
    fun renoaIsMadeOnceEvenWhenTheAnswerIsLostAndTheAppRestarts() = runTest {
        val repo = repo()
        repo.saveOwnerProfile(repo.ownerProfile().revision, "Name: Yash\nA usual day: Busy\n")
        val memory = Memory()
        repo.loseNextCreateAnswer = true
        val before = signUp(repo, memory)
        runCurrent()
        assertEquals(Problem.Unavailable.ownerWords, before.state.value.notice)
        assertTrue(!before.state.value.ready)

        // Trying again finds the Renoa the lost answer was about.
        before.meet()
        runCurrent()
        assertTrue(before.state.value.ready)
        assertEquals(1, repo.creates.map { it.operationId }.toSet().size)
        assertEquals(1, repo.agents.value.size)

        // Opened again after being killed, sign-up sees she is there and goes Home.
        val after = signUp(repo, memory)
        runCurrent()
        assertTrue(after.state.value.done)
        assertEquals(2, repo.creates.size)
        assertEquals(1, repo.agents.value.size)
    }

    @Test
    fun notReachingRenoaSaysSoAndTryingAgainPicksUpWhereItWas() = runTest {
        val repo = repo()
        repo.unreachable = true
        val vm = signUp(repo)
        runCurrent()
        assertEquals(SignUpStep.Checking, vm.state.value.step)
        assertEquals(Problem.Unavailable.ownerWords, vm.state.value.notice)

        repo.unreachable = false
        vm.resume()
        runCurrent()
        assertEquals(SignUpStep.Name, vm.state.value.step)
        assertNull(vm.state.value.notice)
    }

    @Test
    fun aProfileEditedElsewhereMeanwhileKeepsBothChanges() = runTest {
        val repo = repo()
        val vm = signUp(repo)
        runCurrent()
        repo.editFirst = "Likes dal.\n"
        vm.setName("Yash")
        vm.saveName()
        runCurrent()
        assertEquals(SignUpStep.Day, vm.state.value.step)
        assertEquals("Name: Yash\nLikes dal.\n", repo.ownerProfile().content)
    }

    @Test
    fun anOwnerWhoAlreadyHasAnAgentGoesHomeAndNothingIsMade() = runTest {
        val repo = repo()
        repo.create(CreateAgent("elsewhere", "Tifa", "Food", emptySet()))
        repo.creates.clear()
        val memory = Memory().apply { operationId = "stale" }
        val vm = signUp(repo, memory)
        runCurrent()
        assertTrue(vm.state.value.done)
        assertTrue(repo.creates.isEmpty())
        assertNull(memory.operationId, "a stale id from an earlier sign-up is let go")
    }

    @Test
    fun backFromTheDayReturnsToTheName() = runTest {
        val vm = signUp(repo())
        runCurrent()
        vm.setName("Yash")
        vm.saveName()
        runCurrent()
        vm.back()
        assertEquals(SignUpStep.Name, vm.state.value.step)
        assertEquals("Yash", vm.state.value.name)
    }

    @Test
    fun aTappedPhraseIsASentenceOfItsOwnAndTheOwnersWordsStay() {
        assertTrue(!DayWords.has("I work outside.", "I work out"))
        assertEquals("I work outside. I work out.", DayWords.toggle("I work outside.", "I work out"))
        assertEquals("I have kids and a dog. I have kids.", DayWords.toggle("I have kids and a dog.", "I have kids"))
        assertTrue(DayWords.has("Up at 7. i cook at home.", "I cook at home"), "whatever the case")
        assertEquals("Up at 7.", DayWords.toggle("Up at 7. i cook at home.", "I cook at home"))
        assertEquals("I cook at home.", DayWords.toggle("", "I cook at home"))
    }

    @Test
    fun aChangeTouchesOneLineAndLeavesTheRestByteForByte() {
        assertEquals("# About me\r\nName: Yash\r\nLikes tea  \r\n", ProfileLines.withName("# About me\r\nLikes tea  \r\n", "Yash"), "under the heading, with its line endings")
        assertEquals("---\ntitle: x\n---\nName: Yash\nbody", ProfileLines.withName("---\ntitle: x\n---\nbody", "Yash"), "under front matter")
        assertEquals("Likes tea  \nName: Asha\nName: Rex\n", ProfileLines.withName("Likes tea  \nName: Yash\nName: Rex\n", "Asha"), "only the first, in place")
    }

    @Test
    fun theLinesAreOneLineEachAndReplaceThemselves() {
        assertEquals("Name: Yash\n", ProfileLines.withName("", "Yash"))
        assertEquals("Name: Y\nA usual day: Up at 7. Out till 6.\n", ProfileLines.withDay("Name: Y\n", "Up at 7.\n  Out till 6."))
        assertEquals("A usual day: Busy\nOld\n", ProfileLines.withDay("Old\n", "Busy"), "with no name line, at the top")
        assertEquals("Name: Asha\nA usual day: Busy\n", ProfileLines.withName("Name: Yash\nA usual day: Busy\n", "Asha"))
        assertEquals("Yash", ProfileLines.name("Likes dal.\nName:  Yash \n"))
        assertNull(ProfileLines.name("Name:\n"), "an empty value is no name")
    }
}
