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

    @Test
    fun theSameNetworkOverOtherTransportsOrWithOtherAddressesHasMoved() {
        val networks = DefaultNetwork("vpn")
        networks.available("vpn")
        assertFalse(networks.transports("vpn", setOf("vpn", "wifi")), "the first report is what is in use")
        assertFalse(networks.addresses("vpn", setOf("10.0.0.2")))
        assertFalse(networks.transports("vpn", setOf("vpn", "wifi")), "the same again, as when only the signal changed")

        assertTrue(networks.transports("vpn", setOf("vpn", "mobile")), "Wi-Fi to mobile under the VPN")
        assertTrue(networks.addresses("vpn", setOf("10.0.0.3")), "a new address")
        assertFalse(networks.addresses("other", setOf("192.168.1.9")), "a network not in use")

        networks.available("wifi")
        assertFalse(networks.transports("wifi", setOf("wifi")), "a new network's first report is not a second move")
    }
}
