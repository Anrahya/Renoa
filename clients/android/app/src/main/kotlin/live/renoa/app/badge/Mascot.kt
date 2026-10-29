package live.renoa.app.badge

import androidx.compose.runtime.Composable
import androidx.compose.ui.geometry.Rect
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.graphics.Path
import androidx.compose.ui.graphics.StrokeCap
import androidx.compose.ui.graphics.StrokeJoin
import androidx.compose.ui.graphics.drawscope.DrawScope
import androidx.compose.ui.graphics.drawscope.Stroke
import androidx.compose.ui.graphics.vector.PathParser
import androidx.compose.ui.platform.LocalResources
import live.renoa.app.R
import org.w3c.dom.Element
import org.xml.sax.InputSource
import org.xml.sax.SAXException
import java.io.StringReader
import javax.xml.parsers.DocumentBuilderFactory

/** Where a shape lies: SVG path data, or an oval's bounds. */
internal sealed interface Outline {
    data class Data(val d: String) : Outline

    data class Oval(val bounds: Rect) : Outline
}

internal data class SvgShape(val outline: Outline, val fill: Color?, val stroke: Color?, val strokeWidth: Float, val opacity: Float)

/** A drawing read from SVG, and the part of it a tile shows: the head and shoulders. */
internal data class SvgDrawing(val shapes: List<SvgShape>, val portrait: Rect)

private val PAINT = listOf("fill", "stroke", "stroke-width", "opacity")

/** What may sit inside `<svg>`, and the attributes the app reads on each. */
private val ELEMENTS =
    mapOf(
        "title" to emptyList(),
        "desc" to emptyList(),
        "rect" to listOf("id", "x", "y", "width", "height", "fill"),
        "path" to listOf("d") + PAINT,
        "circle" to listOf("cx", "cy", "r") + PAINT,
        "ellipse" to listOf("cx", "cy", "rx", "ry") + PAINT,
    )
private val ROOT = listOf("xmlns", "viewBox", "width", "height")
private val NUMBER = Regex("[+-]?(\\d+\\.?\\d*|\\.\\d+)([eE][+-]?\\d+)?")

/**
 * Reads the small SVG the app draws its mascot from. Inside `<svg>` sit `path`, `circle` and
 * `ellipse`, each with a `fill` and optionally `stroke`, `stroke-width` and `opacity`, drawn in
 * order with round joins and caps; and one square `rect` with id `portrait` and no fill, which
 * marks what a tile shows. Anything else, an attribute an element does not take, or a value the
 * app would read differently from SVG is refused by name, so a drawing the app would draw wrong
 * never loads.
 */
internal fun readSvg(text: String): SvgDrawing {
    val root =
        try {
            DocumentBuilderFactory.newInstance().newDocumentBuilder().parse(InputSource(StringReader(text))).documentElement
        } catch (broken: SAXException) {
            throw IllegalArgumentException("the mascot SVG is not well-formed XML: ${broken.message}", broken)
        }
    require(root.tagName == "svg") { "the mascot SVG is a <${root.tagName}>, not an <svg>" }
    root.allow(ROOT)
    val shapes = mutableListOf<SvgShape>()
    var portrait: Rect? = null
    for (element in root.elements()) {
        val name = element.tagName
        element.allow(ELEMENTS[name] ?: throw IllegalArgumentException("the mascot SVG has a <$name>, which the app does not draw"))
        element.elements().firstOrNull()?.let { throw IllegalArgumentException("the mascot SVG's <$name> holds a <${it.tagName}>; put every shape straight inside <svg>") }
        when (name) {
            "title", "desc" -> Unit
            "rect" -> {
                require(element.getAttribute("id") == "portrait" && portrait == null) { "the mascot SVG may have one <rect>, with id=\"portrait\"" }
                require(element.getAttribute("fill") == "none") { "the portrait <rect> is not drawn; give it fill=\"none\"" }
                val (x, y) = element.number("x") to element.number("y")
                val side = element.positive("width")
                require(element.positive("height") == side) { "the portrait <rect> must be square, as a tile's face is" }
                portrait = Rect(x, y, x + side, y + side)
            }
            else -> shapes += element.shape()
        }
    }
    return SvgDrawing(shapes, portrait ?: throw IllegalArgumentException("the mascot SVG has no <rect id=\"portrait\"> to say what a tile shows"))
}

private fun Element.shape(): SvgShape {
    val outline =
        when (tagName) {
            "path" -> Outline.Data(getAttribute("d").ifEmpty { throw IllegalArgumentException("the mascot SVG has a <path> with no d=") })
            "circle" -> positive("r").let { r -> Outline.Oval(Rect(number("cx") - r, number("cy") - r, number("cx") + r, number("cy") + r)) }
            else -> Outline.Oval(Rect(number("cx") - positive("rx"), number("cy") - positive("ry"), number("cx") + positive("rx"), number("cy") + positive("ry")))
        }
    require(hasAttribute("fill")) { "the mascot SVG's <$tagName> has no fill=; SVG paints that black, so write its colour or fill=\"none\"" }
    val fill = colour(getAttribute("fill"))
    val stroke = if (hasAttribute("stroke")) colour(getAttribute("stroke")) else null
    val opacity = if (hasAttribute("opacity")) number("opacity") else 1f
    require(opacity in 0f..1f) { "the mascot SVG's <$tagName> has opacity=\"${getAttribute("opacity")}\"; write it from 0 to 1" }
    require(opacity == 1f || fill == null || stroke == null) {
        "the mascot SVG's <$tagName> has opacity= with both a fill and a stroke, which SVG fades together and the app would fade apart; split it in two"
    }
    return SvgShape(outline, fill, stroke, if (hasAttribute("stroke-width")) positive("stroke-width") else 1f, opacity)
}

private fun Element.elements(): List<Element> = (0 until childNodes.length).map { childNodes.item(it) }.filterIsInstance<Element>()

private fun Element.allow(reads: List<String>) {
    for (i in 0 until attributes.length) {
        val key = attributes.item(i).nodeName
        require(key in reads) { "the mascot SVG's <$tagName> has $key=, which the app does not draw" }
    }
}

/** A plain number such as `12`, `-0.5` or `1e2`; SVG units, percentages and words are refused. */
private fun Element.number(key: String): Float {
    val value = getAttribute(key)
    require(NUMBER.matches(value)) { "the mascot SVG's <$tagName> needs a plain number for $key=, not \"$value\"" }
    return value.toFloat()
}

private fun Element.positive(key: String): Float = number(key).also { require(it > 0f) { "the mascot SVG's <$tagName> needs $key= above 0" } }

/** "#RRGGBB", or "none" for no paint. */
private fun colour(value: String): Color? {
    if (value == "none") return null
    require(Regex("#[0-9A-Fa-f]{6}").matches(value)) { "the mascot SVG has the colour $value; write it as #RRGGBB" }
    return Color(0xFF000000 or value.drop(1).toLong(16))
}

/** Renoa's mascot, ready to draw. A placeholder until the drawn portraits arrive. */
internal class Mascot(val drawing: SvgDrawing) {
    private val paths =
        drawing.shapes.map { shape ->
            when (val outline = shape.outline) {
                is Outline.Data -> PathParser().parsePathString(outline.d).toPath()
                is Outline.Oval -> Path().apply { addOval(outline.bounds) }
            }
        }

    /** Draws the whole drawing in its own units. */
    fun draw(scope: DrawScope) {
        drawing.shapes.forEachIndexed { i, shape ->
            shape.fill?.let { scope.drawPath(paths[i], it, alpha = shape.opacity) }
            shape.stroke?.let { scope.drawPath(paths[i], it, alpha = shape.opacity, style = Stroke(shape.strokeWidth, cap = StrokeCap.Round, join = StrokeJoin.Round)) }
        }
    }
}

/** Read once for the whole app, the first time a tile needs her. */
@Volatile private var loaded: Mascot? = null

@Composable
internal fun renoaMascot(): Mascot {
    val resources = LocalResources.current
    return loaded ?: Mascot(readSvg(resources.openRawResource(R.raw.renoa_mascot).bufferedReader().use { it.readText() })).also { loaded = it }
}
