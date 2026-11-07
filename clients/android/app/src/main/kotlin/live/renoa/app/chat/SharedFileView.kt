package live.renoa.app.chat

import android.annotation.SuppressLint
import android.content.ActivityNotFoundException
import android.content.ClipData
import android.content.Context
import android.content.Intent
import android.webkit.MimeTypeMap
import android.webkit.RenderProcessGoneDetail
import android.webkit.WebResourceRequest
import android.webkit.WebView
import android.webkit.WebViewClient
import androidx.compose.foundation.background
import androidx.compose.foundation.clickable
import androidx.compose.foundation.gestures.awaitEachGesture
import androidx.compose.foundation.gestures.awaitFirstDown
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.height
import androidx.compose.foundation.layout.navigationBarsPadding
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.layout.statusBarsPadding
import androidx.compose.material3.Text
import androidx.compose.material3.minimumInteractiveComponentSize
import androidx.compose.runtime.Composable
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.rememberCoroutineScope
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.input.pointer.pointerInput
import androidx.compose.ui.platform.LocalContext
import androidx.compose.ui.platform.LocalUriHandler
import androidx.compose.ui.platform.UriHandler
import androidx.compose.ui.semantics.Role
import androidx.compose.ui.semantics.contentDescription
import androidx.compose.ui.semantics.semantics
import androidx.compose.ui.text.style.TextAlign
import androidx.compose.ui.text.style.TextOverflow
import androidx.compose.ui.unit.dp
import androidx.compose.ui.viewinterop.AndroidView
import androidx.core.content.FileProvider
import com.kyant.shapes.Capsule
import kotlinx.coroutines.CancellationException
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.currentCoroutineContext
import kotlinx.coroutines.ensureActive
import kotlinx.coroutines.job
import kotlinx.coroutines.launch
import kotlinx.coroutines.withContext
import live.renoa.app.SHARED_FILES
import live.renoa.app.agents.RepositoryException
import live.renoa.app.agents.wordsOr
import live.renoa.app.signin.RENOA_ORIGIN
import live.renoa.design.Palette
import live.renoa.design.Spinner
import live.renoa.design.Symbol
import live.renoa.design.Type
import live.renoa.design.glass
import live.renoa.design.rememberPress
import live.renoa.design.semibold
import okhttp3.OkHttpClient
import okhttp3.Request
import java.io.File
import java.io.IOException
import java.net.URLDecoder
import java.util.concurrent.TimeUnit
import kotlin.uuid.Uuid

/** Where a file the phone has no page for is kept to hand to an app that opens it: one at a time. */
internal const val SHARED_FILE_DIR = "shared"

/** A file fetched to hand to another app, and its media type without parameters. */
internal class Fetched(val file: File, val type: String)

// The coordinator reads each part from the Host as it sends it, so a slow Host is slow between bytes.
private val http by lazy { OkHttpClient.Builder().readTimeout(60, TimeUnit.SECONDS).build() }

/** What the viewer shows. */
private sealed interface Showing {
    data object Asking : Showing

    data class Page(val url: String) : Showing

    /** The phone has no page for the file, so it is being fetched for another app. */
    data object Fetching : Showing

    data class Problem(val words: String) : Showing
}

/**
 * The file an agent shared as [share], over the chat, with a button that [onClose]s it. Each time it
 * opens it asks [linkTo] for a one-time link and loads it, so a rotation opens the file again
 * rather than a used link. A file a page cannot show, as a PDF, is fetched with a second link and
 * handed to an app on the phone that opens it.
 */
@Composable
internal fun SharedFileView(share: Uuid, from: String, linkTo: suspend (Uuid) -> String, onClose: () -> Unit) {
    val context = LocalContext.current
    val browser = LocalUriHandler.current
    val scope = rememberCoroutineScope()
    var showing by remember(share) { mutableStateOf<Showing>(Showing.Asking) }
    // A failure the owner reads; a cancelled screen just stops.
    val attempt: suspend (suspend () -> Unit) -> Unit = { step ->
        try {
            step()
        } catch (cancelled: CancellationException) {
            throw cancelled
        } catch (refused: RepositoryException) {
            showing = Showing.Problem(refused.wordsOr(NOT_OPENED))
        } catch (failed: IOException) {
            showing = Showing.Problem(NOT_OPENED)
        } catch (none: ActivityNotFoundException) {
            showing = Showing.Problem("No app on this phone opens this file.")
        } catch (failed: RuntimeException) {
            // As a link Renoa answered that is no address, or a file the phone cannot lend.
            showing = Showing.Problem(NOT_OPENED)
        }
    }
    LaunchedEffect(share) { attempt { showing = Showing.Page(RENOA_ORIGIN + linkTo(share)) } }
    val fetch = fetch@{
        // A second event from the same page fetches nothing more.
        if (showing == Showing.Fetching) return@fetch
        showing = Showing.Fetching
        scope.launch {
            attempt {
                val fetched = fetchShared(http, RENOA_ORIGIN + linkTo(share), File(context.cacheDir, SHARED_FILE_DIR))
                openElsewhere(context, fetched)
                onClose()
            }
        }
        Unit
    }
    // Opaque to touch: nothing reaches the chat under it.
    Column(Modifier.fillMaxSize().background(Palette.background).pointerInput(Unit) { awaitEachGesture { awaitFirstDown(requireUnconsumed = false).consume() } }.statusBarsPadding().navigationBarsPadding()) {
        Box(Modifier.fillMaxWidth().padding(top = 6.dp, bottom = 6.dp).padding(horizontal = 16.dp).height(44.dp)) {
            val (source, pressed) = rememberPress()
            Box(
                Modifier
                    .align(Alignment.CenterStart)
                    .minimumInteractiveComponentSize()
                    .size(44.dp)
                    .glass(Capsule(), pressed = pressed)
                    .clickable(source, null, role = Role.Button, onClick = onClose)
                    .semantics { contentDescription = "Close" },
                contentAlignment = Alignment.Center,
            ) { Symbol("chevron.left", size = 32.dp, tint = Palette.text) }
            // Says whose file this is, so a page cannot pass for the app around it.
            Text(
                "From $from",
                Modifier.align(Alignment.Center).padding(horizontal = 56.dp),
                style = Type.system(13f).semibold,
                color = Palette.text,
                maxLines = 1,
                overflow = TextOverflow.Ellipsis,
            )
        }
        Box(Modifier.fillMaxSize(), contentAlignment = Alignment.Center) {
            when (val now = showing) {
                Showing.Asking -> Waiting(OPENING_FILE)
                Showing.Fetching -> Waiting("Getting the file")
                is Showing.Problem -> Text(now.words, Modifier.padding(horizontal = 32.dp), style = Type.subheadline, color = Palette.secondary, textAlign = TextAlign.Center)
                is Showing.Page ->
                    AndroidView(
                        // The link works once, so the view loads it once, when it is made.
                        factory = { sharedFileView(it, browser, onDownload = fetch, onGone = { showing = Showing.Problem(NOT_OPENED) }).apply { loadUrl(now.url) } },
                        modifier = Modifier.fillMaxSize(),
                        onRelease = { it.destroy() },
                    )
            }
        }
    }
}

private const val OPENING_FILE = "Opening the file"
private const val NOT_OPENED = "This file could not be opened. Try again."

@Composable
private fun Waiting(words: String) {
    Row(verticalAlignment = Alignment.CenterVertically, horizontalArrangement = Arrangement.spacedBy(10.dp)) {
        Spinner(size = 14.dp)
        Text(words, style = Type.subheadline, color = Palette.secondary)
    }
}

/**
 * A view for one shared file. Renoa serves it in a sandbox, so its scripts may run but reach
 * nothing of Renoa's; the view lends it no file, no app content and no new windows. A file the
 * view cannot show goes to [onDownload]; see [SharedPage] for where its links go.
 */
@SuppressLint("SetJavaScriptEnabled")
private fun sharedFileView(context: Context, browser: UriHandler, onDownload: () -> Unit, onGone: () -> Unit): WebView =
    WebView(context).apply {
        settings.javaScriptEnabled = true
        settings.allowFileAccess = false
        settings.allowContentAccess = false
        settings.javaScriptCanOpenWindowsAutomatically = false
        settings.setSupportMultipleWindows(false)
        settings.builtInZoomControls = true
        settings.displayZoomControls = false
        settings.loadWithOverviewMode = true
        settings.useWideViewPort = true
        setDownloadListener { _, _, _, _, _ -> onDownload() }
        webViewClient = SharedPage(browser, onGone)
    }

/**
 * Where a shared file's page may go. The file itself loads through loadUrl, which never asks. A web
 * link the owner taps opens in the phone's [browser]; the page never leaves on its own, by a script
 * or a redirect, and nothing else leaves at all. A crashed renderer goes to [onGone] rather than
 * taking the app with it.
 */
internal class SharedPage(private val browser: UriHandler, private val onGone: () -> Unit) : WebViewClient() {
    override fun shouldOverrideUrlLoading(view: WebView, request: WebResourceRequest): Boolean {
        if (!request.isForMainFrame) return false
        if (request.hasGesture() && !request.isRedirect) WebLinks(browser).openUri(request.url.toString())
        return true
    }

    override fun onRenderProcessGone(view: WebView, detail: RenderProcessGoneDetail): Boolean {
        onGone()
        return true
    }
}

/**
 * Fetches the file at [url] into [dir], in place of the one fetched before, named as Renoa names it.
 * A body that ends short of its stated length fails and keeps nothing; cancelling stops the call.
 */
internal suspend fun fetchShared(http: OkHttpClient, url: String, dir: File): Fetched =
    withContext(Dispatchers.IO) {
        val call = http.newCall(Request.Builder().url(url).build())
        val stop = currentCoroutineContext().job.invokeOnCompletion { call.cancel() }
        try {
            call.execute().use { response ->
                if (!response.isSuccessful) throw IOException("the shared file answered ${response.code}")
                dir.deleteRecursively()
                dir.mkdirs()
                val file = File(dir, sharedName(response.header("Content-Disposition")))
                try {
                    var written = 0L
                    response.body.byteStream().use { input ->
                        file.outputStream().use { output ->
                            val buffer = ByteArray(DEFAULT_BUFFER_SIZE)
                            while (true) {
                                currentCoroutineContext().ensureActive()
                                val read = input.read(buffer)
                                if (read < 0) break
                                written += read
                                output.write(buffer, 0, read)
                            }
                        }
                    }
                    val length = response.body.contentLength()
                    if (length >= 0 && written != length) throw IOException("the shared file ended at $written of $length bytes")
                } catch (failed: Exception) {
                    file.delete()
                    throw failed
                }
                Fetched(file, mediaType(response.body.contentType()?.let { "${it.type}/${it.subtype}" }, file.name))
            }
        } finally {
            stop.dispose()
        }
    }

/**
 * The type an app opens [name] as: Renoa's, unless it is the generic one, which no app offers to
 * open, and the name's extension says better.
 */
internal fun mediaType(served: String?, name: String): String =
    served?.takeIf { it != GENERIC_TYPE }
        ?: MimeTypeMap.getSingleton().getMimeTypeFromExtension(name.substringAfterLast('.', "").lowercase())
        ?: GENERIC_TYPE

private const val GENERIC_TYPE = "application/octet-stream"
/**
 * The name Renoa gives a shared file, from its `filename*=UTF-8''…` disposition, as a name alone:
 * no folder, and never a hidden file. Without one it is "file".
 */
internal fun sharedName(disposition: String?): String {
    val encoded = disposition?.let { NAMED.find(it)?.groupValues?.get(1) } ?: return FALLBACK_NAME
    val name = runCatching { URLDecoder.decode(encoded.replace("+", "%2B"), "UTF-8") }.getOrNull() ?: return FALLBACK_NAME
    return fitted(name.substringAfterLast('/').substringAfterLast('\\').trimStart('.')).ifEmpty { FALLBACK_NAME }
}

/** [name] cut, whole characters at a time, to the 255 bytes a file system keeps for one. */
private fun fitted(name: String): String {
    var bytes = 0
    var end = 0
    while (end < name.length) {
        val next = name.offsetByCodePoints(end, 1)
        bytes += name.substring(end, next).toByteArray(Charsets.UTF_8).size
        if (bytes > MAX_NAME_BYTES) break
        end = next
    }
    return name.substring(0, end)
}

private val NAMED = Regex("filename\\*=UTF-8''([^;\\s]+)", RegexOption.IGNORE_CASE)
private const val FALLBACK_NAME = "file"
private const val MAX_NAME_BYTES = 255

/** Hands [fetched] to the app the owner opens it with. */
private fun openElsewhere(context: Context, fetched: Fetched) {
    val uri = FileProvider.getUriForFile(context, context.packageName + SHARED_FILES, fetched.file)
    val open = Intent(Intent.ACTION_VIEW).setDataAndType(uri, fetched.type).addFlags(Intent.FLAG_GRANT_READ_URI_PERMISSION)
    open.clipData = ClipData.newRawUri(fetched.file.name, uri)
    context.startActivity(open)
}
