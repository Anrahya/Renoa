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
const val RCP_BINDING_VERSION = 18

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
    data class AuthenticateTicket(val version: Int, val ticket: String) : ClientMessage {
        override fun toString(): String = "AuthenticateTicket(version=$version, ticket=<secret>)"
    }

    /** The answer to a [ServerMessage.DeviceChallenge]: an ECDSA signature, DER, base64url without padding. */
    @Serializable
    @SerialName("device_proof")
    data class DeviceProof(val signature: String) : ClientMessage

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

    /**
     * `afterSequence` has no default so a missing cursor is sent as `null`, as the binding requires.
     * `live` asks for the task's live activity and is always sent.
     */
    @Serializable
    @SerialName("attach")
    data class Attach(
        @SerialName("request_id") val requestId: Long,
        @SerialName("task_id") val taskId: Uuid,
        @SerialName("after_sequence") val afterSequence: Long?,
        val live: Boolean,
    ) : ClientMessage

    @Serializable
    @SerialName("stop_command")
    data class StopCommand(
        @SerialName("request_id") val requestId: Long,
        @SerialName("task_id") val taskId: Uuid,
        @SerialName("command_id") val commandId: Uuid,
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

    /** `body` has no default so a read sends `null`, as the binding shows. */
    @Serializable
    @SerialName("call_host")
    data class CallHost(
        @SerialName("request_id") val requestId: Long,
        val method: HostMethod,
        val path: String,
        val body: String?,
    ) : ClientMessage

    /** Asks for a one-time link to share [shareId], a file an agent shared. */
    @Serializable
    @SerialName("share_link")
    data class ShareLink(
        @SerialName("request_id") val requestId: Long,
        @SerialName("share_id") val shareId: Uuid,
    ) : ClientMessage

    /** Signs in the computer showing [code] as this device's owner. The code lets a computer in, so it is never printed. */
    @Serializable
    @SerialName("approve_device")
    data class ApproveDevice(
        @SerialName("request_id") val requestId: Long,
        val code: String,
    ) : ClientMessage {
        override fun toString(): String = "ApproveDevice(requestId=$requestId, code=<secret>)"
    }

    /** Registers this device's push token. [provider] is `fcm`; the token is opaque and secret. */
    @Serializable
    @SerialName("set_push_token")
    data class SetPushToken(
        @SerialName("request_id") val requestId: Long,
        val provider: String,
        val token: String,
    ) : ClientMessage {
        override fun toString(): String = "SetPushToken(requestId=$requestId, provider=$provider, token=<secret>)"
    }

    @Serializable
    @SerialName("clear_push_token")
    data class ClearPushToken(@SerialName("request_id") val requestId: Long) : ClientMessage

    @Serializable
    @SerialName("revoke_device")
    data class RevokeDevice(
        @SerialName("request_id") val requestId: Long,
        @SerialName("device_id") val deviceId: Uuid,
    ) : ClientMessage

    /** Registers the key this device's setup notices are sealed to: a 65-byte uncompressed P-256 point, base64url without padding. */
    @Serializable
    @SerialName("set_notice_key")
    data class SetNoticeKey(
        @SerialName("request_id") val requestId: Long,
        @SerialName("public_key") val publicKey: String,
    ) : ClientMessage

    @Serializable
    @SerialName("list_notices")
    data class ListNotices(@SerialName("request_id") val requestId: Long) : ClientMessage

    @Serializable
    @SerialName("acknowledge_notice")
    data class AcknowledgeNotice(
        @SerialName("request_id") val requestId: Long,
        @SerialName("notice_id") val noticeId: Uuid,
    ) : ClientMessage
}

/** Frames a surface can receive. Frames only a node receives are deliberately absent. */
@Serializable
sealed interface ServerMessage {
    /** The answer to one request, which completes the request with the same [requestId]. */
    sealed interface Answer : ServerMessage {
        val requestId: Long
    }

    @Serializable
    @SerialName("authenticated")
    data class Authenticated(val version: Int) : ServerMessage

    /** Sent instead of [Authenticated] to a device that registered a key at sign-in. */
    @Serializable
    @SerialName("device_challenge")
    data class DeviceChallenge(val nonce: String) : ServerMessage

    @Serializable
    @SerialName("task_list")
    data class TaskList(
        @SerialName("request_id") override val requestId: Long,
        val tasks: List<TaskSummary>,
    ) : ServerMessage.Answer

    @Serializable
    @SerialName("target_list")
    data class TargetList(
        @SerialName("request_id") override val requestId: Long,
        val targets: List<TargetSummary>,
    ) : ServerMessage.Answer

    @Serializable
    @SerialName("task_opened")
    data class TaskOpened(
        @SerialName("request_id") override val requestId: Long,
        @SerialName("task_id") val taskId: Uuid,
    ) : ServerMessage.Answer

    @Serializable
    @SerialName("attached")
    data class Attached(
        @SerialName("request_id") override val requestId: Long,
        @SerialName("task_id") val taskId: Uuid,
        @SerialName("through_sequence") val throughSequence: Long?,
    ) : ServerMessage.Answer

    @Serializable
    @SerialName("command_accepted")
    data class CommandAccepted(
        @SerialName("request_id") override val requestId: Long,
        @SerialName("command_id") val commandId: Uuid,
    ) : ServerMessage.Answer

    @Serializable
    @SerialName("stop_accepted")
    data class StopAccepted(
        @SerialName("request_id") override val requestId: Long,
        @SerialName("command_id") val commandId: Uuid,
        val outcome: StopOutcome,
    ) : ServerMessage.Answer

    /** Advisory text of a running turn. It has no sequence in the task and is never replayed. */
    @Serializable
    @SerialName("live_activity")
    data class LiveActivityFrame(
        @SerialName("task_id") val taskId: Uuid,
        @SerialName("command_id") val commandId: Uuid,
        val activity: LiveActivity,
    ) : ServerMessage

    @Serializable
    @SerialName("task_deleted")
    data class TaskDeleted(
        @SerialName("request_id") override val requestId: Long,
        @SerialName("task_id") val taskId: Uuid,
    ) : ServerMessage.Answer

    @Serializable
    @SerialName("host_answer")
    data class HostAnswer(
        @SerialName("request_id") override val requestId: Long,
        val status: Int,
        val body: String,
    ) : ServerMessage.Answer

    /** A one-time link: a path on Renoa's own address whose query holds a ticket, so it is never printed. */
    @Serializable
    @SerialName("share_link_issued")
    data class ShareLinkIssued(
        @SerialName("request_id") override val requestId: Long,
        val path: String,
    ) : ServerMessage.Answer {
        override fun toString(): String = "ShareLinkIssued(requestId=$requestId, path=<secret>)"
    }

    /** The computer that showed the code signs in next time it asks, under the name it gave. */
    @Serializable
    @SerialName("device_approved")
    data class DeviceApproved(
        @SerialName("request_id") override val requestId: Long,
        val surface: String,
        val label: String,
    ) : ServerMessage.Answer

    @Serializable
    @SerialName("push_token_set")
    data class PushTokenSet(@SerialName("request_id") override val requestId: Long) : ServerMessage.Answer

    @Serializable
    @SerialName("push_token_cleared")
    data class PushTokenCleared(@SerialName("request_id") override val requestId: Long) : ServerMessage.Answer

    @Serializable
    @SerialName("device_revoked")
    data class DeviceRevoked(
        @SerialName("request_id") override val requestId: Long,
        @SerialName("device_id") val deviceId: Uuid,
    ) : ServerMessage.Answer

    @Serializable
    @SerialName("notice_key_set")
    data class NoticeKeySet(@SerialName("request_id") override val requestId: Long) : ServerMessage.Answer

    @Serializable
    @SerialName("notice_list")
    data class NoticeList(
        @SerialName("request_id") override val requestId: Long,
        val notices: List<HeldNotice>,
    ) : ServerMessage.Answer

    @Serializable
    @SerialName("notice_acknowledged")
    data class NoticeAcknowledged(@SerialName("request_id") override val requestId: Long) : ServerMessage.Answer

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
