package live.renoa.app.theme

import androidx.compose.ui.graphics.Color
import androidx.compose.ui.graphics.luminance
import java.io.File
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
    fun theDesignSystemsTwoThemesAreGraphiteDarkAndPaperLight() {
        assertEquals(listOf("Graphite" to true, "Paper" to false), ThemeName.entries.map { it.name to it.dark })
        assertTrue(ThemeName.Graphite.colors.background.luminance() < ThemeName.Paper.colors.background.luminance())
    }

    @Test
    fun inkIsTheAccentThePrimaryIsTheForeground() {
        for (theme in ThemeName.entries) assertEquals(theme.colors.foreground, theme.colors.primary, theme.name)
    }

    @Test
    fun everyThemeSetsEveryTokenToARealColour() {
        val tokens = RenoaColors::class.memberProperties
        for (theme in ThemeName.entries) {
            for (token in tokens) {
                val value = token.get(theme.colors) as Color
                assertNotEquals(Color.Unspecified, value, "${theme.name} leaves ${token.name} unset")
                assertEquals(1f, value.alpha, "${theme.name} ${token.name} should be opaque")
            }
        }
    }

    @Test
    fun noTwoThemesShareAGround() {
        val colors = ThemeName.entries.map { it.colors }
        assertEquals(colors.size, colors.map { it.background }.toSet().size)
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
        var theme = ThemeName.Graphite
        val seen = mutableListOf<ThemeName>()
        repeat(ThemeName.entries.size) {
            seen += theme
            theme = theme.next()
        }
        assertEquals(ThemeName.entries.toList(), seen)
        assertEquals(ThemeName.Graphite, theme)
    }

    /** The `--name: #RRGGBB` declarations of the block [selector] opens in `design/system/tokens.css`. */
    private fun tokens(selector: String): Map<String, Color> {
        val css = File(System.getProperty("renoa.design"), "system/tokens.css").readText()
        val start = css.indexOf("$selector {")
        assertTrue(start >= 0, "tokens.css has no $selector block")
        val block = css.substring(start, css.indexOf('}', start))
        return Regex("""--([a-z0-9-]+):\s*#([0-9A-Fa-f]{6});""").findAll(block).associate { match ->
            match.groupValues[1] to Color(0xFF000000 or match.groupValues[2].toLong(16))
        }
    }

    @Test
    fun everyColourIsTheDesignSystemsToken() {
        val blocks = mapOf(ThemeName.Paper to tokens(":root"), ThemeName.Graphite to tokens(":root[data-theme=\"dark\"]"))
        for ((theme, declared) in blocks) {
            for (token in RenoaColors::class.memberProperties) {
                val name = token.name.replace(Regex("[A-Z]")) { "-" + it.value.lowercase() }
                assertEquals(declared[name], token.get(theme.colors), "${theme.name} --$name")
            }
        }
    }
}
