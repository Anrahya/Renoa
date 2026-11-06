package live.renoa.app.update

import android.content.ActivityNotFoundException
import androidx.compose.runtime.Composable
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.rememberCoroutineScope
import androidx.compose.runtime.setValue
import androidx.compose.ui.platform.LocalContext
import kotlinx.coroutines.CancellationException
import kotlinx.coroutines.launch
import live.renoa.app.BuildConfig
import live.renoa.design.CardSection
import live.renoa.design.SecondaryButton
import live.renoa.design.ValueRow

private sealed interface Update {
    data object None : Update

    data class Ready(val release: AppRelease) : Update

    data class Downloading(val release: AppRelease) : Update

    data class Failed(val release: AppRelease, val words: String) : Update
}

/** This app's version and, when Renoa serves a newer one, the button that downloads and installs it. */
@Composable
internal fun AppVersionSection() {
    val updates = LocalAppUpdates.current
    val context = LocalContext.current.applicationContext
    val scope = rememberCoroutineScope()
    var update by remember { mutableStateOf<Update>(Update.None) }
    // Not finding out is the same as being newest: Settings has nothing to offer then.
    LaunchedEffect(updates) {
        update =
            try {
                updates?.newer()?.let(Update::Ready) ?: Update.None
            } catch (cancelled: CancellationException) {
                throw cancelled
            } catch (_: Exception) {
                Update.None
            }
    }
    val get = { release: AppRelease ->
        if (updates != null) {
            update = Update.Downloading(release)
            scope.launch {
                update =
                    try {
                        val apk = updates.download(release)
                        try {
                            install(context, apk)
                            Update.Ready(release)
                        } catch (_: ActivityNotFoundException) {
                            Update.Failed(release, "This phone has no installer to open the update with.")
                        }
                    } catch (cancelled: CancellationException) {
                        throw cancelled
                    } catch (_: Exception) {
                        Update.Failed(release, "The update didn't download. Try again.")
                    }
            }
        }
    }
    val footer = (update as? Update.Failed)?.words
    CardSection(footer = footer) {
        row { ValueRow("App version", BuildConfig.VERSION_NAME) }
        when (val shown = update) {
            Update.None -> Unit
            is Update.Ready, is Update.Failed -> {
                val release = (shown as? Update.Ready)?.release ?: (shown as Update.Failed).release
                row { ValueRow("Version ${release.versionName} is out") { SecondaryButton("Update") { get(release) } } }
            }
            is Update.Downloading -> row { ValueRow("Version ${shown.release.versionName} is out") { SecondaryButton("Downloading…", enabled = false) {} } }
        }
    }
}
