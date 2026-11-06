package live.renoa.app.editor

import live.renoa.app.agents.AbilityIcon
import live.renoa.app.agents.AbilityKind
import live.renoa.app.agents.AbilitySwitch
import live.renoa.app.agents.FakeWorld
import kotlin.test.Test
import kotlin.test.assertEquals
import kotlin.test.assertTrue

class AbilityLayoutTest {
    @Test
    fun theStarterAbilitiesHaveUniqueIdsAndTheKindsTheDesignNames() {
        val all = FakeWorld.starterAbilities()
        assertEquals(all.size, all.map { it.id }.toSet().size)
        assertEquals(setOf("phone", "files", "commands"), all.filter { it.kind == AbilityKind.Machine }.map { it.id }.toSet())
        assertEquals(setOf("wallet"), all.filter { it.kind == AbilityKind.Soon }.map { it.id }.toSet())
        assertEquals(setOf("calendar", "search", "notes"), all.filter { it.lit }.map { it.id }.toSet())
    }

    @Test
    fun whoMaySwitchWhatIsDecidedByKindAloneAndEveryKindIsCovered() {
        for (kind in AbilityKind.entries) {
            val ability = AbilitySwitch("x", "X", AbilityIcon.More, kind, lit = false)
            val expectedAfter = kind == AbilityKind.Everyday
            val expectedWhileCreating = kind != AbilityKind.Soon
            assertEquals(expectedAfter, ability.canSwitch(creating = false), "$kind after creation")
            assertEquals(expectedWhileCreating, ability.canSwitch(creating = true), "$kind while creating")
        }
    }

    @Test
    fun everyAbilityIconHasPathDataAndNoTwoShareOne() {
        val paths = AbilityIcon.entries.map { it.path }
        assertTrue(paths.all { it.isNotBlank() })
        assertEquals(paths.size, paths.toSet().size)
    }
}
