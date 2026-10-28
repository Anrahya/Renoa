package live.renoa.rcp

/** The coordinator answered a request, or ended the session, with a typed RCP error. */
class RcpException(
    val code: RcpErrorCode,
    message: String,
    val requestId: Long?,
) : Exception("${code.name}: $message")

/** The peer sent something binding 14 does not allow. The session ends. */
class ProtocolViolation(message: String, cause: Throwable? = null) : Exception(message, cause)

/**
 * The coordinator asked this device to prove a key it registered at sign-in, and the client was
 * given no key to prove it with. Signing in again registers a new key.
 */
class DeviceKeyRequired(message: String) : Exception(message)

/** The connection ended before the operation finished. The outcome of a write is unknown. */
class Disconnected(message: String, cause: Throwable? = null) : Exception(message, cause)
