package live.renoa.rcp

import kotlinx.serialization.json.jsonPrimitive
import kotlin.test.Test
import kotlin.test.assertEquals

/** A signed-in app can revoke itself, which is how signing out ends its sign-in on Renoa too. */
class RevokeDeviceTest : CoordinatorTest() {
    @Test
    fun aDeviceRevokesItselfAndIsAnsweredBeforeItsConnectionEnds() = eventually {
        val app = client().also { it.connect() }
        val me = coordinator.credentials.deviceId

        app.revokeDevice(me)

        assertEquals(listOf(me.toString()), coordinator.revokedDevices)
        assertEquals(me.toString(), coordinator.framesOfType("revoke_device").single().getValue("device_id").jsonPrimitive.content)
    }
}
