@file:UseSerializers(StrictUuidSerializer::class)

package live.renoa.rcp

import kotlinx.serialization.ExperimentalSerializationApi
import kotlinx.serialization.SerialName
import kotlinx.serialization.Serializable
import kotlinx.serialization.UseSerializers
import kotlinx.serialization.json.JsonClassDiscriminator
import kotlinx.serialization.json.JsonElement
import kotlin.uuid.Uuid

/**
 * The records inside RCP frames, as binding 14 shapes them. Frame fields are snake_case and
 * live in [ClientMessage] and [ServerMessage]; the records below use the camelCase names the
 * binding gives them.
 */

@Serializable
data class DeviceCredentials(val deviceId: Uuid, val credential: String) {
    /** The credential is a bearer secret: printing the record must not print it. */
    override fun toString(): String = "DeviceCredentials(deviceId=$deviceId, credential=<secret>)"
}

/** A task's state, from its latest command. */
@Serializable
enum class TaskStatus {
    /** The latest command has not ended. */
    @SerialName("working")
    Working,

    /** No command, or the latest one completed or was cancelled. */
    @SerialName("idle")
    Idle,

    /** The latest command failed. */
    @SerialName("failed")
    Failed,
}

@Serializable
data class TaskSummary(val taskId: Uuid, val target: String, val status: TaskStatus)

/** An agent a node offers. [name] is what the owner calls it; [target] is its identity. */
@Serializable
data class TargetSummary(val nodeId: Uuid, val target: String, val name: String)

@Serializable
enum class StopOutcome {
    /** The stop is recorded; the command's terminal event tells whether it won. */
    @SerialName("requested")
    Requested,

    /** The command had already ended before any stop, and nothing was recorded. */
    @SerialName("already_ended")
    AlreadyEnded,
}

/** What a running turn shows before its durable `assistant_message` supersedes it. */
@Serializable
sealed interface LiveActivity {
    /** Visible text since the previous frame. [sequence] counts from zero for each message of the turn. */
    @Serializable
    @SerialName("text_delta")
    data class TextDelta(val sequence: Long, val text: String) : LiveActivity

    /** Drop what was shown for the message in progress. */
    @Serializable
    @SerialName("text_reset")
    data object TextReset : LiveActivity
}

@Serializable
enum class Author {
    /** Text written by someone other than the principal, such as a member of a shared channel. */
    @SerialName("guest")
    Guest,
}

/** The only command input the binding defines. */
@Serializable
sealed interface CommandInput {
    @Serializable
    @SerialName("text")
    data class Text(
        val text: String,
        /** Where the submitting surface received the text, in its own words. */
        val context: String? = null,
        val author: Author? = null,
    ) : CommandInput
}

@Serializable
data class CommandEnvelope(
    val commandId: Uuid,
    val principalId: Uuid,
    val surface: String,
    val target: String,
    val input: CommandInput,
)

@OptIn(ExperimentalSerializationApi::class)
@Serializable
@JsonClassDiscriminator("status")
sealed interface ExecutionTerminal {
    @Serializable
    @SerialName("completed")
    data object Completed : ExecutionTerminal

    @Serializable
    @SerialName("failed")
    data class Failed(val error: String) : ExecutionTerminal

    @Serializable
    @SerialName("cancelled")
    data class Cancelled(val reason: String) : ExecutionTerminal
}

/** Durable execution activity. It carries whole messages, never token deltas. */
@Serializable
sealed interface ExecutionEventKind {
    @Serializable
    @SerialName("execution_started")
    data object ExecutionStarted : ExecutionEventKind

    @Serializable
    @SerialName("turn_started")
    data object TurnStarted : ExecutionEventKind

    @Serializable
    @SerialName("assistant_message")
    data class AssistantMessage(val text: String) : ExecutionEventKind

    @Serializable
    @SerialName("tool_started")
    data class ToolStarted(
        @SerialName("call_id") val callId: String,
        val name: String,
        val arguments: JsonElement,
    ) : ExecutionEventKind

    @Serializable
    @SerialName("tool_finished")
    data class ToolFinished(
        @SerialName("call_id") val callId: String,
        val output: String,
        @SerialName("is_error") val isError: Boolean,
    ) : ExecutionEventKind

    @Serializable
    @SerialName("execution_terminated")
    data class ExecutionTerminated(val terminal: ExecutionTerminal) : ExecutionEventKind
}

@Serializable
data class ExecutionEvent(
    val eventId: Uuid,
    val executionId: Uuid,
    val sequence: Long,
    val recordedAtMs: Long,
    val kind: ExecutionEventKind,
)

@Serializable
sealed interface TaskEventKind {
    @Serializable
    @SerialName("command_submitted")
    data class CommandSubmitted(val command: CommandEnvelope) : TaskEventKind

    @Serializable
    @SerialName("execution_event")
    data class ExecutionRecorded(val commandId: Uuid, val event: ExecutionEvent) : TaskEventKind
}

@Serializable
data class TaskEvent(
    val eventId: Uuid,
    val taskId: Uuid,
    val sequence: Long,
    val kind: TaskEventKind,
)

/** Every error code binding 14 defines. */
@Serializable
enum class RcpErrorCode {
    @SerialName("authentication_failed")
    AuthenticationFailed,

    @SerialName("invalid_message")
    InvalidMessage,

    @SerialName("invalid_role")
    InvalidRole,

    @SerialName("node_offline")
    NodeOffline,

    @SerialName("not_found")
    NotFound,

    @SerialName("conflict")
    Conflict,

    @SerialName("internal")
    Internal,

    @SerialName("replay_required")
    ReplayRequired,

    @SerialName("version_mismatch")
    VersionMismatch,
}
