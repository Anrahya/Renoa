package live.renoa.app.chat

import android.content.Context
import android.net.wifi.WifiManager
import android.os.Build
import androidx.compose.runtime.Composable
import androidx.compose.runtime.DisposableEffect
import androidx.compose.ui.platform.LocalContext

/**
 * Asks Android to keep Wi-Fi out of power save while the chat is on screen. Idle, a phone's Wi-Fi
 * can hold incoming packets for seconds and let them through in a burst, so a streaming reply stalls
 * mid-word until the owner touches the screen. Android's rule is that it honours the lock only while
 * the app is in front with the screen on; before Android 10 there is no low-latency lock, so none is
 * taken. A lock that cannot be taken is skipped: the chat opens either way.
 */
@Composable
fun QuickWifi() {
    val context = LocalContext.current
    DisposableEffect(context) {
        val wifi = context.applicationContext.getSystemService(Context.WIFI_SERVICE) as WifiManager?
        val lock =
            if (wifi != null && Build.VERSION.SDK_INT >= Build.VERSION_CODES.Q) {
                runCatching {
                    wifi.createWifiLock(WifiManager.WIFI_MODE_FULL_LOW_LATENCY, "renoa:chat").apply {
                        setReferenceCounted(false)
                        acquire()
                    }
                }.getOrNull()
            } else {
                null
            }
        onDispose { lock?.release() }
    }
}
