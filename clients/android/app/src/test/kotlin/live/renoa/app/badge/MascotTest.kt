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
    private val portrait = """<rect id="portrait" x="0" y="0" width="10" height="10" fill="none"/>"""

    private fun read(body: String) = readSvg("<svg>$portrait$body</svg>")

    private fun refusal(text: String) = assertFailsWith<IllegalArgumentException> { readSvg(text) }.message.orEmpty()

    private fun refused(body: String) = refusal("<svg>$portrait$body</svg>")

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
    fun itReadsTheFileAsXmlTheWayABrowserDoes() {
        val shape = read("""<!-- a note --><desc><![CDATA[<path d="M0 0" fill="#000000"/>]]></desc><path d='M0 0' fill='#FF0000' opacity=".5"/>""").shapes.single()
        assertEquals(Color(0xFFFF0000), shape.fill, "single quotes are quotes, and a description is words, not shapes")
        assertEquals(0.5f, shape.opacity)
        assertEquals(1f, shape.strokeWidth)
        val ellipse = read("""<ellipse cx="0" cy="0" rx="1e2" ry="50" fill="none" stroke="#000000"/>""").shapes.single()
        assertEquals(Rect(-100f, -50f, 100f, 50f), (ellipse.outline as Outline.Oval).bounds)
        assertEquals(null, ellipse.fill)
    }

    @Test
    fun anythingTheAppWouldDrawWrongIsRefusedByName() {
        val dot = """d="M0 0" fill="#000000""""
        assertEquals("the mascot SVG has a <g>, which the app does not draw", refused("<g><path $dot/></g>"))
        assertEquals("the mascot SVG has a <svg:path>, which the app does not draw", refused("<svg:path $dot/>"))
        assertTrue("<title> holds a <path>" in refused("<title><path $dot/></title>"))
        assertEquals("the mascot SVG's <path> has transform=, which the app does not draw", refused("<path $dot transform='scale(2)'/>"))
        assertTrue("<path> has r=" in refused("<path $dot r=\"3\"/>"), "an attribute of another element")
        assertTrue("<svg> has transform=" in refusal("""<svg transform="scale(3)">$portrait</svg>"""))
        assertTrue("not well-formed" in refusal("<svg>$portrait<path $dot></svg>"))

        assertTrue("#RRGGBB" in refused("""<path d="M0 0" fill="red"/>"""))
        assertTrue("no fill=" in refused("""<path d="M0 0" stroke="#000000"/>"""), "SVG paints a missing fill black")
        assertTrue("plain number for r=" in refused("""<circle cx="0" cy="0" r="Infinity" fill="none"/>"""))
        assertTrue("plain number for stroke-width=" in refused("""<path $dot stroke="#000000" stroke-width="2px"/>"""))
        assertTrue("r= above 0" in refused("""<circle cx="0" cy="0" r="-1" fill="none"/>"""))
        assertTrue("from 0 to 1" in refused("""<path $dot opacity="7"/>"""))
        assertTrue("split it in two" in refused("""<path $dot stroke="#000000" opacity="0.5"/>"""))
        assertTrue("no d=" in refused("""<path fill="#000000"/>"""))

        assertTrue("one <rect>" in refused(portrait))
        assertTrue("no <rect" in refusal("<svg><path $dot/></svg>"))
        assertTrue("square" in refusal("""<svg><rect id="portrait" x="0" y="0" width="10" height="5" fill="none"/></svg>"""))
        assertTrue("width= above 0" in refusal("""<svg><rect id="portrait" x="0" y="0" width="0" height="0" fill="none"/></svg>"""))
        assertTrue("fill=\"none\"" in refusal("""<svg><rect id="portrait" x="0" y="0" width="10" height="10"/></svg>"""))
    }
}
