package live.renoa.app.agents

import live.renoa.rcp.DeviceKeyRequired
import live.renoa.rcp.RcpErrorCode
import live.renoa.rcp.RcpException

/** Whether the coordinator answered this request; an error with no request ends the connection. */
internal val RcpException.answered: Boolean get() = requestId != null

/**
 * Whether the coordinator answered this request with a refusal that a retry cannot change. One like
 * `node_offline` or `internal` may pass.
 */
internal val RcpException.refusesTheRequest: Boolean
    get() =
        answered &&
            when (code) {
                RcpErrorCode.InvalidMessage, RcpErrorCode.InvalidRole, RcpErrorCode.NotFound, RcpErrorCode.Conflict -> true
                RcpErrorCode.AuthenticationFailed, RcpErrorCode.NodeOffline, RcpErrorCode.Internal,
                RcpErrorCode.ReplayRequired, RcpErrorCode.VersionMismatch,
                -> false
            }

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
 * A refused sign-in: the handshake's refusal of the device credential (it carries no request id),
 * or a device key the coordinator wants and the phone no longer has.
 */
internal val Exception.signsOut: Boolean
    get() = this is DeviceKeyRequired || (this is RcpException && code == RcpErrorCode.AuthenticationFailed && requestId == null)

/** Something the app does not hold is gone once Renoa has said what there is; before that, Renoa was not reached. */
internal fun notHeld(heard: Boolean) = if (heard) Problem.NotFound else Problem.Unavailable
