package live.renoa.app.agents

import kotlinx.serialization.Serializable
import kotlinx.serialization.json.buildJsonObject
import kotlinx.serialization.json.put
import live.renoa.rcp.Attachment
import live.renoa.rcp.HostMethod
import java.security.MessageDigest
import kotlin.io.encoding.Base64
import kotlin.uuid.Uuid

/**
 * A file the owner chose to send, read into memory and ready to upload. Its [uploadId] is chosen
 * once, so sending it again resumes the same upload rather than starting another.
 */
class OutgoingFile(val name: String, val mimeType: String, val bytes: ByteArray, val uploadId: Uuid = Uuid.random())

@Serializable
private data class UploadStatus(val size: Long, val received: Long, val complete: Boolean)

/**
 * Uploads files to the Host in parts, in order, as `docs/renoa-host-v0.md` describes. Starting
 * an upload again or sending a stored part again changes nothing, so an upload resumes from what
 * the Host says it has.
 */
internal class HostUploads(private val host: HostClient) {
    /** Uploads [file] and returns what names it in a message. */
    suspend fun upload(file: OutgoingFile): Attachment {
        if (file.bytes.size > MAX_BYTES) {
            throw RepositoryException(Problem.Invalid, "${file.name} is larger than ${MAX_BYTES / (1024 * 1024)} MB.")
        }
        val id = file.uploadId
        val attachment =
            try {
                Attachment(id, file.name, file.mimeType)
            } catch (refused: IllegalArgumentException) {
                throw RepositoryException(Problem.Invalid, "${file.name} cannot be sent: ${refused.message}")
            }
        val start =
            buildJsonObject {
                put("size", file.bytes.size)
                put("sha256", MessageDigest.getInstance("SHA-256").digest(file.bytes).toHexString())
            }
        var status = host.call(HostMethod.Put, "/v1/host/uploads/$id", start.toString(), UploadStatus.serializer())
        while (!status.complete) {
            if (status.size != file.bytes.size.toLong() || status.received !in 0 until status.size || status.received % PART_BYTES != 0L) {
                throw RepositoryException(Problem.Unavailable, "Renoa answered the upload of ${file.name} with a place it cannot be at.")
            }
            val offset = status.received.toInt()
            val part = file.bytes.copyOfRange(offset, minOf(offset + PART_BYTES, file.bytes.size))
            val body = buildJsonObject { put("data", Base64.encode(part)) }
            status = host.call(HostMethod.Put, "/v1/host/uploads/$id/parts/${offset / PART_BYTES}", body.toString(), UploadStatus.serializer())
        }
        return attachment
    }

    companion object {
        /** The Host's part size: every part but the last is this long. */
        const val PART_BYTES = 512 * 1024

        /** The largest file the Host takes. */
        const val MAX_BYTES = 50 * PART_BYTES
    }
}
