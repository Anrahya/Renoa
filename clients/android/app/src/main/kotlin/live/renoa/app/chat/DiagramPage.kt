package live.renoa.app.chat

import android.annotation.SuppressLint
import android.content.Context
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.graphics.luminance
import androidx.compose.ui.graphics.toArgb
import live.renoa.app.R
import live.renoa.app.theme.RenoaColors
import live.renoa.app.theme.RenoaShapes
import live.renoa.app.theme.ThemeName
import java.security.SecureRandom
import java.util.Base64

/**
 * What every diagram's page is made from: the page around a diagram ([template], from
 * `assets/diagram/page.html`), Pele itself ([pele], its browser build), and Renoa's sans as base64
 * ([font]), so the page needs nothing it does not carry.
 */
internal class DiagramKit(val template: String, val pele: String, val font: String)

private var kit: DiagramKit? = null

/** The kit, read from the app's assets on first use and kept for the life of the process. */
// A font resource is a file like a raw one, and is read as one: the page needs its bytes, not a Typeface.
@SuppressLint("ResourceType")
@Synchronized
internal fun diagramKit(context: Context): DiagramKit =
    kit ?: run {
        val assets = context.assets
        DiagramKit(
            template = assets.open("diagram/page.html").use { String(it.readBytes(), Charsets.UTF_8) },
            pele = assets.open("diagram/pele.global.js").use { String(it.readBytes(), Charsets.UTF_8) },
            font = context.resources.openRawResource(R.font.instrument_sans).use { Base64.getEncoder().encodeToString(it.readBytes()) },
        ).also { kit = it }
    }

/** The kit if it has been read already, so a diagram drawn again need not wait for it. */
@Synchronized
internal fun diagramKitIfRead(): DiagramKit? = kit

/**
 * The page that draws [source] in [colors]: Pele, the source as escaped text it reads as data, and
 * the theme as Pele's CSS variables. [nonce] lets the page's own scripts, and only them, run. The
 * page declares its theme's colour scheme alone, so a browser that darkens pages leaves it be.
 * Each `{{name}}` in the template is filled once, so nothing filled in is read as a placeholder.
 */
internal fun diagramPage(kit: DiagramKit, source: String, colors: RenoaColors, nonce: String = newNonce()): String {
    val scheme = if (colors.background.luminance() < 0.5f) "only dark" else "only light"
    val fills = mapOf("nonce" to nonce, "scheme" to scheme, "font" to kit.font, "theme" to peleTheme(colors), "pele" to kit.pele, "source" to escapeHtml(source))
    return PLACEHOLDER.replace(kit.template) { fills[it.groupValues[1]] ?: error("the diagram page has no ${it.value}") }
}

private val PLACEHOLDER = Regex("\\{\\{(\\w+)\\}\\}")

/** [text] as HTML text: it can close no element and open none. */
internal fun escapeHtml(text: String): String =
    buildString(text.length) {
        for (c in text) {
            when (c) {
                '&' -> append("&amp;")
                '<' -> append("&lt;")
                '>' -> append("&gt;")
                '"' -> append("&quot;")
                '\'' -> append("&#39;")
                else -> append(c)
            }
        }
    }

/**
 * Pele's CSS variables in [colors], so a diagram sits in a reply like the rest of it: the reply's
 * ground behind labels, ink for text and accents, the code block's grey for shapes, and the
 * parts' pigments for a chart's series. Labels on a fill the diagram sets itself take whichever
 * theme's ink reads on it.
 */
internal fun peleTheme(colors: RenoaColors): String {
    val series = listOf(colors.partSkills, colors.partModel, colors.partLoop, colors.partContext, colors.mutedForeground, colors.partTools, colors.partProfile, colors.foreground)
    val variables =
        listOf(
            "bg" to hex(colors.background),
            "fg" to hex(colors.foreground),
            "muted" to hex(colors.mutedForeground),
            "line" to hex(colors.foregroundSoft),
            "surface" to hex(colors.secondary),
            "surface-alt" to hex(colors.muted),
            "border" to hex(colors.input),
            "accent" to hex(colors.primary),
            "critical" to hex(colors.stateFailed),
            "error" to hex(colors.stateFailed),
            "fg-on-light" to hex(ThemeName.Paper.colors.foreground),
            "fg-on-dark" to hex(ThemeName.Graphite.colors.foreground),
            "font" to "\"Instrument Sans\", sans-serif",
            "font-mono" to "monospace",
            "radius" to "${RenoaShapes.Control.value}px",
        ) + series.mapIndexed { i, colour -> "series-${i + 1}" to hex(colour) }
    return variables.joinToString(" ") { (name, value) -> "--pele-$name: $value;" }
}

private fun hex(colour: Color) = "#%06X".format(colour.toArgb() and 0xFFFFFF)

private val random = SecureRandom()

private fun newNonce(): String = ByteArray(16).also(random::nextBytes).joinToString("") { "%02x".format(it) }
