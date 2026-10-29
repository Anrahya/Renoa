package live.renoa.app.badge

import androidx.compose.ui.geometry.Rect
import androidx.compose.ui.graphics.Color
import java.io.File
import kotlin.test.Test
import kotlin.test.assertEquals
import kotlin.test.assertFailsWith
import kotlin.test.assertTrue

class MascotTest {
    private val svg = File("src/main/res/raw/renoa_mascot.svg").readText()

    @Test
    fun theMascotReadsAsShapesInOrderWithItsPortraitAndHerStar() {
        val drawing = readSvg(svg)
        assertEquals(Rect(40f, 40f, 160f, 160f), drawing.portrait)
        assertTrue(drawing.shapes.size > 30)
        assertEquals(Color(0xFFE3B23C), drawing.shapes.last().fill, "the star hairpin is drawn last, over her hair")
        val blush = drawing.shapes.first { it.outline is Outline.Oval && it.fill == Color(0xFFF09C8C) }
        assertEquals(0.55f, blush.opacity)
        assertEquals(Rect(64f, 119.6f, 78f, 126.4f), (blush.outline as Outline.Oval).bounds)
    }

    @Test
    fun anythingTheAppWouldDrawWrongIsRefusedByName() {
        val portrait = """<rect id="portrait" x="0" y="0" width="10" height="10" fill="none"/>"""
        fun refusal(body: String) = assertFailsWith<IllegalArgumentException> { readSvg("<svg>$body</svg>") }.message.orEmpty()
        assertEquals("the mascot SVG has a <g>, which the app does not draw", refusal("$portrait<g><path d=\"M0 0\"/></g>"))
        assertTrue("transform=" in refusal("""$portrait<path d="M0 0" transform="scale(2)"/>"""))
        assertTrue("#RRGGBB" in refusal("""$portrait<path d="M0 0" fill="red"/>"""))
        assertTrue("one <rect>" in refusal("""$portrait$portrait"""))
        assertTrue("no <rect" in refusal("""<path d="M0 0"/>"""))
        assertTrue("no d=" in refusal("""$portrait<path fill="#000000"/>"""))
    }
}
