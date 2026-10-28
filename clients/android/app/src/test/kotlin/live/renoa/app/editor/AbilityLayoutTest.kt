package live.renoa.app.editor

import androidx.compose.ui.graphics.Color
import live.renoa.app.agents.AbilityKind
import live.renoa.app.agents.AbilityIcon
import live.renoa.app.agents.AbilitySwitch
import live.renoa.app.agents.FakeWorld
import kotlin.test.Test
import kotlin.test.assertEquals
import kotlin.test.assertTrue

class AbilityLayoutTest {
    @Test
    fun tilesFillColumnsOfThreeAndEveryOtherColumnHangsHalfATileLower() {
        val spots = (0 until 15).map(AbilityLayout::spot)
        assertEquals(listOf(0, 0, 0, 1, 1, 1), spots.take(6).map { it.column })
        assertEquals(listOf(0, 1, 2, 0, 1, 2), spots.take(6).map { it.row })
        assertEquals(0f, spots[0].y)
        assertEquals(AbilityLayout.ROW_STEP / 2, spots[3].y)
        assertEquals(spots[0].y, spots[6].y, "the third column lines up with the first")
        assertEquals(5, AbilityLayout.columns(15))
        assertEquals(4, AbilityLayout.columns(10))
    }

    @Test
    fun noTwoTilesShareASpotAndAllFitInsideTheGrid() {
        val count = 31
        val spots = (0 until count).map(AbilityLayout::spot)
        assertEquals(count, spots.map { it.x to it.y }.toSet().size)
        assertTrue(spots.all { it.x + AbilityLayout.TILE <= AbilityLayout.width(count) + 0.01f })
        assertTrue(spots.all { it.y + AbilityLayout.TILE <= AbilityLayout.HEIGHT + 0.01f }, "a tile hangs below the grid")
    }

    @Test
    fun theStarterTilesFillFiveFullColumns() {
        assertEquals(15, FakeWorld.starterAbilities().size)
        assertEquals(5, AbilityLayout.columns(FakeWorld.starterAbilities().size))
    }

    @Test
    fun inkReadsOnBothPaleAndDeepTiles() {
        assertEquals(Color.White, inkOn(Color(0xFFC9485B)))
        assertEquals(Color(0xFF1A1204), inkOn(Color(0xFFF2B8C6)))
    }

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
