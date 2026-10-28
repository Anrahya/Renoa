package live.renoa.app.theme

import androidx.compose.ui.graphics.Color
import androidx.compose.ui.graphics.luminance
import live.renoa.app.badge.AgentLook
import kotlin.reflect.full.memberProperties
import kotlin.test.Test
import kotlin.test.assertEquals
import kotlin.test.assertNotEquals
import kotlin.test.assertTrue

class ThemeTest {
    private fun contrast(a: Color, b: Color): Float {
        val hi = maxOf(a.luminance(), b.luminance())
        val lo = minOf(a.luminance(), b.luminance())
        return (hi + 0.05f) / (lo + 0.05f)
    }

    @Test
    fun theThreeThemesAreMidnightEmberAndCrimsonWithMidnightFirst() {
        assertEquals(listOf("Midnight", "Ember", "Crimson"), ThemeName.entries.map { it.name })
    }

    @Test
    fun everyThemeSetsEveryTokenToARealColour() {
        val tokens = RenoaColors::class.memberProperties
        assertTrue(tokens.size >= 17, "expected the full token set, found ${tokens.size}")
        for (theme in ThemeName.entries) {
            for (token in tokens) {
                val value = token.get(theme.colors) as Color
                assertNotEquals(Color.Unspecified, value, "${theme.name} leaves ${token.name} unset")
                assertEquals(1f, value.alpha, "${theme.name} ${token.name} should be opaque")
            }
        }
    }

    @Test
    fun noTwoThemesShareAGroundOrASignature() {
        val colors = ThemeName.entries.map { it.colors }
        assertEquals(colors.size, colors.map { it.background }.toSet().size)
        assertEquals(colors.size, colors.map { it.signature }.toSet().size)
    }

    @Test
    fun textHoldsFourAndAHalfToOneOnTheSurfacesItSitsOn() {
        for (theme in ThemeName.entries) {
            val c = theme.colors
            val pairs =
                mapOf(
                    "foreground on background" to (c.foreground to c.background),
                    "foreground on card" to (c.foreground to c.card),
                    "foreground on secondary" to (c.foreground to c.secondary),
                    "soft on background" to (c.foregroundSoft to c.background),
                    "soft on card" to (c.foregroundSoft to c.card),
                    "muted on background" to (c.mutedForeground to c.background),
                    "muted on card" to (c.mutedForeground to c.card),
                    "muted on muted" to (c.mutedForeground to c.muted),
                    "on primary" to (c.primaryForeground to c.primary),
                    "on signature" to (c.onSignature to c.signature),
                    "failed on background" to (c.stateFailed to c.background),
                )
            for ((name, pair) in pairs) {
                val ratio = contrast(pair.first, pair.second)
                assertTrue(ratio >= 4.5f, "${theme.name}: $name is only %.2f to 1".format(ratio))
            }
        }
    }

    @Test
    fun aControlEdgeHoldsThreeToOneOnTheGround() {
        for (theme in ThemeName.entries) {
            val ratio = contrast(theme.colors.input, theme.colors.background)
            assertTrue(ratio >= 3f, "${theme.name}: the input edge is only %.2f to 1".format(ratio))
        }
    }

    @Test
    fun nextCyclesThroughEveryThemeAndComesBack() {
        var theme = ThemeName.Midnight
        val seen = mutableListOf<ThemeName>()
        repeat(ThemeName.entries.size) {
            seen += theme
            theme = theme.next()
        }
        assertEquals(ThemeName.entries.toList(), seen)
        assertEquals(ThemeName.Midnight, theme)
    }

    @Test
    fun anAgentGetsSixTonesFromItsNameAndTheSameNameAlwaysGetsTheSame() {
        val a = AgentLook.forName("Tifa")
        val b = AgentLook.forName("Tifa")
        assertEquals(6, a.tones.size)
        assertEquals(a.tones, b.tones)
        assertEquals('T', a.letter)
        assertEquals(a.tones[0], a.main)
        assertNotEquals(AgentLook.forName("Tifa").tones, AgentLook.forName("Nagisa").tones)
    }

    @Test
    fun aMonogramSurvivesAnEmptyOrPaddedName() {
        assertEquals('?', AgentLook.forName("").letter)
        assertEquals('K', AgentLook.forName("  kai'sa").letter)
    }
}
