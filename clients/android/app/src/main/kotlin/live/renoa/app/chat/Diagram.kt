package live.renoa.app.chat

import android.annotation.SuppressLint
import android.content.Context
import android.view.View
import android.webkit.RenderProcessGoneDetail
import android.webkit.WebChromeClient
import android.webkit.WebResourceRequest
import android.webkit.WebResourceResponse
import android.webkit.WebView
import android.webkit.WebViewClient
import androidx.compose.foundation.background
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.height
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.material3.Text
import androidx.compose.runtime.Composable
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.produceState
import androidx.compose.runtime.remember
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.draw.alpha
import androidx.compose.ui.platform.LocalContext
import androidx.compose.ui.semantics.Role
import androidx.compose.ui.semantics.contentDescription
import androidx.compose.ui.semantics.role
import androidx.compose.ui.semantics.semantics
import androidx.compose.ui.unit.dp
import androidx.compose.ui.viewinterop.AndroidView
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.withContext
import live.renoa.app.theme.Renoa
import live.renoa.app.theme.RenoaShapes
import live.renoa.app.theme.RenoaType

/** What a diagram's placeholder says while it waits to be drawn. */
internal const val DRAWING = "Drawing…"

/**
 * A Mermaid diagram drawn from [source] by Pele in a web view of its own, sized to the drawing. A
 * drawing wider than the reply scrolls sideways in its box. Until it is first drawn it is a
 * placeholder; if it cannot be drawn, it is [fallback] instead, the source as a code block.
 */
@Composable
internal fun Diagram(source: String, fallback: @Composable () -> Unit) {
    var failed by remember(source) { mutableStateOf(false) }
    if (failed) return fallback()
    val context = LocalContext.current
    val colors = Renoa.colors
    var height by remember(source) { mutableStateOf(heights[source]) }
    val kit by produceState(diagramKitIfRead(), context) { if (value == null) value = withContext(Dispatchers.IO) { diagramKit(context) } }
    val page = remember(kit, source, colors) { kit?.let { diagramPage(it, source, colors) } }
    val label = remember(source) { diagramLabel(source) }
    Box(
        Modifier
            .padding(vertical = 8.dp)
            .fillMaxWidth()
            .height(height?.dp ?: PLACEHOLDER_HEIGHT)
            .semantics(mergeDescendants = true) {
                contentDescription = label
                role = Role.Image
            },
    ) {
        if (page != null) {
            AndroidView(
                factory = { diagramView(it) },
                modifier = Modifier.matchParentSize().alpha(if (height == null) 0f else 1f),
                update = { view ->
                    view.reports { report ->
                        when (report) {
                            DiagramReport.Failed -> failed = true
                            is DiagramReport.Drawn -> {
                                height = report.height
                                heights[source] = report.height
                            }
                        }
                    }
                    // Run again only when the page changes, as with the theme: each run draws the diagram anew.
                    view.loadDataWithBaseURL(null, page, "text/html", "utf-8", null)
                },
                onRelease = { it.destroy() },
            )
        }
        if (height == null) Placeholder(Modifier.matchParentSize())
    }
}

/** An unfinished diagram's stand-in, quiet until the whole of it has arrived and is drawn. */
@Composable
internal fun DiagramPlaceholder() = Placeholder(Modifier.padding(vertical = 8.dp).fillMaxWidth().height(PLACEHOLDER_HEIGHT))

@Composable
private fun Placeholder(modifier: Modifier) {
    val colors = Renoa.colors
    Box(modifier.background(colors.muted, RoundedCornerShape(RenoaShapes.Control)).padding(horizontal = 12.dp), contentAlignment = Alignment.CenterStart) {
        Text(DRAWING, style = RenoaType.Caption, color = colors.mutedForeground)
    }
}

private val PLACEHOLDER_HEIGHT = 48.dp

/**
 * Drawn heights by source, so a diagram scrolled back into view takes its height at once rather
 * than growing from the placeholder. A wrong guess, as after a turn of the phone, is corrected as
 * soon as the page measures. Main thread only.
 */
private val heights =
    object : LinkedHashMap<String, Int>(16, 0.75f, true) {
        override fun removeEldestEntry(eldest: MutableMap.MutableEntry<String, Int>) = size > 64
    }

/** What a diagram's page says through its title: drawn at a height in dp, or not drawable. */
internal sealed interface DiagramReport {
    data class Drawn(val height: Int) : DiagramReport

    data object Failed : DiagramReport
}

/** The report in a page's [title], or null for any other title, such as the one a page has before it says anything. */
internal fun diagramReport(title: String?): DiagramReport? {
    if (title == "x") return DiagramReport.Failed
    val height = title?.let(HEIGHT::matchEntire)?.groupValues?.get(1)?.toInt() ?: return null
    return if (height > 0) DiagramReport.Drawn(height) else null
}

private val HEIGHT = Regex("h:(\\d{1,5})")

/** How a screen reader names a diagram: by its accessible title, else its title, else its first line. */
internal fun diagramLabel(source: String): String {
    val lines = source.lines().map { it.trim() }.filter { it.isNotEmpty() }
    fun field(name: String) = Regex("^$name(?:\\s*:\\s*|\\s+)(\\S.*)$", RegexOption.IGNORE_CASE).let { field -> lines.firstNotNullOfOrNull { field.find(it)?.groupValues?.get(1) } }
    val name = field("accTitle") ?: field("title") ?: lines.firstOrNull()
    return if (name == null) "Diagram" else "Diagram: $name"
}

/**
 * A web view that runs the diagram's page and nothing else: no network, no files, no bridge to the
 * app, no navigation, nothing to type into or select. Its ground is clear, so the reply shows through.
 */
// The page's scripts are Pele and the few lines that mount it, both shipped in the app; the source is data.
@SuppressLint("SetJavaScriptEnabled")
private fun diagramView(context: Context): WebView =
    WebView(context).apply {
        settings.javaScriptEnabled = true
        settings.blockNetworkLoads = true
        settings.allowFileAccess = false
        settings.allowContentAccess = false
        settings.javaScriptCanOpenWindowsAutomatically = false
        settings.setSupportMultipleWindows(false)
        settings.setSupportZoom(false)
        settings.builtInZoomControls = false
        setBackgroundColor(android.graphics.Color.TRANSPARENT)
        isFocusable = false
        isFocusableInTouchMode = false
        // A long press is taken here, so it cannot start a selection.
        setOnLongClickListener { true }
        isHapticFeedbackEnabled = false
        isVerticalScrollBarEnabled = false
        // The box around it names the diagram; the drawing's own text would only repeat it in pieces.
        importantForAccessibility = View.IMPORTANT_FOR_ACCESSIBILITY_NO_HIDE_DESCENDANTS
        webViewClient = Sealed
    }

/** Keeps a diagram's view where it is: nothing it asks for loads, and no link it holds goes anywhere. */
private object Sealed : WebViewClient() {
    override fun shouldOverrideUrlLoading(view: WebView, request: WebResourceRequest) = true

    override fun shouldInterceptRequest(view: WebView, request: WebResourceRequest) = WebResourceResponse(null, null, null)

    // The pages share one renderer; if it goes, each diagram becomes its code rather than the app going with it.
    override fun onRenderProcessGone(view: WebView, detail: RenderProcessGoneDetail): Boolean {
        (view.webChromeClient as? Reports)?.onReceivedTitle(view, "x")
        return true
    }
}

/** Passes what the page says through its title to [report]. */
private class Reports(var report: (DiagramReport) -> Unit) : WebChromeClient() {
    override fun onReceivedTitle(view: WebView, title: String?) {
        diagramReport(title)?.let { report(it) }
    }
}

private fun WebView.reports(report: (DiagramReport) -> Unit) {
    (webChromeClient as? Reports)?.let { it.report = report } ?: run { webChromeClient = Reports(report) }
}
