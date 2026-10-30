package live.renoa.rcp

import kotlin.time.Duration
import kotlin.time.Duration.Companion.seconds

/**
 * How long the client waits on Renoa before it counts the connection as gone, so the caller can
 * try again instead of waiting forever. A phone's Wi-Fi power save can hold packets for seconds,
 * so every bound is well above that. Each is positive and finite.
 *
 * [connect] bounds opening the socket and signing in. [reply] bounds how long an attach's replay
 * may go without an applied event, those [RcpSurface.connect] makes to re-attach included; a
 * replay cut short keeps what it applied, so the next attempt asks for less.
 *
 * [reply] also bounds a request, [hostReply] a Host call, and [probe] a [RcpSurface.probe]: once
 * Renoa has sent no frame at all for that long while the answer is due, the connection ends.
 * Frames still arriving mean Renoa is busy, as when the coordinator streams a replay before it
 * reads the next request, so the wait goes on. The coordinator waits for the Host up to 60
 * seconds before it answers a Host call (`HOST_CALL_TIMEOUT` in renoa-control, which a test
 * reads). A connection ends rather than drops one late answer, which would end it anyway as an
 * answer to no request. [close] bounds the close handshake of [RcpSurface.disconnect].
 *
 * [keepalive] is the WebSocket ping interval. A socket whose pong has not come back by the next
 * ping fails, so a half-open socket, as after a Wi-Fi or NAT change, ends within about twice this
 * even while nothing else is sent. It matches the coordinator's own 30-second ping and is meant
 * to sit well above how long power save holds a packet.
 */
class RcpTimeouts(
    val connect: Duration = 30.seconds,
    val reply: Duration = 30.seconds,
    val hostReply: Duration = 75.seconds,
    val probe: Duration = 10.seconds,
    val keepalive: Duration = 30.seconds,
    val close: Duration = 2.seconds,
) {
    init {
        val all = mapOf("connect" to connect, "reply" to reply, "hostReply" to hostReply, "probe" to probe, "keepalive" to keepalive, "close" to close)
        for ((name, limit) in all) require(limit.isPositive() && limit.isFinite()) { "RcpTimeouts.$name must be positive and finite, not $limit" }
    }
}
