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
