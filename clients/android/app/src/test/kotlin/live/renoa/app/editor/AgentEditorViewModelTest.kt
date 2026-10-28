package live.renoa.app.editor

import kotlinx.coroutines.ExperimentalCoroutinesApi
import kotlinx.coroutines.launch
import kotlinx.coroutines.test.TestScope
import kotlinx.coroutines.test.runCurrent
import kotlinx.coroutines.test.runTest
import live.renoa.app.MainDispatcherRule
import live.renoa.app.agents.AbilitySwitch
import live.renoa.app.agents.AgentProfile
import live.renoa.app.agents.AgentRepository
import live.renoa.app.agents.CreateAgent
import live.renoa.app.agents.EditProfile
import live.renoa.app.agents.FakeAgentRepository
import live.renoa.app.agents.FakeWorld
import live.renoa.app.agents.Problem
import live.renoa.app.agents.RepositoryException
import org.junit.Rule
import kotlin.test.Test
import kotlin.test.assertEquals
import kotlin.test.assertNotEquals
import kotlin.test.assertNotNull
import kotlin.test.assertNull
import kotlin.test.assertTrue

@OptIn(ExperimentalCoroutinesApi::class)
class AgentEditorViewModelTest {
    @get:Rule val main = MainDispatcherRule()

    /** Counts what reaches the repository and can refuse ability changes. */
    private class Recording(private val inner: AgentRepository) : AgentRepository by inner {
        val setAbilityCalls = mutableListOf<Triple<String, String, Boolean>>()
        val creates = mutableListOf<CreateAgent>()
        val edits = mutableListOf<EditProfile>()
        var refuseAbility = false

        /** The next create takes effect but its answer is lost on the way back. */
        var loseNextCreateAnswer = false

        override suspend fun setAbility(agentId: String, abilityId: String, lit: Boolean): AbilitySwitch {
            setAbilityCalls += Triple(agentId, abilityId, lit)
            if (refuseAbility) throw RepositoryException(Problem.Invalid, "refused")
            return inner.setAbility(agentId, abilityId, lit)
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

        override suspend fun edit(agentId: String, edit: EditProfile) = inner.edit(agentId, edit.also { edits += it })
    }

    private fun TestScope.repo() = Recording(FakeAgentRepository(backgroundScope, FakeWorld.demo()))

    private fun TestScope.creating(repo: Recording): AgentEditorViewModel =
        AgentEditorViewModel(repo, EditorMode.Creating, operationId = "op-1").also { vm -> backgroundScope.launch { vm.state.collect {} } }

    private fun TestScope.editing(repo: Recording, id: String = "tifa"): AgentEditorViewModel =
        AgentEditorViewModel(repo, EditorMode.Editing(id)).also { vm -> backgroundScope.launch { vm.state.collect {} } }

    // --- creating ---

    @Test
    fun theCreateScreenStartsWithTheStarterSwitchesAndCannotBeSavedWithoutAName() = runTest {
        val vm = creating(repo())
        runCurrent()
        val state = vm.state.value
        assertTrue(state.loaded)
        assertEquals(setOf("calendar", "notes", "search"), state.abilities.filter { it.lit }.map { it.id }.toSet())
        assertTrue(!state.canSave)
        vm.setName("Mira")
        assertTrue(vm.state.value.canSave)
        vm.setName("   ")
        assertTrue(!vm.state.value.canSave, "a name of spaces is no name")
    }

    @Test
    fun savingMakesTheAgentWithExactlyTheLitSwitchesAndThenFinishes() = runTest {
        val repo = repo()
        val vm = creating(repo)
        runCurrent()
        vm.setName("  Mira ")
        vm.setPurpose(" Keeps the plants alive. ")
        vm.toggle("music")
        vm.toggle("calendar")
        vm.save()
        runCurrent()

        val request = repo.creates.single()
        assertEquals("Mira", request.name)
        assertEquals("Keeps the plants alive.", request.purpose)
        assertEquals(setOf("notes", "search", "music"), request.litAbilities)
        assertEquals("Mira", repo.agents.value.single { it.id == vm.state.value.finishedAgentId }.name)
    }

    @Test
    fun tappingBringToLifeTwiceMakesOneAgent() = runTest {
        val repo = repo()
        val before = repo.agents.value.size
        val vm = creating(repo)
        runCurrent()
        vm.setName("Mira")
        vm.save()
        vm.save()
        runCurrent()
        assertEquals(before + 1, repo.agents.value.size)
    }

    @Test
    fun aSaveWhoseAnswerWasLostCanBeRetriedAndStillMakesOneAgent() = runTest {
        val repo = repo()
        val before = repo.agents.value.size
        val vm = creating(repo)
        runCurrent()
        vm.setName("Mira")
        repo.loseNextCreateAnswer = true

        vm.save()
        runCurrent()
        assertNull(vm.state.value.finishedAgentId)
        assertEquals(Problem.Unavailable.ownerWords, vm.state.value.notice)
        assertTrue(!vm.state.value.saving, "the owner can try again")

        vm.save()
        runCurrent()
        assertNotNull(vm.state.value.finishedAgentId)
        assertEquals(1, repo.creates.map { it.operationId }.toSet().size, "every attempt must carry one operation id")
        assertEquals(2, repo.creates.size)
        assertEquals(before + 1, repo.agents.value.size)
    }

    @Test
    fun textThatCouldNotBeKeptIsCaughtBeforeTheRoundTrip() = runTest {
        val repo = repo()
        val vm = creating(repo)
        runCurrent()
        vm.setName("a".repeat(600))
        assertTrue(!vm.state.value.canSave, "caught before the round trip")
        vm.setName("Mira")
        vm.setPurpose("bell\u0007")
        assertTrue(!vm.state.value.canSave)
        vm.setPurpose("fine")
        vm.save()
        runCurrent()
        assertNotNull(vm.state.value.finishedAgentId)
    }

    @Test
    fun whileCreatingMachineAbilitiesCanBeChosenButNotTheOnesNotBuiltYet() = runTest {
        val vm = creating(repo())
        runCurrent()
        vm.toggle("files")
        assertTrue(vm.state.value.abilities.first { it.id == "files" }.lit)
        vm.toggle("wallet")
        val wallet = vm.state.value.abilities.first { it.id == "wallet" }
        assertTrue(!wallet.lit)
        assertNotNull(vm.state.value.notice)
    }

    // --- editing ---

    @Test
    fun editingLoadsTheAgentAndKeepsDoneDisabledUntilItIsValid() = runTest {
        val vm = editing(repo())
        runCurrent()
        assertEquals("Tifa", vm.state.value.name)
        assertEquals("Your training partner. Pushes hard, cheers louder.", vm.state.value.purpose)
        assertTrue(vm.state.value.canSave)
        vm.setName("")
        assertTrue(!vm.state.value.canSave)
    }

    @Test
    fun anEverydaySwitchTakesEffectAtOnceAndIsKeptByTheRepository() = runTest {
        val repo = repo()
        val vm = editing(repo)
        runCurrent()
        vm.toggle("maps")
        assertTrue(vm.state.value.abilities.first { it.id == "maps" }.lit, "the tile lights at once")
        runCurrent()
        assertEquals(listOf(Triple("tifa", "maps", true)), repo.setAbilityCalls)
        assertTrue(repo.profile("tifa").abilities.first { it.id == "maps" }.lit)
    }

    @Test
    fun aSwitchTheRepositoryRefusesGoesBackAndSaysSo() = runTest {
        val repo = repo()
        val vm = editing(repo)
        runCurrent()
        repo.refuseAbility = true
        vm.toggle("maps")
        runCurrent()
        assertTrue(!vm.state.value.abilities.first { it.id == "maps" }.lit)
        assertNotNull(vm.state.value.notice)
    }

    @Test
    fun afterTheAgentIsMadeMachineAbilitiesAndUnbuiltOnesDoNotEvenReachTheRepository() = runTest {
        val repo = repo()
        val vm = editing(repo)
        runCurrent()
        val before = vm.state.value.abilities
        for (id in listOf("phone", "files", "commands", "wallet")) {
            vm.toggle(id)
            runCurrent()
            assertEquals(before, vm.state.value.abilities, "$id changed")
            assertNotNull(vm.state.value.notice, "$id should explain itself")
        }
        assertEquals(emptyList(), repo.setAbilityCalls)
    }

    @Test
    fun tappingAnUnknownTileDoesNothing() = runTest {
        val repo = repo()
        val vm = editing(repo)
        runCurrent()
        val before = vm.state.value
        vm.toggle("teleport")
        assertEquals(before, vm.state.value)
    }

    @Test
    fun doneSendsWhatTheOwnerSawAndWhatItReplacesAndThenFinishes() = runTest {
        val repo = repo()
        val vm = editing(repo)
        runCurrent()
        vm.setName("Tifa Lockhart")
        vm.save()
        runCurrent()
        assertEquals(
            listOf(EditProfile("Tifa", "Tifa Lockhart", "Your training partner. Pushes hard, cheers louder.", "Your training partner. Pushes hard, cheers louder.")),
            repo.edits,
        )
        assertEquals("tifa", vm.state.value.finishedAgentId)
    }

    @Test
    fun doneWithNothingChangedSendsNothing() = runTest {
        val repo = repo()
        val vm = editing(repo)
        runCurrent()
        vm.save()
        runCurrent()
        assertEquals(emptyList(), repo.edits)
        assertEquals("tifa", vm.state.value.finishedAgentId)
    }

    @Test
    fun anEditMadeStaleByAnotherDeviceSaysSoAndDoesNotFinish() = runTest {
        val repo = repo()
        val vm = editing(repo)
        runCurrent()
        // Another app renames the agent while this screen is open.
        repo.edit("tifa", EditProfile("Tifa", "Teef", "Your training partner. Pushes hard, cheers louder.", "Your training partner. Pushes hard, cheers louder."))
        vm.setPurpose("Something new")
        vm.save()
        runCurrent()
        assertNull(vm.state.value.finishedAgentId)
        assertNotNull(vm.state.value.notice)
        assertTrue(!vm.state.value.saving)
        assertNotEquals("Something new", repo.profile("tifa").purpose)
    }

    @Test
    fun anAgentThatCannotBeOpenedIsNotEditable() = runTest {
        val vm = editing(repo(), id = "nobody")
        runCurrent()
        assertTrue(!vm.state.value.loaded)
        assertTrue(!vm.state.value.canSave)
        assertNotNull(vm.state.value.notice)
    }
}
