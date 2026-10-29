package live.renoa.app.badge

import androidx.compose.runtime.Composable
import androidx.compose.runtime.remember
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

/** Where a shape lies: SVG path data, or an oval's bounds. */
internal sealed interface Outline {
    data class Data(val d: String) : Outline

    data class Oval(val bounds: Rect) : Outline
}

internal data class SvgShape(val outline: Outline, val fill: Color?, val stroke: Color?, val strokeWidth: Float, val opacity: Float)

/** A drawing read from SVG, and the part of it a tile shows: the head and shoulders. */
internal data class SvgDrawing(val shapes: List<SvgShape>, val portrait: Rect)

private val ELEMENTS = setOf("svg", "title", "desc", "path", "circle", "ellipse", "rect")
private val ATTRIBUTES = setOf("xmlns", "viewBox", "width", "height", "id", "d", "cx", "cy", "r", "rx", "ry", "x", "y", "fill", "stroke", "stroke-width", "opacity")

/**
 * Reads the small SVG the app draws its mascot from: `path`, `circle` and `ellipse` with `fill`,
 * `stroke`, `stroke-width` and `opacity`, drawn in order with round joins and caps, and one
 * `rect` with id `portrait` that marks what a tile shows and is not drawn. Anything else is
 * refused by name, so a drawing the app would draw wrong never loads.
 */
internal fun readSvg(text: String): SvgDrawing {
    val shapes = mutableListOf<SvgShape>()
    var portrait: Rect? = null
    val body = text.replace(Regex("<!--.*?-->", RegexOption.DOT_MATCHES_ALL), "").replace(Regex("<\\?.*?\\?>", RegexOption.DOT_MATCHES_ALL), "")
    for (tag in Regex("<\\s*(/?)\\s*([A-Za-z]+)([^>]*)>").findAll(body)) {
        val (closing, name, rest) = tag.destructured
        require(name in ELEMENTS) { "the mascot SVG has a <$name>, which the app does not draw" }
        if (closing.isNotEmpty() || name in setOf("svg", "title", "desc")) continue
        val attributes = Regex("([A-Za-z:-]+)\\s*=\\s*\"([^\"]*)\"").findAll(rest).associate { it.groupValues[1] to it.groupValues[2] }
        attributes.keys.firstOrNull { it !in ATTRIBUTES }?.let { throw IllegalArgumentException("the mascot SVG's <$name> has $it=, which the app does not draw") }
        fun number(key: String): Float = attributes[key]?.toFloatOrNull() ?: throw IllegalArgumentException("the mascot SVG's <$name> needs a number for $key=")
        if (name == "rect") {
            require(attributes["id"] == "portrait" && portrait == null) { "the mascot SVG may have one <rect>, with id=\"portrait\"" }
            portrait = Rect(number("x"), number("y"), number("x") + number("width"), number("y") + number("height"))
            continue
        }
        val outline =
            when (name) {
                "path" -> Outline.Data(attributes["d"] ?: throw IllegalArgumentException("the mascot SVG has a <path> with no d="))
                "circle" -> number("r").let { r -> Outline.Oval(Rect(number("cx") - r, number("cy") - r, number("cx") + r, number("cy") + r)) }
                else -> Outline.Oval(Rect(number("cx") - number("rx"), number("cy") - number("ry"), number("cx") + number("rx"), number("cy") + number("ry")))
            }
        shapes += SvgShape(outline, colour(attributes["fill"]), colour(attributes["stroke"]), attributes["stroke-width"]?.toFloatOrNull() ?: 1f, attributes["opacity"]?.toFloatOrNull() ?: 1f)
    }
    return SvgDrawing(shapes, portrait ?: throw IllegalArgumentException("the mascot SVG has no <rect id=\"portrait\"> to say what a tile shows"))
}

/** "#RRGGBB", or "none" and absent for no paint. */
private fun colour(value: String?): Color? {
    if (value == null || value == "none") return null
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

@Composable
internal fun rememberMascot(): Mascot {
    val resources = LocalResources.current
    return remember(resources) { Mascot(readSvg(resources.openRawResource(R.raw.renoa_mascot).bufferedReader().use { it.readText() })) }
}
