package live.renoa.app.push

import android.Manifest
import android.content.Context
import android.content.pm.PackageManager
import android.os.Build
import androidx.activity.compose.rememberLauncherForActivityResult
import androidx.activity.result.contract.ActivityResultContracts
import androidx.compose.runtime.Composable
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.ui.platform.LocalContext
import androidx.core.content.ContextCompat
import androidx.core.content.edit

/**
 * Asks once, on Android 13 and later, to show notifications, the first time the owner is [sending] a
 * message: the moment a reply becomes worth hearing about. The phone's own dialog does the asking.
 * [answered] runs with whatever the owner chose, so Renoa can be told.
 */
@Composable
internal fun AskToNotify(sending: Boolean, answered: () -> Unit) {
    if (Build.VERSION.SDK_INT < Build.VERSION_CODES.TIRAMISU) return
    val context = LocalContext.current
    val ask = rememberLauncherForActivityResult(ActivityResultContracts.RequestPermission()) { answered() }
    LaunchedEffect(sending) {
        if (!sending || ContextCompat.checkSelfPermission(context, Manifest.permission.POST_NOTIFICATIONS) == PackageManager.PERMISSION_GRANTED) return@LaunchedEffect
        val asked = context.getSharedPreferences("push", Context.MODE_PRIVATE)
        if (asked.getBoolean(ASKED, false)) return@LaunchedEffect
        asked.edit { putBoolean(ASKED, true) }
        ask.launch(Manifest.permission.POST_NOTIFICATIONS)
    }
}

private const val ASKED = "asked-to-notify"
