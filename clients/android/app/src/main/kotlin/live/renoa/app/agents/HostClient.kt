package live.renoa.app.agents

import kotlinx.coroutines.CancellationException
import kotlinx.serialization.KSerializer
import kotlinx.serialization.json.Json
import live.renoa.rcp.HostAnswer
import live.renoa.rcp.HostMethod
import live.renoa.rcp.RcpErrorCode
import live.renoa.rcp.RcpException
import live.renoa.rcp.RcpSurface

private val hostJson = Json { ignoreUnknownKeys = true }

/** What a coordinator refusal means to the screens. */
internal fun RcpException.asProblem(): RepositoryException {
    val problem =
        when (code) {
            RcpErrorCode.NotFound -> Problem.NotFound
            RcpErrorCode.Conflict -> Problem.Conflict
            RcpErrorCode.InvalidMessage -> Problem.Invalid
            else -> Problem.Unavailable
        }
    return RepositoryException(problem, message.orEmpty())
}

/**
 * The Host's control, called over the app's RCP connection. Every failure is a
 * [RepositoryException]: a refusal names what was wrong, and anything else is [Problem.Unavailable].
 */
internal class HostClient(private val surface: RcpSurface) {
    suspend fun <T> call(method: HostMethod, path: String, body: String?, shape: KSerializer<T>): T = decode(send(method, path, body), shape, "$method $path")

    private suspend fun send(method: HostMethod, path: String, body: String?): HostAnswer =
        try {
            surface.callHost(method, path, body)
        } catch (cancelled: CancellationException) {
            throw cancelled
        } catch (refused: RcpException) {
            throw refused.asProblem()
        } catch (lost: Exception) {
            // Not connected, the connection lost, or a protocol failure that ended it.
            throw RepositoryException(Problem.Unavailable, lost.message.orEmpty())
        }

    private fun <T> decode(answer: HostAnswer, shape: KSerializer<T>, call: String): T {
        val problem =
            when (answer.status) {
                in 200..299 -> null
                404 -> Problem.NotFound
                409 -> Problem.Conflict
                400, 422 -> Problem.Invalid
                else -> Problem.Unavailable
            }
        if (problem != null) throw RepositoryException(problem, "the Host answered ${answer.status}: ${answer.body}")
        return try {
            hostJson.decodeFromString(shape, answer.body)
        } catch (malformed: IllegalArgumentException) {
            // Every decoding failure, a missing field among them, is one of these.
            throw RepositoryException(Problem.Unavailable, "the Host's answer to $call is not what it returns: ${malformed.message}")
        }
    }
}
