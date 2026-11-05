package live.renoa.app.chat

import android.content.ActivityNotFoundException
import android.content.ContentResolver
import android.graphics.Bitmap
import android.graphics.BitmapFactory
import android.graphics.ImageDecoder
import android.net.Uri
import android.provider.OpenableColumns
import androidx.activity.compose.rememberLauncherForActivityResult
import androidx.activity.result.PickVisualMediaRequest
import androidx.activity.result.contract.ActivityResultContracts
import androidx.compose.foundation.Image
import androidx.compose.foundation.background
import androidx.compose.foundation.clickable
import androidx.compose.foundation.horizontalScroll
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.layout.widthIn
import androidx.compose.foundation.rememberScrollState
import androidx.compose.material3.Text
import androidx.compose.material3.minimumInteractiveComponentSize
import androidx.compose.runtime.Composable
import androidx.compose.runtime.remember
import androidx.compose.runtime.rememberCoroutineScope
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.draw.clip
import androidx.compose.ui.graphics.asImageBitmap
import androidx.compose.ui.layout.ContentScale
import androidx.compose.ui.platform.LocalContext
import androidx.compose.ui.semantics.Role
import androidx.compose.ui.semantics.contentDescription
import androidx.compose.ui.semantics.semantics
import androidx.compose.ui.text.style.TextOverflow
import androidx.compose.ui.unit.dp
import androidx.core.content.FileProvider
import com.kyant.shapes.RoundedRectangle
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.launch
import kotlinx.coroutines.withContext
import live.renoa.app.agents.HostUploads
import live.renoa.app.agents.OutgoingFile
import live.renoa.design.DropdownMenu
import live.renoa.design.MenuItem
import live.renoa.design.Palette
import live.renoa.design.Symbol
import live.renoa.design.Type
import live.renoa.rcp.Attachment
import java.io.ByteArrayOutputStream
import java.io.File
import java.io.IOException
import java.io.InputStream
import kotlin.math.max
import kotlin.math.roundToInt

/**
 * The longest edge a photo is sent at. Claude's vision guidance scales anything longer down to
 * this before the model sees it, so more pixels would only cost upload time.
 */
private const val LONG_EDGE = 1568

/** How a shrunk photo is compressed: small enough to send quickly, sharp enough to read a label. */
private const val JPEG_QUALITY = 85

/** The suffix of the manifest's camera provider, after the app's package name. */
private const val CAMERA_AUTHORITY = ".camera"

/** The "+" that offers a photo from the gallery, a new one from the camera, or any file. */
@Composable
internal fun AttachButton(room: Int, onFiles: (List<OutgoingFile>) -> Unit, onProblem: (String) -> Unit) {
    val context = LocalContext.current
    val scope = rememberCoroutineScope()
    val read = { uris: List<Uri> ->
        if (uris.isNotEmpty()) {
            scope.launch {
                try {
                    onFiles(uris.take(room).map { readOutgoing(context.contentResolver, it) })
                    if (uris.size > room) onProblem("A message holds ${Attachment.MAX_PER_COMMAND} files at most.")
                } catch (unreadable: IOException) {
                    onProblem(unreadable.message ?: "That file could not be read.")
                } catch (_: SecurityException) {
                    onProblem("Renoa was not allowed to open that file.")
                }
            }
        }
    }
    val photos = rememberLauncherForActivityResult(ActivityResultContracts.PickMultipleVisualMedia(Attachment.MAX_PER_COMMAND)) { read(it) }
    val files = rememberLauncherForActivityResult(ActivityResultContracts.OpenMultipleDocuments()) { read(it) }
    // The camera app writes the photo here; the manifest's provider lends it this one file.
    val shot = { FileProvider.getUriForFile(context, context.packageName + CAMERA_AUTHORITY, File(context.cacheDir, "camera/photo.jpg").also { it.parentFile?.mkdirs() }) }
    val camera = rememberLauncherForActivityResult(ActivityResultContracts.TakePicture()) { taken -> if (taken) read(listOf(shot())) }
    DropdownMenu(
        items = {
            listOf(
                MenuItem("Photos", icon = "photo") { photos.launch(PickVisualMediaRequest(ActivityResultContracts.PickVisualMedia.ImageOnly)) },
                MenuItem("Camera", icon = "camera.fill") {
                    try {
                        camera.launch(shot())
                    } catch (_: ActivityNotFoundException) {
                        onProblem("This phone has no camera app to take a photo with.")
                    }
                },
                MenuItem("Files", icon = "doc") { files.launch(arrayOf("*/*")) },
            )
        },
        // The tap area is the 48dp round the symbol, as the send button's is, so the two sit level.
        modifier = Modifier.minimumInteractiveComponentSize().size(34.dp),
        enabled = room > 0,
        label = "Add a photo or file",
    ) { Symbol("plus", size = 22.dp, tint = if (room > 0) Palette.text else Palette.tertiary) }
}

/** The files waiting in the composer, each with a way to take it back out. */
@Composable
internal fun PickedFiles(files: List<OutgoingFile>, onRemove: (Int) -> Unit, modifier: Modifier = Modifier) {
    Row(modifier.horizontalScroll(rememberScrollState()), horizontalArrangement = Arrangement.spacedBy(8.dp)) {
        files.forEachIndexed { index, file ->
            Box {
                FileTile(file.name, file.mimeType, file.bytes)
                Box(
                    Modifier.align(Alignment.TopEnd).clickable(role = Role.Button) { onRemove(index) }.semantics { contentDescription = "Remove ${file.name}" }.padding(2.dp),
                ) { Symbol("xmark.circle.fill", size = 20.dp, tint = Palette.secondary) }
            }
        }
    }
}

/** The files the owner sent with a message, above its words. */
@Composable
internal fun SentFiles(attachments: List<Attachment>) {
    Column(horizontalAlignment = Alignment.End, verticalArrangement = Arrangement.spacedBy(4.dp)) {
        attachments.forEach { FileTile(it.name, it.mimeType, bytes = null) }
    }
}

/** A photo as its picture when its bytes are here, otherwise an icon with the file's name. */
@Composable
private fun FileTile(name: String, mimeType: String, bytes: ByteArray?) {
    val picture = remember(bytes) { bytes?.takeIf { mimeType.startsWith("image/") }?.let { thumbnail(it) } }
    val shape = RoundedRectangle(14.dp)
    if (picture != null) {
        Image(picture.asImageBitmap(), name, Modifier.size(64.dp).clip(shape), contentScale = ContentScale.Crop)
        return
    }
    Row(
        Modifier.widthIn(max = 220.dp).clip(shape).background(Palette.bubbleUser).padding(horizontal = 12.dp, vertical = 10.dp),
        verticalAlignment = Alignment.CenterVertically,
        horizontalArrangement = Arrangement.spacedBy(8.dp),
    ) {
        Symbol(if (mimeType.startsWith("image/")) "photo" else "doc", size = 18.dp, tint = Palette.secondary)
        Text(name, style = Type.footnote, color = Palette.text, maxLines = 1, overflow = TextOverflow.Ellipsis)
    }
}

/** A small copy of a photo for its tile, decoded at about the tile's size. */
private fun thumbnail(bytes: ByteArray): Bitmap? {
    val bounds = BitmapFactory.Options().apply { inJustDecodeBounds = true }
    BitmapFactory.decodeByteArray(bytes, 0, bytes.size, bounds)
    var sample = 1
    while (max(bounds.outWidth, bounds.outHeight) / (sample * 2) >= 192) sample *= 2
    return BitmapFactory.decodeByteArray(bytes, 0, bytes.size, BitmapFactory.Options().apply { inSampleSize = sample })
}

/**
 * Reads the file at [uri] to send it. A photo, unless it moves, is turned upright and shrunk to
 * [LONG_EDGE] pixels on its long edge as a JPEG; anything else goes as it is, up to the Host's limit.
 */
internal suspend fun readOutgoing(resolver: ContentResolver, uri: Uri): OutgoingFile =
    withContext(Dispatchers.IO) {
        val mimeType = resolver.getType(uri)?.takeIf(Attachment::validMediaType) ?: "application/octet-stream"
        val name =
            sendableName(
                resolver.query(uri, arrayOf(OpenableColumns.DISPLAY_NAME), null, null, null)?.use { if (it.moveToFirst()) it.getString(0) else null },
            )
        if (mimeType.startsWith("image/") && mimeType != "image/gif") {
            val photo =
                try {
                    // The decoder applies the camera's orientation, so the photo arrives upright.
                    ImageDecoder.decodeBitmap(ImageDecoder.createSource(resolver, uri)) { decoder, info, _ ->
                        val long = max(info.size.width, info.size.height)
                        if (long > LONG_EDGE) {
                            val scale = LONG_EDGE.toFloat() / long
                            decoder.setTargetSize(max(1, (info.size.width * scale).roundToInt()), max(1, (info.size.height * scale).roundToInt()))
                        }
                        decoder.allocator = ImageDecoder.ALLOCATOR_SOFTWARE
                    }
                } catch (unreadable: ImageDecoder.DecodeException) {
                    throw IOException("$name could not be read as a photo.", unreadable)
                }
            val jpeg = ByteArrayOutputStream().also { photo.compress(Bitmap.CompressFormat.JPEG, JPEG_QUALITY, it) }.toByteArray()
            photo.recycle()
            return@withContext OutgoingFile(sendableName(name.substringBeforeLast('.') + ".jpg"), "image/jpeg", jpeg)
        }
        val bytes = resolver.openInputStream(uri)?.use { it.readAtMost(HostUploads.MAX_BYTES) } ?: throw IOException("$name could not be read.")
        if (bytes.isEmpty()) throw IOException("$name is empty.")
        OutgoingFile(name, mimeType, bytes)
    }

/**
 * [raw] as a name Renoa takes ([Attachment.validName]): control characters become spaces, and a
 * name too long loses the end of its stem, keeping its extension.
 */
internal fun sendableName(raw: String?): String {
    val name = raw.orEmpty().map { if (it.isISOControl()) ' ' else it }.joinToString("").trim()
    if (name.isEmpty()) return "file"
    if (Attachment.validName(name)) return name
    val extension = name.substringAfterLast('.', "").let { if (it.isEmpty() || it.length > 16) "" else ".$it" }
    var stem = name.removeSuffix(extension)
    while (stem.isNotEmpty() && !Attachment.validName(stem + extension)) stem = stem.dropLast(if (stem.last().isLowSurrogate()) 2 else 1)
    return if (stem.isEmpty()) "file" else stem + extension
}

/** The stream's bytes, refusing one longer than [limit] before reading it all. */
private fun InputStream.readAtMost(limit: Int): ByteArray {
    val out = ByteArrayOutputStream()
    val buffer = ByteArray(64 * 1024)
    while (true) {
        val read = read(buffer)
        if (read < 0) return out.toByteArray()
        if (out.size() + read > limit) throw IOException("That file is larger than ${limit / (1024 * 1024)} MB.")
        out.write(buffer, 0, read)
    }
}
