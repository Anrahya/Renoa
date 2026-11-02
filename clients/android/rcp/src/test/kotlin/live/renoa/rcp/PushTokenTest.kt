package live.renoa.rcp

import kotlinx.serialization.json.jsonPrimitive
import kotlin.test.Test
import kotlin.test.assertEquals
import kotlin.test.assertFailsWith
import kotlin.test.assertNull

/** A signed-in device tells Renoa where to wake it, and takes that back. */
class PushTokenTest : CoordinatorTest() {
    @Test
    fun aDeviceRegistersItsFcmTokenAndClearsIt() = eventually {
        val app = client().also { it.connect() }

        app.setPushToken("fcm-token-1")
        assertEquals("fcm-token-1", coordinator.pushToken)
        val sent = coordinator.framesOfType("set_push_token").single()
        assertEquals("fcm", sent.getValue("provider").jsonPrimitive.content, "the binding's only provider")

        app.clearPushToken()
        assertNull(coordinator.pushToken)
        app.clearPushToken()
        assertEquals(2, coordinator.framesOfType("clear_push_token").size, "clearing again is answered too")
    }

    @Test
    fun aTicketConnectionIsRefusedWithTheTypedError() = eventually {
        val ticket = "ab".repeat(32)
        coordinator.validTickets += ticket
        val browser = client(Authentication.Ticket { ticket }).also { it.connect() }

        assertEquals(RcpErrorCode.InvalidRole, assertFailsWith<RcpException> { browser.setPushToken("fcm-token-1") }.code)
        assertEquals(RcpErrorCode.InvalidRole, assertFailsWith<RcpException> { browser.clearPushToken() }.code)
        assertNull(coordinator.pushToken)
    }
}
