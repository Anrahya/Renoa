package live.renoa.rcp

/** The coordinator answered a request, or ended the session, with a typed RCP error. */
class RcpException(
    val code: RcpErrorCode,
    message: String,
    val requestId: Long?,
) : Exception("${code.name}: $message")

/** The peer sent something binding 13 does not allow. The session ends. */
class ProtocolViolation(message: String, cause: Throwable? = null) : Exception(message, cause)

/** The connection ended before the operation finished. The outcome of a write is unknown. */
class Disconnected(message: String, cause: Throwable? = null) : Exception(message, cause)
