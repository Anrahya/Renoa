@file:UseSerializers(StrictUuidSerializer::class)

package live.renoa.rcp

import kotlinx.serialization.KSerializer
import kotlinx.serialization.SerialName
import kotlinx.serialization.Serializable
import kotlinx.serialization.UseSerializers
import kotlinx.serialization.descriptors.PrimitiveKind
import kotlinx.serialization.descriptors.PrimitiveSerialDescriptor
import kotlinx.serialization.descriptors.SerialDescriptor
import kotlinx.serialization.encoding.Decoder
import kotlinx.serialization.encoding.Encoder
import kotlinx.serialization.json.Json
import kotlin.uuid.Uuid

/** The binding version this client speaks. The server rejects any other value. */
const val RCP_BINDING_VERSION = 13

/** The largest application message the coordinator accepts, in bytes. */
const val MAX_FRAME_BYTES = 1 shl 20

/** UUIDs on the wire are lowercase and hyphenated, and nothing else is accepted. */
object StrictUuidSerializer : KSerializer<Uuid> {
    override val descriptor: SerialDescriptor =
        PrimitiveSerialDescriptor("live.renoa.rcp.Uuid", PrimitiveKind.STRING)

    override fun serialize(encoder: Encoder, value: Uuid) = encoder.encodeString(value.toString())

    override fun deserialize(decoder: Decoder): Uuid {
        val text = decoder.decodeString()
        val parsed = Uuid.parseHexDash(text)
        require(parsed.toString() == text) { "UUID must be lowercase and hyphenated, got \"$text\"" }
        return parsed
    }
}

/** Frames a surface sends. Field names are the binding's snake_case. */
@Serializable
sealed interface ClientMessage {
    @Serializable
    @SerialName("authenticate")
    data class Authenticate(val version: Int, val credentials: DeviceCredentials) : ClientMessage

    @Serializable
    @SerialName("authenticate_ticket")
    data class AuthenticateTicket(val version: Int, val ticket: String) : ClientMessage

    @Serializable
    @SerialName("list_tasks")
    data class ListTasks(@SerialName("request_id") val requestId: Long) : ClientMessage

    @Serializable
    @SerialName("list_targets")
    data class ListTargets(@SerialName("request_id") val requestId: Long) : ClientMessage

    @Serializable
    @SerialName("open_task")
    data class OpenTask(
        @SerialName("request_id") val requestId: Long,
        @SerialName("task_id") val taskId: Uuid,
        @SerialName("node_id") val nodeId: Uuid,
        val target: String,
    ) : ClientMessage

    /** `afterSequence` has no default so a missing cursor is sent as `null`, as the binding requires. */
    @Serializable
    @SerialName("attach")
    data class Attach(
        @SerialName("request_id") val requestId: Long,
        @SerialName("task_id") val taskId: Uuid,
        @SerialName("after_sequence") val afterSequence: Long?,
    ) : ClientMessage

    @Serializable
    @SerialName("submit")
    data class Submit(
        @SerialName("request_id") val requestId: Long,
        @SerialName("task_id") val taskId: Uuid,
        @SerialName("command_id") val commandId: Uuid,
        val input: CommandInput,
    ) : ClientMessage

    @Serializable
    @SerialName("delete_task")
    data class DeleteTask(
        @SerialName("request_id") val requestId: Long,
        @SerialName("task_id") val taskId: Uuid,
    ) : ClientMessage
}

/** Frames a surface can receive. Frames only a node receives are deliberately absent. */
@Serializable
sealed interface ServerMessage {
    @Serializable
    @SerialName("authenticated")
    data class Authenticated(val version: Int) : ServerMessage

    @Serializable
    @SerialName("task_list")
    data class TaskList(
        @SerialName("request_id") val requestId: Long,
        val tasks: List<TaskSummary>,
    ) : ServerMessage

    @Serializable
    @SerialName("target_list")
    data class TargetList(
        @SerialName("request_id") val requestId: Long,
        val targets: List<TargetSummary>,
    ) : ServerMessage

    @Serializable
    @SerialName("task_opened")
    data class TaskOpened(
        @SerialName("request_id") val requestId: Long,
        @SerialName("task_id") val taskId: Uuid,
    ) : ServerMessage

    @Serializable
    @SerialName("attached")
    data class Attached(
        @SerialName("request_id") val requestId: Long,
        @SerialName("task_id") val taskId: Uuid,
        @SerialName("through_sequence") val throughSequence: Long?,
    ) : ServerMessage

    @Serializable
    @SerialName("command_accepted")
    data class CommandAccepted(
        @SerialName("request_id") val requestId: Long,
        @SerialName("command_id") val commandId: Uuid,
    ) : ServerMessage

    @Serializable
    @SerialName("task_deleted")
    data class TaskDeleted(
        @SerialName("request_id") val requestId: Long,
        @SerialName("task_id") val taskId: Uuid,
    ) : ServerMessage

    @Serializable
    @SerialName("task_event")
    data class TaskEventFrame(val event: TaskEvent) : ServerMessage

    @Serializable
    @SerialName("error")
    data class Error(
        @SerialName("request_id") val requestId: Long?,
        val code: RcpErrorCode,
        val message: String,
    ) : ServerMessage
}

internal val wireJson = Json {
    classDiscriminator = "type"
    ignoreUnknownKeys = true
}

fun encodeClientMessage(message: ClientMessage): String =
    wireJson.encodeToString(ClientMessage.serializer(), message)

/** Decodes one frame, or throws [ProtocolViolation] naming what was wrong with it. */
fun decodeServerMessage(text: String): ServerMessage =
    try {
        wireJson.decodeFromString(ServerMessage.serializer(), text)
    } catch (failure: IllegalArgumentException) {
        throw ProtocolViolation("server frame is not a binding $RCP_BINDING_VERSION surface frame: ${failure.message}", failure)
    }
