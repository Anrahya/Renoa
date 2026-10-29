package live.renoa.app.chat

import androidx.compose.ui.graphics.toArgb
import com.mikepenz.markdown.model.State
import com.mikepenz.markdown.model.parseMarkdown
import live.renoa.app.R
import live.renoa.app.theme.ThemeName
import org.intellij.markdown.MarkdownElementTypes
import org.intellij.markdown.ast.ASTNode
import org.junit.Test
import org.junit.runner.RunWith
import org.robolectric.RobolectricTestRunner
import org.robolectric.RuntimeEnvironment
import org.robolectric.annotation.Config
import java.io.File
import java.util.Base64
import kotlin.test.assertEquals
import kotlin.test.assertFalse
import kotlin.test.assertIs
import kotlin.test.assertNotEquals
import kotlin.test.assertNull
import kotlin.test.assertTrue

/** The page a diagram is drawn in, built from the app's real assets. Each page built is also saved to build/diagrams to open in a browser. */
@RunWith(RobolectricTestRunner::class)
@Config(sdk = [36])
class DiagramPageTest {
    private val context = RuntimeEnvironment.getApplication()
    private val kit by lazy { diagramKit(context) }

    private fun page(source: String, theme: ThemeName = ThemeName.Graphite, nonce: String = "n0nce", name: String? = null) =
        diagramPage(kit, source, theme.colors, nonce).also { page ->
            if (name != null) File("build/diagrams").apply { mkdirs() }.resolve("$name-${theme.name.lowercase()}.html").writeText(page)
        }

    private val sourceOpen = "<div id=\"source\" hidden>"

    /** The source as the page holds it: the text between its element's tags, before the browser decodes it. */
    private fun held(page: String): String = page.substringAfter(sourceOpen).substringBefore("</div>")

    private fun decoded(html: String) = html.replace("&lt;", "<").replace("&gt;", ">").replace("&quot;", "\"").replace("&#39;", "'").replace("&amp;", "&")

    @Test
    fun theSourceIsTextThatClosesAndOpensNothingAndReadsBackExactly() {
        val hostile = "flowchart LR\n  A[\"</script><script>alert(1)</script>\"] --> B['it''s `tick` & &amp; {{pele}}']\n  </div><img src=x onerror=alert(2)>"
        val built = page(hostile, name = "hostile")
        val inPage = held(built)
        assertFalse('<' in inPage || '>' in inPage || '"' in inPage || '\'' in inPage, inPage)
        assertEquals(hostile, decoded(inPage))
        // Apart from the source, the page is the page of an empty diagram: the source changed nothing else, and no placeholder in it was filled.
        val empty = page("")
        assertEquals(empty.replace(sourceOpen + held(empty), sourceOpen), built.replace(sourceOpen + inPage, sourceOpen))
    }

    @Test
    fun onlyThePagesOwnScriptsRunAndNothingLoadsBeyondTheFontItCarries() {
        val built = page("flowchart TD\n  A --> B", nonce = "abc123", name = "flowchart")
        val policy = built.substringAfter("Content-Security-Policy\" content=\"").substringBefore('"')
        assertEquals("default-src 'none'; script-src 'nonce-abc123'; style-src 'unsafe-inline'; font-src data:; base-uri 'none'; form-action 'none'", policy)
        assertEquals(2, Regex("<script\\b").findAll(built).count())
        assertEquals(2, Regex("<script nonce=\"abc123\">").findAll(built).count())
        assertTrue(built.contains("<script nonce=\"abc123\">${kit.pele}</script>"), "Pele runs whole, as shipped")
        assertNotEquals(diagramPage(kit, "", ThemeName.Graphite.colors), diagramPage(kit, "", ThemeName.Graphite.colors), "each page has its own nonce")
    }

    @Test
    fun theKitIsPeleAsShippedAndRenoasSans() {
        assertTrue(kit.pele.startsWith("/*! Pele 1.12.0 | MIT License"), kit.pele.take(60))
        val sans = context.resources.openRawResource(R.font.instrument_sans).use { it.readBytes() }
        assertTrue(sans.contentEquals(Base64.getDecoder().decode(kit.font)))
    }

    @Test
    fun theDiagramTakesItsColoursFromTheThemeItIsDrawnIn() {
        for (theme in ThemeName.entries) {
            val colors = theme.colors
            val built = page("pie title Your week\n  \"Work\" : 40\n  \"Sleep\" : 50\n  \"Cooking\" : 8", theme, name = "pie")
            val variables = Regex("--pele-([\\w-]+): ([^;]+);").findAll(peleTheme(colors)).associate { it.groupValues[1] to it.groupValues[2] }
            assertTrue(built.contains(":root { ${peleTheme(colors)} }"))
            // The page claims its own theme's scheme alone, so a browser that darkens pages leaves a Paper diagram light.
            val scheme = built.substringAfter("<meta name=\"color-scheme\" content=\"").substringBefore('"')
            assertEquals(if (theme.dark) "only dark" else "only light", scheme)
            fun hex(c: androidx.compose.ui.graphics.Color) = "#%06X".format(c.toArgb() and 0xFFFFFF)
            assertEquals(hex(colors.background), variables["bg"])
            assertEquals(hex(colors.foreground), variables["fg"])
            assertEquals(hex(colors.primary), variables["accent"])
            assertEquals(hex(colors.partSkills), variables["series-1"])
            assertEquals((1..8).map { "series-$it" }.toSet(), variables.keys.filter { it.startsWith("series-") }.toSet())
            assertTrue(variables.getValue("font").startsWith("\"Instrument Sans\""))
            page(
                "flowchart TD\n  A[Check the fridge] --> B{Lentils left?}\n  B -->|Yes| C[Dal and rice]\n  B -->|No| D[Order groceries]\n  D --> C\n  C --> E[Salad on the side]",
                theme,
                name = "dinner",
            )
        }
        assertNotEquals(peleTheme(ThemeName.Graphite.colors), peleTheme(ThemeName.Paper.colors))
        page("flowchart LR\n  A[Wake up] --> B[Coffee] --> C[Check messages] --> D[Plan the day] --> E[Groceries] --> F[Cook dinner] --> G[Sleep]", name = "wide")
        page("flowchart LR\n  A -->", name = "broken")
    }

    @Test
    fun thePageSaysItsHeightOrThatItCannotDrawAndNothingElseCounts() {
        assertEquals(DiagramReport.Drawn(240), diagramReport("h:240"))
        assertEquals(DiagramReport.Failed, diagramReport("x"))
        assertEquals(DiagramReport.Drawn(MAX_DIAGRAM_HEIGHT), diagramReport("h:$MAX_DIAGRAM_HEIGHT"))
        // Too tall to lay out, or to read as a picture: it shows as its code.
        assertEquals(DiagramReport.Failed, diagramReport("h:${MAX_DIAGRAM_HEIGHT + 1}"))
        assertEquals(DiagramReport.Failed, diagramReport("h:55630"))
        for (other in listOf(null, "", "about:blank", "h:", "h:0", "h:-3", "h:12px", "h:999999", "240", "X", "data:text/html,")) assertNull(diagramReport(other), "$other")
    }

    @Test
    fun aDiagramIsNamedByItsTitleOrItsFirstLine() {
        assertEquals("Diagram: Dinner tonight", diagramLabel("flowchart TD\n  accTitle: Dinner tonight\n  title Other\n  A --> B"))
        assertEquals("Diagram: Your week", diagramLabel("---\ntitle: Your week\n---\npie\n  \"Work\" : 40"))
        assertEquals("Diagram: Launch plan", diagramLabel("gantt\n  title Launch plan\n  section A"))
        assertEquals("Diagram: flowchart TD", diagramLabel("\n  flowchart TD\n  A --> B"))
        assertEquals("Diagram", diagramLabel("  \n"))
    }

    private fun fence(markdown: String): MermaidFence? {
        val parsed = assertIs<State.Success>(parseMarkdown(markdown))
        fun find(node: ASTNode): ASTNode? = if (node.type == MarkdownElementTypes.CODE_FENCE) node else node.children.firstNotNullOfOrNull(::find)
        return mermaidFence(parsed.content, find(parsed.node)!!)
    }

    @Test
    fun aMermaidFenceIsADiagramOnceItsClosingFenceHasArrived() {
        val closed = fence("Look:\n\n```mermaid\nflowchart TD\n  A --> B\n```\n\nDone.")!!
        assertEquals("flowchart TD\n  A --> B", closed.source)
        assertTrue(closed.closed)
        val open = fence("Look:\n\n```Mermaid\nflowchart TD\n  A --> B\n")!!
        assertEquals("flowchart TD\n  A --> B", open.source)
        assertFalse(open.closed)
        assertFalse(fence("```mermaid")!!.closed)
        assertEquals("", fence("```mermaid")!!.source)
        assertEquals("flowchart TD", fence("- step\n\n  ```mermaid\n  flowchart TD\n  ```\n")!!.source)
        assertNull(fence("```kotlin\nval a = 1\n```"))
        assertNull(fence("```\nflowchart TD\n```"))
        assertNull(fence("```mermaidish\nflowchart TD\n```"))
    }
}
