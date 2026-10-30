package live.renoa.app

import android.net.ConnectivityManager
import android.net.LinkProperties
import android.net.Network
import android.net.NetworkCapabilities

/**
 * Which reports about the default network mean the phone may have moved: another network, one
 * after none, or the same network over other transports (as Wi-Fi to mobile under a VPN) or with
 * other addresses. Android reports the network in use, then its transports and addresses, as soon
 * as a callback is registered or a network comes up; those first reports are not moves. Only
 * transports and addresses are compared, so other capability changes, such as signal strength,
 * are not moves either.
 */
internal class DefaultNetwork<N : Any>(private var inUse: N?) {
    private var transports: Any? = null
    private var addresses: Any? = null

    @Synchronized
    fun available(network: N): Boolean = (network != inUse).also { if (it) use(network) }

    @Synchronized
    fun lost(network: N) {
        if (network == inUse) use(null)
    }

    @Synchronized
    fun transports(network: N, now: Any): Boolean = moved(network, transports, now).also { if (network == inUse) transports = now }

    @Synchronized
    fun addresses(network: N, now: Any): Boolean = moved(network, addresses, now).also { if (network == inUse) addresses = now }

    private fun moved(network: N, before: Any?, now: Any) = network == inUse && before != null && before != now

    private fun use(network: N?) {
        inUse = network
        transports = null
        addresses = null
    }
}

private val TRANSPORTS =
    listOf(
        NetworkCapabilities.TRANSPORT_CELLULAR,
        NetworkCapabilities.TRANSPORT_WIFI,
        NetworkCapabilities.TRANSPORT_ETHERNET,
        NetworkCapabilities.TRANSPORT_BLUETOOTH,
        NetworkCapabilities.TRANSPORT_VPN,
    )

/**
 * Calls [changed] when the phone may have moved networks, as [DefaultNetwork] decides. Returns the
 * callback to unregister, or null if Android refused, which leaves the app without the hint and
 * nothing else.
 */
internal fun watchNetwork(connectivity: ConnectivityManager, changed: () -> Unit): ConnectivityManager.NetworkCallback? =
    runCatching {
        val networks = DefaultNetwork(connectivity.activeNetwork)
        val callback =
            object : ConnectivityManager.NetworkCallback() {
                override fun onAvailable(network: Network) {
                    if (networks.available(network)) changed()
                }

                override fun onLost(network: Network) = networks.lost(network)

                override fun onCapabilitiesChanged(network: Network, capabilities: NetworkCapabilities) {
                    if (networks.transports(network, TRANSPORTS.filter { capabilities.hasTransport(it) }.toSet())) changed()
                }

                override fun onLinkPropertiesChanged(network: Network, properties: LinkProperties) {
                    if (networks.addresses(network, properties.linkAddresses.map { it.address }.toSet())) changed()
                }
            }
        connectivity.registerDefaultNetworkCallback(callback)
        callback
    }.getOrNull()
