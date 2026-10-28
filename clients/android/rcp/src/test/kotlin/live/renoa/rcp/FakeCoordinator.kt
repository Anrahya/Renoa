package live.renoa.rcp

import kotlinx.serialization.json.JsonArray
import kotlinx.serialization.json.JsonElement
import kotlinx.serialization.json.JsonNull
import kotlinx.serialization.json.JsonObject
import kotlinx.serialization.json.JsonPrimitive
import kotlinx.serialization.json.Json
import kotlinx.serialization.json.booleanOrNull
import kotlinx.serialization.json.buildJsonObject
import kotlinx.serialization.json.contentOrNull
import kotlinx.serialization.json.jsonObject
import kotlinx.serialization.json.jsonPrimitive
import kotlinx.serialization.json.long
import kotlinx.serialization.json.put
import kotlinx.serialization.json.putJsonObject
import mockwebserver3.Dispatcher
import mockwebserver3.MockResponse
import mockwebserver3.MockWebServer
import mockwebserver3.RecordedRequest
import okhttp3.Response
import okhttp3.WebSocket
import okhttp3.WebSocketListener
import java.security.PublicKey
import java.security.SecureRandom
import java.security.Signature
import java.util.Base64
import java.util.concurrent.ConcurrentHashMap
import java.util.concurrent.CopyOnWriteArrayList
import java.util.concurrent.atomic.AtomicInteger
import kotlin.uuid.Uuid

/**
 * A coordinator written from docs/rcp-json-ws-v0.md, not from the client's own models: it builds
 * and reads raw JSON, so a client that drifts from the binding cannot agree with it by accident.
 */
class FakeCoordinator : AutoCloseable {
    class TaskRecord(val nodeId: Uuid, val target: String) {
        val journal = mutableListOf<JsonObject>()
        val commands = LinkedHashMap<String, String>()
        var status = "idle"

        /** Commands whose execution has ended, and commands with a recorded stop. */
        val ended = mutableSetOf<String>()
        val stopped = mutableSetOf<String>()
    }

    val principalId: Uuid = Uuid.random()
    val credentials = DeviceCredentials(Uuid.random(), "c".repeat(64))
    val validTickets = mutableSetOf<String>()
    val nodeId: Uuid = Uuid.random()
    var nodeOnline = true
    /** Advertised targets and the names their owner gave them. */
    val advertised = linkedMapOf("agent:arcee" to "Arcee", "agent:alpha" to "Alpha")

    /** When set, the device registered this key at sign-in and must prove it on every connection. */
    @Volatile var deviceKey: PublicKey? = null

    /** When set, the challenge carries this nonce instead of a fresh one. */
    @Volatile var nonceOverride: String? = null
    val tasks = LinkedHashMap<Uuid, TaskRecord>()

    /** Every frame a client sent, in arrival order. */
    val received = CopyOnWriteArrayList<JsonObject>()
    val connectionsAccepted = AtomicInteger()

    /** When set, the next submit is admitted and journaled but its acknowledgement is never sent. */
    @Volatile var dropNextAcknowledgement = false

    /** Runs on the server thread the moment a submit frame arrives, before it is admitted. */
    @Volatile var onSubmitArrived: () -> Unit = {}

    /** Lets a test make the coordinator answer wrongly: every reply passes through it. */
    @Volatile var rewriteReply: (JsonObject) -> JsonObject = { it }

    /** When set, every connection attempt is answered with a redirect to this URL instead of an upgrade. */
    @Volatile var redirectTo: String? = null

    /** The version the coordinator claims in `authenticated`, and whether it refuses the client's version instead. */
    @Volatile var authenticatedVersion = 14
    @Volatile var refuseVersion = false

    private val sockets = CopyOnWriteArrayList<Session>()
    private val server = MockWebServer()

    val endpoint: String get() = "ws://127.0.0.1:${server.port}/connect"

    init {
        server.dispatcher =
            object : Dispatcher() {
                override fun dispatch(request: RecordedRequest): MockResponse {
                    connectionsAccepted.incrementAndGet()
                    redirectTo?.let { return MockResponse.Builder().code(307).addHeader("Location", it).build() }
                    return MockResponse.Builder().webSocketUpgrade(Session()).build()
                }
            }
        server.start()
    }

    fun seedTask(taskId: Uuid = Uuid.random(), target: String = "agent:arcee"): Uuid {
        tasks[taskId] = TaskRecord(nodeId, target)
        return taskId
    }

    /** Appends to a task's journal and pushes the event to every socket attached to it. */
    fun publish(taskId: Uuid, kind: JsonObject): JsonObject {
        val frame = append(taskId, kind)
        for (session in sockets) if (taskId in session.attached) session.socket.send(frame.toString())
        return frame
    }

    /** Sends a live frame to every socket attached to the task that asked for live activity. */
    fun publishLive(taskId: Uuid, commandId: Uuid, activity: JsonObject) {
        val frame =
            buildJsonObject {
                put("type", "live_activity")
                put("task_id", taskId.toString())
                put("command_id", commandId.toString())
                put("activity", activity)
            }
        for (session in sockets) if (taskId in session.attachedLive) session.socket.send(frame.toString())
    }

    /** Appends without pushing, as if the event had happened while the surface was offline. */
    fun append(taskId: Uuid, kind: JsonObject): JsonObject {
        val record = tasks.getValue(taskId)
        val frame =
            buildJsonObject {
                put("type", "task_event")
                putJsonObject("event") {
                    put("eventId", Uuid.random().toString())
                    put("taskId", taskId.toString())
                    put("sequence", record.journal.size.toLong())
                    put("kind", kind)
                }
            }
        record.journal += frame
        return frame
    }

    fun assistantMessage(commandId: Uuid, text: String): JsonObject =
        buildJsonObject {
            put("type", "execution_event")
            put("commandId", commandId.toString())
            putJsonObject("event") {
                put("eventId", Uuid.random().toString())
                put("executionId", Uuid.random().toString())
                put("sequence", 1L)
                put("recordedAtMs", 1_786_137_600_000L)
                putJsonObject("kind") {
                    put("type", "assistant_message")
                    put("text", text)
                }
            }
        }

    /** Sends a raw frame to every connected socket, for protocol faults the coordinator never produces itself. */
    fun injectRaw(frame: String) {
        for (session in sockets) session.socket.send(frame)
    }

    fun framesOfType(type: String): List<JsonObject> = received.filter { it["type"]?.jsonPrimitive?.contentOrNull == type }

    fun error(requestId: Long?, code: String, message: String): String =
        buildJsonObject {
            put("type", "error")
            put("request_id", requestId?.let { JsonPrimitive(it) } ?: JsonNull)
            put("code", code)
            put("message", message)
        }.toString()

    override fun close() {
        server.close()
    }

    private inner class Session : WebSocketListener() {
        lateinit var socket: WebSocket
        val attached: MutableSet<Uuid> = ConcurrentHashMap.newKeySet()
        val attachedLive: MutableSet<Uuid> = ConcurrentHashMap.newKeySet()
        private var authenticated = false
        private var nonce: String? = null

        override fun onOpen(webSocket: WebSocket, response: Response) {
            socket = webSocket
            sockets += this
        }

        override fun onClosed(webSocket: WebSocket, code: Int, reason: String) {
            sockets -= this
        }

        override fun onFailure(webSocket: WebSocket, t: Throwable, response: Response?) {
            sockets -= this
        }

        override fun onClosing(webSocket: WebSocket, code: Int, reason: String) {
            webSocket.close(code, null)
        }

        override fun onMessage(webSocket: WebSocket, text: String) {
            val frame = Json.parseToJsonElement(text).jsonObject
            received += frame
            val type = frame.getValue("type").jsonPrimitive.content
            if (type == "authenticate" || type == "authenticate_ticket") {
                authenticate(frame, type)
                return
            }
            if (type == "device_proof" && nonce != null && !authenticated) {
                verifyProof(frame)
                return
            }
            if (!authenticated) {
                webSocket.send(error(null, "authentication_failed", "authenticate first"))
                webSocket.close(1008, null)
                return
            }
            val requestId = frame.getValue("request_id").jsonPrimitive.long
            when (type) {
                "list_tasks" -> reply(requestId, "task_list") { put("tasks", tasks.entries.sortedBy { it.key.toString() }.toJsonArray { id, record -> task(id, record) }) }
                "list_targets" -> reply(requestId, "target_list") { put("targets", targetList()) }
                "open_task" -> openTask(requestId, frame)
                "delete_task" -> deleteTask(requestId, frame)
                "attach" -> attach(requestId, frame)
                "submit" -> submit(requestId, frame)
                "stop_command" -> stopCommand(requestId, frame)
                else -> webSocket.send(error(requestId, "invalid_message", "unsupported frame $type"))
            }
        }

        private fun authenticate(frame: JsonObject, type: String) {
            val version = frame.getValue("version").jsonPrimitive.long
            if (version != 14L || refuseVersion) {
                socket.send(error(null, "version_mismatch", "expected binding 14, received $version"))
                socket.close(1008, null)
                return
            }
            val valid =
                if (type == "authenticate") {
                    val credentials = frame.getValue("credentials").jsonObject
                    credentials["deviceId"]?.jsonPrimitive?.content == this@FakeCoordinator.credentials.deviceId.toString() &&
                        credentials["credential"]?.jsonPrimitive?.content == this@FakeCoordinator.credentials.credential
                } else {
                    validTickets.remove(frame.getValue("ticket").jsonPrimitive.content)
                }
            if (!valid) {
                socket.send(error(null, "authentication_failed", "credentials were not accepted"))
                socket.close(1008, null)
                return
            }
            if (type == "authenticate" && deviceKey != null) {
                val bytes = ByteArray(32).also { SecureRandom().nextBytes(it) }
                val challenge = nonceOverride ?: bytes.joinToString("") { "%02x".format(it) }
                nonce = challenge
                socket.send("""{"type":"device_challenge","nonce":"$challenge"}""")
                return
            }
            authenticated = true
            socket.send("""{"type":"authenticated","version":$authenticatedVersion}""")
        }

        /** Checks the signature over the proof text the binding defines, as the coordinator does. */
        private fun verifyProof(frame: JsonObject) {
            val text = "renoa device proof v1\n${credentials.deviceId}\n$nonce"
            val valid =
                runCatching {
                    val encoded = frame.getValue("signature").jsonPrimitive.content
                    // The coordinator decodes base64url without padding and refuses padding.
                    require('=' !in encoded)
                    val signature = Base64.getUrlDecoder().decode(encoded)
                    Signature.getInstance("SHA256withECDSA").run {
                        initVerify(deviceKey)
                        update(text.toByteArray(Charsets.UTF_8))
                        verify(signature)
                    }
                }.getOrDefault(false)
            if (!valid) {
                socket.send(error(null, "authentication_failed", "the device proof did not verify"))
                socket.close(1008, null)
                return
            }
            authenticated = true
            socket.send("""{"type":"authenticated","version":$authenticatedVersion}""")
        }

        private fun reply(requestId: Long, type: String, body: kotlinx.serialization.json.JsonObjectBuilder.() -> Unit) {
            val frame =
                buildJsonObject {
                    put("type", type)
                    put("request_id", requestId)
                    body()
                }
            socket.send(rewriteReply(frame).toString())
        }

        private fun targetList(): JsonArray =
            JsonArray(
                if (nodeOnline) {
                    advertised.keys.sorted().map { target ->
                        buildJsonObject {
                            put("nodeId", nodeId.toString())
                            put("target", target)
                            put("name", advertised.getValue(target))
                        }
                    }
                } else {
                    emptyList()
                },
            )

        private fun task(id: Uuid, record: TaskRecord): JsonObject =
            buildJsonObject {
                put("taskId", id.toString())
                put("target", record.target)
                put("status", record.status)
            }

        private fun openTask(requestId: Long, frame: JsonObject) {
            val taskId = Uuid.parse(frame.getValue("task_id").jsonPrimitive.content)
            val target = frame.getValue("target").jsonPrimitive.content
            when {
                frame.getValue("node_id").jsonPrimitive.content != nodeId.toString() || target !in advertised ->
                    socket.send(error(requestId, "not_found", "no such node or target"))
                !nodeOnline -> socket.send(error(requestId, "node_offline", "the node is offline"))
                tasks[taskId]?.target?.let { it != target } == true ->
                    socket.send(error(requestId, "conflict", "task identity already bound differently"))
                else -> {
                    tasks.getOrPut(taskId) { TaskRecord(nodeId, target) }
                    reply(requestId, "task_opened") { put("task_id", taskId.toString()) }
                }
            }
        }

        private fun deleteTask(requestId: Long, frame: JsonObject) {
            val taskId = Uuid.parse(frame.getValue("task_id").jsonPrimitive.content)
            tasks.remove(taskId)
            reply(requestId, "task_deleted") { put("task_id", taskId.toString()) }
        }

        private fun attach(requestId: Long, frame: JsonObject) {
            val taskId = Uuid.parse(frame.getValue("task_id").jsonPrimitive.content)
            val record = tasks[taskId] ?: run {
                socket.send(error(requestId, "not_found", "no such task"))
                return
            }
            val live = frame["live"]?.jsonPrimitive?.booleanOrNull ?: run {
                socket.send(error(requestId, "invalid_message", "a version 14 attach carries live"))
                return
            }
            val after = frame.getValue("after_sequence").let { if (it is JsonNull) -1L else it.jsonPrimitive.long }
            val through = record.journal.size - 1L
            reply(requestId, "attached") { put("task_id", taskId.toString()); put("through_sequence", if (through < 0) JsonNull else JsonPrimitive(through)) }
            for (event in record.journal) {
                if (event.getValue("event").jsonObject.getValue("sequence").jsonPrimitive.long > after) socket.send(event.toString())
            }
            attached += taskId
            if (live) attachedLive += taskId
        }

        private fun stopCommand(requestId: Long, frame: JsonObject) {
            val taskId = Uuid.parse(frame.getValue("task_id").jsonPrimitive.content)
            val commandId = frame.getValue("command_id").jsonPrimitive.content
            val record = tasks[taskId]
            if (record == null || commandId !in record.commands) {
                socket.send(error(requestId, "not_found", "no such command"))
                return
            }
            val outcome =
                when {
                    commandId in record.stopped -> "requested"
                    commandId in record.ended -> "already_ended"
                    else -> "requested".also { record.stopped += commandId }
                }
            reply(requestId, "stop_accepted") { put("command_id", commandId); put("outcome", outcome) }
        }

        private fun submit(requestId: Long, frame: JsonObject) {
            onSubmitArrived()
            val taskId = Uuid.parse(frame.getValue("task_id").jsonPrimitive.content)
            val commandId = frame.getValue("command_id").jsonPrimitive.content
            val text = frame.getValue("input").jsonObject.getValue("text").jsonPrimitive.content
            val record = tasks[taskId] ?: run {
                socket.send(error(requestId, "not_found", "no such task"))
                return
            }
            val existing = record.commands[commandId]
            if (existing != null && existing != text) {
                socket.send(error(requestId, "conflict", "command id was already used with different content"))
                return
            }
            if (existing == null) {
                record.commands[commandId] = text
                publish(
                    taskId,
                    buildJsonObject {
                        put("type", "command_submitted")
                        putJsonObject("command") {
                            put("commandId", commandId)
                            put("principalId", principalId.toString())
                            put("surface", "android")
                            put("target", record.target)
                            putJsonObject("input") {
                                put("type", "text")
                                put("text", text)
                            }
                        }
                    },
                )
            }
            if (dropNextAcknowledgement) {
                dropNextAcknowledgement = false
                socket.cancel()
                return
            }
            reply(requestId, "command_accepted") { put("command_id", commandId) }
        }

        private fun <K, V> Collection<Map.Entry<K, V>>.toJsonArray(item: (K, V) -> JsonElement): JsonArray =
            JsonArray(map { item(it.key, it.value) })
    }

}
