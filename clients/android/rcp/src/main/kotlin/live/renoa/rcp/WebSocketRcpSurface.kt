package live.renoa.rcp

import kotlinx.coroutines.CancellationException
import kotlinx.coroutines.CompletableDeferred
import kotlinx.coroutines.CoroutineScope
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.SupervisorJob
import kotlinx.coroutines.async
import kotlinx.coroutines.cancel
import kotlinx.coroutines.channels.Channel
import kotlinx.coroutines.launch
import kotlinx.coroutines.selects.select
import kotlinx.coroutines.withTimeoutOrNull
import okhttp3.OkHttpClient
import java.util.concurrent.TimeUnit
import java.util.concurrent.atomic.AtomicBoolean
import java.util.concurrent.atomic.AtomicLong
import kotlin.coroutines.CoroutineContext
import kotlin.reflect.KClass
import kotlin.time.Duration
import kotlin.uuid.Uuid

private const val MAX_SAFE_INTEGER = 9_007_199_254_740_991L

/** Replay of one attachment: complete once the event at [through] has been applied. [applied] hears each event applied before then. */
private class Replay {
    val done = CompletableDeferred<Unit>()
    val applied = Channel<Unit>(Channel.CONFLATED)

    @Volatile
    var through: Long? = null
}

private class TaskAttachment(val taskId: Uuid, val live: ApplyLiveActivity?, val apply: suspend (TaskEvent) -> Unit) {
    @Volatile
    var cursor: Long? = null

    @Volatile
    var replay: Replay? = null
}

/**
 * The surface client over a WebSocket, speaking [RCP_BINDING_VERSION].
 *
 * Frames are handled one at a time, in arrival order, on [context]; [attach] callbacks run
 * there, and so are [ApplyLiveActivity] callbacks. Retry timing after a disconnect is the caller's policy: this client never reconnects
 * on its own. [connect] re-attaches every task that finished replay on this instance.
 *
 * Each wait for an answer from Renoa fails once it is overdue by [timeouts]: [connect]'s sign-in,
 * then each re-attach on its own; a request once Renoa has sent nothing at all for its deadline;
 * a replay once it stops moving. [awaitDisconnect] waits as long as the connection lives: the
 * keepalive ends a dead socket, and [probe] finds one sooner. Fetching a ticket, before the
 * socket opens, is the caller's to bound.
 */
class WebSocketRcpSurface(
    private val endpoint: String,
    private val authentication: Authentication,
    private val state: RcpState,
    private val http: OkHttpClient = OkHttpClient(),
    context: CoroutineContext = Dispatchers.Default,
    private val newCommandId: () -> Uuid = { Uuid.random() },
    private val timeouts: RcpTimeouts = RcpTimeouts(),
) : RcpSurface {
    private val scope = CoroutineScope(SupervisorJob() + context)

    // Credentials go in the first frame after the upgrade, so a redirect could carry them to a
    // host, or a plain-text scheme, that requireSecureEndpoint never saw. Redirects fail the upgrade.
    private val socketClient =
        http.newBuilder()
            .followRedirects(false)
            .followSslRedirects(false)
            .pingInterval(timeouts.keepalive.inWholeMilliseconds, TimeUnit.MILLISECONDS)
            .build()

    private val attachments = LinkedHashMap<Uuid, TaskAttachment>()
    private val nextRequestId = AtomicLong(1)
    private val connecting = AtomicBoolean(false)

    @Volatile
    private var current: Connection? = null

    @Volatile
    private var latest: Connection? = null

    @Volatile
    private var closed = false

    init {
        requireSecureEndpoint(endpoint)
    }

    override suspend fun connect() {
        check(!closed) { "RCP client is closed" }
        check(connecting.compareAndSet(false, true)) { "RCP client is already connecting" }
        try {
            check(current == null) { "RCP client is already connected" }
            // The last connection's loop may still be applying the frame it held when it ended;
            // its cursor must be stored before this one asks to replay from it.
            latest?.reader?.join()
            val identity = identityFrame(authentication)
            val connection = openConnection(socketClient, endpoint)
            current = connection
            latest = connection
            // close() sets closed and then reads current; this sets current and then reads closed,
            // so a close that raced with this connect ends the connection one way or the other.
            if (closed) end(connection, Disconnected("RCP client closed"), 1000)
            connection.reader = scope.launch { receive(connection) }
            try {
                val signedIn =
                    withTimeoutOrNull(timeouts.connect) {
                        connection.opened.await()
                        send(connection, identity)
                        connection.authenticated.await()
                    }
                if (signedIn == null) {
                    val late = Disconnected("Renoa did not finish connecting within ${timeouts.connect}")
                    end(connection, late, null)
                    throw late
                }
                for (attachment in synchronized(attachments) { attachments.values.toList() }) {
                    reattach(attachment)
                }
            } catch (failure: Throwable) {
                end(connection, failure as? Exception ?: Disconnected("connect was cancelled", failure), 1000)
                throw failure
            }
        } finally {
            connecting.set(false)
        }
    }

    override suspend fun disconnect() {
        val connection = current ?: return
        connection.socket.close(1000, "client disconnect")
        withTimeoutOrNull(timeouts.close) { connection.ended.await() }
            ?: end(connection, Disconnected("the server did not finish the close handshake"), 1000)
    }

    override suspend fun awaitDisconnect(): Throwable =
        checkNotNull(latest) { "RCP client has not connected" }.ended.await()

    override suspend fun probe() {
        request(ServerMessage.TargetList::class, { ClientMessage.ListTargets(it) }, timeouts.probe)
    }

    override suspend fun listTasks(): List<TaskSummary> =
        request(ServerMessage.TaskList::class, { ClientMessage.ListTasks(it) }).tasks

    override suspend fun listTargets(): List<TargetSummary> =
        request(ServerMessage.TargetList::class, { ClientMessage.ListTargets(it) }).targets

    override suspend fun openTask(taskId: Uuid, nodeId: Uuid, target: String) {
        request(ServerMessage.TaskOpened::class, { ClientMessage.OpenTask(it, taskId, nodeId, target) }) {
            if (it.taskId != taskId) throw ProtocolViolation("open_task response changed the task identity")
        }
    }

    override suspend fun deleteTask(taskId: Uuid) {
        request(ServerMessage.TaskDeleted::class, { ClientMessage.DeleteTask(it, taskId) }) {
            if (it.taskId != taskId) throw ProtocolViolation("delete_task response changed the task identity")
        }
        synchronized(attachments) { attachments.remove(taskId) }
        // Its unsent commands can never be admitted now, and the first of them would block every retry behind it.
        for (command in state.pendingCommands()) if (command.taskId == taskId) state.removeCommand(command.commandId)
    }

    override suspend fun attach(taskId: Uuid, live: ApplyLiveActivity?, apply: suspend (TaskEvent) -> Unit) {
        connected()
        val attachment = TaskAttachment(taskId, live, apply)
        synchronized(attachments) {
            check(taskId !in attachments) { "task $taskId is already attached" }
            attachments[taskId] = attachment
        }
        try {
            attachExisting(attachment)
        } catch (failure: Throwable) {
            synchronized(attachments) { attachments.remove(taskId) }
            throw failure
        }
    }

    override suspend fun submitText(taskId: Uuid, text: String, files: List<Attachment>): CommandSubmission {
        connected()
        require(files.size <= Attachment.MAX_PER_COMMAND) { "A message carries at most ${Attachment.MAX_PER_COMMAND} files." }
        val command = PendingTextCommand(newCommandId(), taskId, text, files)
        requireFits(command)
        state.enqueueCommand(command)
        return CommandSubmission(command.commandId, scope.async { sendPending(command) })
    }

    override suspend fun stopCommand(taskId: Uuid, commandId: Uuid): StopOutcome =
        request(ServerMessage.StopAccepted::class, { ClientMessage.StopCommand(it, taskId, commandId) }) {
            if (it.commandId != commandId) throw ProtocolViolation("stop answer changed the command identity")
        }.outcome

    override suspend fun callHost(method: HostMethod, path: String, body: String?): HostAnswer =
        request(ServerMessage.HostAnswer::class, { ClientMessage.CallHost(it, method, path, body) }, timeouts.hostReply)
            .let { HostAnswer(it.status, it.body) }

    override suspend fun shareLink(shareId: Uuid): String =
        request(ServerMessage.ShareLinkIssued::class, { ClientMessage.ShareLink(it, shareId) }).path

    override suspend fun approveDevice(code: String): ApprovedDevice =
        request(ServerMessage.DeviceApproved::class, { ClientMessage.ApproveDevice(it, code) }).let { ApprovedDevice(it.surface, it.label) }

    override suspend fun setPushToken(token: String) {
        request(ServerMessage.PushTokenSet::class, { ClientMessage.SetPushToken(it, "fcm", token) })
    }

    override suspend fun clearPushToken() {
        request(ServerMessage.PushTokenCleared::class, { ClientMessage.ClearPushToken(it) })
    }

    override suspend fun revokeDevice(deviceId: Uuid) {
        request(ServerMessage.DeviceRevoked::class, { ClientMessage.RevokeDevice(it, deviceId) })
    }

    override suspend fun setNoticeKey(publicKey: ByteArray) {
        require(publicKey.size == 65 && publicKey[0] == 0x04.toByte()) { "a notice key is a 65-byte uncompressed P-256 point" }
        request(ServerMessage.NoticeKeySet::class, { ClientMessage.SetNoticeKey(it, base64Url(publicKey)) })
    }

    override suspend fun listNotices(): List<HeldNotice> =
        request(ServerMessage.NoticeList::class, { ClientMessage.ListNotices(it) }).notices

    override suspend fun acknowledgeNotice(noticeId: Uuid) {
        request(ServerMessage.NoticeAcknowledged::class, { ClientMessage.AcknowledgeNotice(it, noticeId) })
    }

    override suspend fun resendPending(command: PendingTextCommand) {
        connected()
        if (state.pendingCommands().none { it.commandId == command.commandId }) return
        sendPending(command)
    }

    override fun close() {
        closed = true
        current?.let { end(it, Disconnected("RCP client closed"), 1000) }
        scope.cancel()
    }

    private suspend fun sendPending(command: PendingTextCommand) {
        val input = CommandInput.Text(command.text, attachments = command.attachments)
        request(
            ServerMessage.CommandAccepted::class,
            { ClientMessage.Submit(it, command.taskId, command.commandId, input) },
        ) {
            if (it.commandId != command.commandId) throw ProtocolViolation("command admission changed the command identity")
        }
        state.removeCommand(command.commandId)
    }

    /** A frame the coordinator would refuse must not reach the outbox: it could never be admitted. */
    private fun requireFits(command: PendingTextCommand) {
        val worstCase =
            ClientMessage.Submit(
                MAX_SAFE_INTEGER,
                command.taskId,
                command.commandId,
                CommandInput.Text(command.text, attachments = command.attachments),
            )
        val bytes = encodeClientMessage(worstCase).toByteArray(Charsets.UTF_8).size
        require(bytes <= MAX_FRAME_BYTES) {
            "text is $bytes bytes once encoded; one RCP message holds at most $MAX_FRAME_BYTES. Send shorter text."
        }
    }

    private fun connected(): Connection {
        val connection = current
        check(connection != null && connection.authenticated.isCompleted && !connection.ended.isCompleted) {
            "RCP client is not connected"
        }
        return connection
    }

    private fun send(connection: Connection, frame: String) {
        if (!connection.socket.send(frame)) throw Disconnected("RCP connection is closing")
    }

    private suspend fun <R : ServerMessage> request(
        expected: KClass<R>,
        build: (Long) -> ClientMessage,
        limit: Duration = timeouts.reply,
        onReceive: (R) -> Unit = {},
    ): R {
        val connection = connected()
        val id = nextRequestId.getAndIncrement()
        check(id <= MAX_SAFE_INTEGER) { "RCP request id space exhausted" }
        val pending = Pending(expected, { onReceive(expected.javaObjectType.cast(it)) })
        connection.pending[id] = pending
        if (connection.ended.isCompleted) {
            connection.pending.remove(id)
            throw Disconnected("RCP connection ended before the request was sent", connection.ended.await())
        }
        try {
            send(connection, encodeClientMessage(build(id)))
        } catch (failure: Exception) {
            connection.pending.remove(id)
            throw failure
        }
        while (true) {
            val since = System.nanoTime()
            withTimeoutOrNull(limit) { pending.response.await() }?.let { return expected.javaObjectType.cast(it) }
            // The coordinator answers a connection's requests in order and streams a replay before
            // reading on, so frames still coming mean busy, not gone.
            if (!connection.heardSince(since)) silent(connection, "Renoa sent nothing for $limit while ${expected.simpleName} for request $id was due")
        }
    }

    /** Waits for [replay] to finish while each of its events arrives within [RcpTimeouts.reply] of the last. */
    private suspend fun awaitReplay(connection: Connection, replay: Replay, taskId: Uuid) {
        while (true) {
            val finished =
                withTimeoutOrNull(timeouts.reply) {
                    select<Boolean> {
                        replay.done.onAwait { true }
                        replay.applied.onReceive { false }
                    }
                } ?: silent(connection, "Renoa sent no more of the replay of task $taskId for ${timeouts.reply}")
            if (finished) return
        }
    }

    /** An answer that is overdue ends [connection], as [RcpTimeouts] says, so every other wait on it fails too. */
    private fun silent(connection: Connection, reason: String): Nothing {
        val silent = Disconnected(reason)
        end(connection, silent, null)
        throw silent
    }

    /** A task the coordinator no longer has would fail every later connect, so it is forgotten as the failure is reported. */
    private suspend fun reattach(attachment: TaskAttachment) {
        try {
            attachExisting(attachment)
        } catch (gone: RcpException) {
            if (gone.code == RcpErrorCode.NotFound) synchronized(attachments) { attachments.remove(attachment.taskId) }
            throw gone
        }
    }

    private suspend fun attachExisting(attachment: TaskAttachment) {
        val connection = connected()
        attachment.cursor = state.cursor(attachment.taskId)
        val replay = Replay()
        attachment.replay = replay
        request(
            ServerMessage.Attached::class,
            { ClientMessage.Attach(it, attachment.taskId, attachment.cursor, live = attachment.live != null) },
        ) {
            if (it.taskId != attachment.taskId) throw ProtocolViolation("attach response changed the task identity")
            replay.through = it.throughSequence
            val cursor = attachment.cursor
            if (it.throughSequence == null || (cursor != null && cursor >= it.throughSequence)) {
                attachment.replay = null
                replay.done.complete(Unit)
            }
        }
        awaitReplay(connection, replay, attachment.taskId)
    }

    private suspend fun receive(connection: Connection) {
        try {
            for (event in connection.inbound) {
                when (event) {
                    Inbound.Opened -> connection.opened.complete(Unit)
                    is Inbound.Text -> handle(connection, event.text)
                    Inbound.Binary -> throw ProtocolViolation("the server sent a binary frame; binding $RCP_BINDING_VERSION is text only")
                    is Inbound.Closed -> {
                        end(connection, Disconnected("RCP connection closed (${event.code} ${event.reason})"), null)
                        return
                    }
                    is Inbound.Failed -> {
                        end(connection, Disconnected("RCP connection failed: ${event.cause.message}", event.cause), null)
                        return
                    }
                }
            }
        } catch (cancelled: CancellationException) {
            end(connection, Disconnected("RCP client closed"), 1000)
            throw cancelled
        } catch (failure: Exception) {
            val code =
                when (failure) {
                    is RcpException, is DeviceKeyRequired -> 1000
                    is ProtocolViolation -> 1002
                    else -> 1011
                }
            end(connection, failure, code)
        }
    }

    private suspend fun handle(connection: Connection, text: String) {
        when (val message = decodeServerMessage(text)) {
            is ServerMessage.Authenticated -> {
                if (message.version != RCP_BINDING_VERSION || connection.authenticated.isCompleted) {
                    throw ProtocolViolation("unexpected authenticated frame with version ${message.version}")
                }
                connection.authenticated.complete(Unit)
            }
            is ServerMessage.DeviceChallenge -> prove(connection, message.nonce)
            is ServerMessage.TaskEventFrame -> applyTaskEvent(message.event)
            is ServerMessage.LiveActivityFrame -> applyLiveActivity(message)
            is ServerMessage.Error -> {
                val failure = RcpException(message.code, message.message, message.requestId)
                val id = message.requestId ?: throw failure
                val pending =
                    connection.pending.remove(id) ?: throw ProtocolViolation("error references unknown request $id")
                pending.response.completeExceptionally(failure)
            }
            is ServerMessage.Answer -> complete(connection, message.requestId, message)
        }
    }

    /** Answers the connection's one challenge with the device key's signature over the binding's proof text. */
    private suspend fun prove(connection: Connection, nonce: String) {
        if (connection.authenticated.isCompleted || connection.challenged) {
            throw ProtocolViolation("unexpected device_challenge")
        }
        connection.challenged = true
        send(connection, proofFrame(authentication, nonce))
    }

    private fun complete(connection: Connection, requestId: Long, message: ServerMessage) {
        val pending =
            connection.pending.remove(requestId)
                ?: throw ProtocolViolation("response references unknown request $requestId")
        try {
            if (!pending.expected.isInstance(message)) {
                throw ProtocolViolation("expected ${pending.expected.simpleName}, received ${message::class.simpleName}")
            }
            pending.onReceive(message)
        } catch (failure: Exception) {
            pending.response.completeExceptionally(failure)
            throw failure
        }
        pending.response.complete(message)
    }

    private suspend fun applyTaskEvent(event: TaskEvent) {
        val attachment =
            synchronized(attachments) { attachments[event.taskId] }
                ?: throw ProtocolViolation("received an event for unattached task ${event.taskId}")
        val cursor = attachment.cursor
        if (cursor != null && event.sequence <= cursor) return
        val expected = if (cursor == null) 0L else cursor + 1
        if (event.sequence != expected) {
            throw ProtocolViolation("task ${event.taskId} event sequence ${event.sequence} follows $cursor")
        }
        attachment.apply(event)
        state.advanceCursor(event.taskId, event.sequence)
        attachment.cursor = event.sequence
        val replay = attachment.replay
        val through = replay?.through
        if (replay != null && through != null && event.sequence >= through) {
            attachment.replay = null
            replay.done.complete(Unit)
        } else {
            replay?.applied?.trySend(Unit)
        }
    }

    private fun applyLiveActivity(frame: ServerMessage.LiveActivityFrame) {
        val attachment =
            synchronized(attachments) { attachments[frame.taskId] }
                ?: throw ProtocolViolation("received live activity for unattached task ${frame.taskId}")
        val live =
            attachment.live
                ?: throw ProtocolViolation("received live activity for task ${frame.taskId}, which did not ask for it")
        try {
            live(frame.commandId, frame.activity)
        } catch (ignored: Exception) {
            // Drawing a draft is the app's concern; the durable record carries the same text.
        }
    }

    private fun end(connection: Connection, reason: Throwable, closeCode: Int?) {
        // Whoever waits for the end may connect again at once, so current is cleared before ended completes.
        if (!connection.claimEnd { if (current === connection) current = null }) return
        connection.ended.complete(reason)
        connection.opened.completeExceptionally(reason)
        connection.authenticated.completeExceptionally(reason)
        for (pending in connection.pending.values) pending.response.completeExceptionally(reason)
        connection.pending.clear()
        synchronized(attachments) {
            for (attachment in attachments.values) {
                attachment.replay?.done?.completeExceptionally(reason)
                attachment.replay = null
            }
        }
        if (closeCode != null) connection.socket.close(closeCode, null) else connection.socket.cancel()
        // Frames still queued are dropped, not applied: the next connection replays from the cursor.
        connection.inbound.cancel()
    }
}
