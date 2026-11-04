package live.renoa.rcp

private val TICKET_PATTERN = Regex("^[0-9a-fA-F]{64}$")
private val NONCE_PATTERN = Regex("^[0-9a-f]{64}$")

/** The first frame of a connection: who this client is, by device credential or by one-use ticket. */
internal suspend fun identityFrame(authentication: Authentication): String =
    encodeClientMessage(
        when (authentication) {
            is Authentication.Device -> ClientMessage.Authenticate(RCP_BINDING_VERSION, authentication.credentials)
            is Authentication.Ticket -> {
                val ticket = authentication.get()
                require(TICKET_PATTERN.matches(ticket)) { "connection ticket must be 64 hexadecimal characters" }
                ClientMessage.AuthenticateTicket(RCP_BINDING_VERSION, ticket)
            }
        },
    )

/** The answer to a `device_challenge`: the device key's signature over the binding's proof text. */
internal suspend fun proofFrame(authentication: Authentication, nonce: String): String {
    val device =
        authentication as? Authentication.Device
            ?: throw ProtocolViolation("a ticket connection was sent a device_challenge")
    if (!NONCE_PATTERN.matches(nonce)) {
        throw ProtocolViolation("device_challenge nonce must be 64 lowercase hexadecimal characters")
    }
    val sign =
        device.sign
            ?: throw DeviceKeyRequired("this device registered a key at sign-in, and the client was given none; sign in again")
    return encodeClientMessage(ClientMessage.DeviceProof(base64Url(sign(deviceProofText(device.credentials.deviceId, nonce)))))
}
