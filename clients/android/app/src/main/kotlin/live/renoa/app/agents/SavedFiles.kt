package live.renoa.app.agents

import kotlinx.serialization.KSerializer
import kotlinx.serialization.descriptors.PrimitiveKind
import kotlinx.serialization.descriptors.PrimitiveSerialDescriptor
import kotlinx.serialization.encoding.Decoder
import kotlinx.serialization.encoding.Encoder
import java.io.File
import java.io.IOException
import java.time.Instant

/**
 * Writes [bytes] to a sibling, syncs it, then renames it over this file, so a process killed at any
 * point leaves the old contents or the new.
 */
internal fun File.replaceAtomically(bytes: ByteArray) {
    val next = replacement
    next.outputStream().use { out ->
        out.write(bytes)
        out.fd.sync()
    }
    if (!next.renameTo(this)) throw IOException("could not replace $this")
}

/** The sibling [replaceAtomically] writes before renaming it over this file. */
internal val File.replacement: File get() = File(parentFile, "$name.new")

/** An instant as milliseconds since the epoch. */
internal object InstantMillis : KSerializer<Instant> {
    override val descriptor = PrimitiveSerialDescriptor("live.renoa.app.InstantMillis", PrimitiveKind.LONG)

    override fun serialize(encoder: Encoder, value: Instant) = encoder.encodeLong(value.toEpochMilli())

    override fun deserialize(decoder: Decoder): Instant = Instant.ofEpochMilli(decoder.decodeLong())
}
