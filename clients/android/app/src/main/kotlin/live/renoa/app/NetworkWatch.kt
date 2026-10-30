package live.renoa.app

import android.net.ConnectivityManager
import android.net.Network

/**
 * Which default networks are new to the app: one after another, or one after none. Android also
 * reports the network already in use as soon as a callback is registered; that one is not new.
 */
internal class DefaultNetwork<N : Any>(private var inUse: N?) {
    @Synchronized
    fun available(network: N): Boolean = (network != inUse).also { inUse = network }

    @Synchronized
    fun lost(network: N) {
        if (network == inUse) inUse = null
    }
}

/**
 * Calls [changed] when the phone moves to another network, or onto one after none. Returns the
 * callback to unregister, or null if Android refused the registration, which leaves the app
 * without the hint and nothing else.
 */
internal fun watchNetwork(connectivity: ConnectivityManager, changed: () -> Unit): ConnectivityManager.NetworkCallback? {
    val networks = DefaultNetwork(connectivity.activeNetwork)
    val callback =
        object : ConnectivityManager.NetworkCallback() {
            override fun onAvailable(network: Network) {
                if (networks.available(network)) changed()
            }

            override fun onLost(network: Network) = networks.lost(network)
        }
    return runCatching { connectivity.registerDefaultNetworkCallback(callback) }.map { callback }.getOrNull()
}
