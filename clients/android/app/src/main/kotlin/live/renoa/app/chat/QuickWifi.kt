package live.renoa.app.chat

import android.content.Context
import android.net.wifi.WifiManager
import android.os.Build
import androidx.compose.runtime.Composable
import androidx.compose.runtime.DisposableEffect
import androidx.compose.ui.platform.LocalContext

/**
 * Keeps Wi-Fi out of power save while the chat is on screen. Idle, a phone's Wi-Fi holds incoming
 * packets for seconds and lets them through in a burst, so a streaming reply stalls mid-word until
 * the owner touches the screen. Android honours the lock only while the app is in front and the
 * screen is on, and it has no low-latency mode before Android 10.
 */
@Composable
fun QuickWifi() {
    val context = LocalContext.current
    DisposableEffect(context) {
        val wifi = context.applicationContext.getSystemService(Context.WIFI_SERVICE) as WifiManager?
        val lock =
            if (wifi != null && Build.VERSION.SDK_INT >= Build.VERSION_CODES.Q) {
                wifi.createWifiLock(WifiManager.WIFI_MODE_FULL_LOW_LATENCY, "renoa:chat").apply {
                    setReferenceCounted(false)
                    acquire()
                }
            } else {
                null
            }
        onDispose { lock?.release() }
    }
}
