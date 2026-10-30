package live.renoa.app

import kotlin.test.Test
import kotlin.test.assertFalse
import kotlin.test.assertTrue

class DefaultNetworkTest {
    @Test
    fun onlyAnotherNetworkOrOneAfterNoneIsNew() {
        val networks = DefaultNetwork("wifi")
        assertFalse(networks.available("wifi"), "the report Android makes on registration")
        assertTrue(networks.available("mobile"), "Wi-Fi to mobile")
        assertFalse(networks.available("mobile"))

        networks.lost("wifi")
        assertFalse(networks.available("mobile"), "losing a network not in use changes nothing")
        networks.lost("mobile")
        assertTrue(networks.available("mobile"), "back after none")

        assertTrue(DefaultNetwork<String>(null).available("wifi"), "online after starting offline")
    }
}
