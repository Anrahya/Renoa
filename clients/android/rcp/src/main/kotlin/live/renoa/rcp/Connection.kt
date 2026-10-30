package live.renoa.rcp

import kotlinx.coroutines.CompletableDeferred
import kotlinx.coroutines.channels.Channel
import okhttp3.OkHttpClient
import okhttp3.Request
import okhttp3.Response
import okhttp3.WebSocket
import okhttp3.WebSocketListener
import okio.ByteString
import java.util.concurrent.ConcurrentHashMap
import kotlin.reflect.KClass
import kotlin.time.Duration
import kotlin.time.Duration.Companion.seconds

/**
 * How long the client waits on Renoa before it counts the connection as gone, so the caller can
 * try again instead of waiting forever. A phone's Wi-Fi power save can hold packets for seconds,
 * so every bound is well above that.
 *
 * [connect] bounds opening the socket and signing in. [reply] bounds each request's answer and
 * each attach's replay, those [RcpSurface.connect] makes to re-attach included; a replay cut short
 * keeps what it applied, so the next attempt asks for less. [hostReply] bounds a Host call, on
 * which the coordinator waits for the Host up to 60 seconds before it answers (`HOST_CALL_TIMEOUT`
 * in renoa-control). A request not answered in time ends the connection: Renoa answers every
 * request once, so the connection is no longer trusted, and a late answer would end it anyway as
 * an answer to no request.
 *
 * [keepalive] is the WebSocket ping interval. A socket whose pong has not come back by the next
 * ping fails, so a half-open socket, as after a Wi-Fi or NAT change, ends within twice this even
 * while nothing else is sent. It matches the coordinator's own 30-second ping, and a pong would
 * have to be held for the whole interval, far longer than power save holds one, to end a socket
 * that still works.
 */
class RcpTimeouts(
    val connect: Duration = 30.seconds,
    val reply: Duration = 30.seconds,
    val hostReply: Duration = 75.seconds,
    val keepalive: Duration = 30.seconds,
)

/** Everything the socket can tell the client, in the order it happened. */
internal sealed interface Inbound {
    data object Opened : Inbound

    data class Text(val text: String) : Inbound

    data object Binary : Inbound

    data class Closed(val code: Int, val reason: String) : Inbound

    data class Failed(val cause: Throwable) : Inbound
}

/** A request waiting for its response. [onReceive] runs in the receive loop, before the next frame. */
internal class Pending(
    val expected: KClass<out ServerMessage>,
    val onReceive: (ServerMessage) -> Unit,
    val response: CompletableDeferred<ServerMessage> = CompletableDeferred(),
)

/** One WebSocket and the requests in flight on it. [ended] holds the exact reason it ended. */
internal class Connection(val socket: WebSocket, val inbound: Channel<Inbound>) {
    val pending = ConcurrentHashMap<Long, Pending>()
    val opened = CompletableDeferred<Unit>()
    val authenticated = CompletableDeferred<Unit>()

    /** Whether the coordinator has asked for the device-key proof; it asks at most once. Read on the receive loop only. */
    var challenged = false
    val ended = CompletableDeferred<Throwable>()
}

internal fun openConnection(http: OkHttpClient, endpoint: String): Connection {
    val inbound = Channel<Inbound>(Channel.UNLIMITED)
    val listener =
        object : WebSocketListener() {
            override fun onOpen(webSocket: WebSocket, response: Response) {
                inbound.trySend(Inbound.Opened)
            }

            override fun onMessage(webSocket: WebSocket, text: String) {
                inbound.trySend(Inbound.Text(text))
            }

            override fun onMessage(webSocket: WebSocket, bytes: ByteString) {
                inbound.trySend(Inbound.Binary)
            }

            override fun onClosing(webSocket: WebSocket, code: Int, reason: String) {
                inbound.trySend(Inbound.Closed(code, reason))
            }

            override fun onClosed(webSocket: WebSocket, code: Int, reason: String) {
                inbound.trySend(Inbound.Closed(code, reason))
            }

            override fun onFailure(webSocket: WebSocket, t: Throwable, response: Response?) {
                inbound.trySend(Inbound.Failed(t))
            }
        }
    val socket = http.newWebSocket(Request.Builder().url(endpoint).build(), listener)
    return Connection(socket, inbound)
}

private val LOOPBACK_HOSTS = setOf("localhost", "127.0.0.1", "[::1]", "::1")

/**
 * Credentials are sent in the first frame, so the endpoint must be `wss://`. Plain `ws://` is
 * accepted only for a loopback host, which is the one place the coordinator accepts it too.
 */
internal fun requireSecureEndpoint(endpoint: String) {
    val url = Request.Builder().url(endpoint).build().url
    val secure = url.scheme == "https"
    val loopback = url.host in LOOPBACK_HOSTS
    require(secure || loopback) {
        "RCP endpoint \"$endpoint\" would send credentials in plain text. Use a wss:// endpoint; ws:// is only allowed for localhost."
    }
}
