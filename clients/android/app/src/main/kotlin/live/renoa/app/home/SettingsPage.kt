// Portions copyright 2026 Po Kai Lee and the Codync contributors.
// Licensed under the Apache License, Version 2.0; see licenses/Apache-2.0-Codync.txt.
// Adapted from Codync apps/android `SettingsView.kt`, `AppearanceSettings.kt`, `StateScreen.kt`,
// and `AccountSwitcherView.kt` at ed8daf11f89398e598ef1f791abc3014ded68496.
package live.renoa.app.home

import android.Manifest
import android.content.Context
import android.content.Intent
import android.content.pm.PackageManager
import android.os.Build
import android.provider.Settings
import androidx.activity.compose.rememberLauncherForActivityResult
import androidx.activity.result.contract.ActivityResultContracts
import androidx.compose.foundation.background
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.PaddingValues
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.layout.statusBarsPadding
import androidx.compose.material3.Text
import androidx.compose.runtime.Composable
import androidx.compose.runtime.DisposableEffect
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableIntStateOf
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.draw.clip
import androidx.compose.ui.platform.LocalContext
import androidx.compose.ui.platform.LocalInspectionMode
import androidx.compose.ui.semantics.clearAndSetSemantics
import androidx.compose.ui.semantics.heading
import androidx.compose.ui.semantics.semantics
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.text.style.TextOverflow
import androidx.compose.ui.unit.Dp
import androidx.compose.ui.unit.dp
import androidx.core.content.ContextCompat
import androidx.core.content.edit
import androidx.lifecycle.Lifecycle
import androidx.lifecycle.LifecycleEventObserver
import androidx.lifecycle.compose.LocalLifecycleOwner
import com.kyant.shapes.Capsule
import live.renoa.app.BuildConfig
import live.renoa.app.agents.Link
import live.renoa.app.push.Notices
import live.renoa.app.theme.Appearance
import live.renoa.design.CardForm
import live.renoa.design.CardSection
import live.renoa.design.ChoicePicker
import live.renoa.design.DesignDialog
import live.renoa.design.DesignSheet
import live.renoa.design.DialogAction
import live.renoa.design.ModalHeader
import live.renoa.design.Palette
import live.renoa.design.Symbol
import live.renoa.design.Type
import live.renoa.design.ValueRow
import live.renoa.design.bold
import live.renoa.design.plainButton
import live.renoa.design.semibold

private const val PUSH_PREFS = "push"
private const val ASKED_TO_NOTIFY = "asked-to-notify"

/** The Settings tab: appearance on this phone, notification permission, app version, and signing out. */
@Composable
internal fun SettingsPage(
    appearance: Appearance,
    onAppearance: (Appearance) -> Unit,
    onSignOut: () -> Unit,
    onNotificationsChanged: () -> Unit,
    bottomInset: Dp,
) {
    var confirmSignOut by remember { mutableStateOf(false) }
    CardForm(
        Modifier.background(Palette.background).statusBarsPadding(),
        PaddingValues(start = 20.dp, top = 12.dp, end = 20.dp, bottom = bottomInset),
    ) {
        Text("Settings", Modifier.semantics { heading() }, style = Type.largeTitle.bold, color = Palette.text)
        CardSection(title = "Appearance", footer = "Only changes appearance on this device.") {
            row {
                ValueRow("Theme") {
                    ChoicePicker(appearance, Appearance.entries.map { it to it.label }, onAppearance, label = "Theme")
                }
            }
        }
        NotificationsSection(onNotificationsChanged)
        CardSection {
            row { ValueRow("App version", BuildConfig.VERSION_NAME) }
        }
        CardSection {
            row {
                SignOutRow { confirmSignOut = true }
            }
        }
    }
    SignOutDialog(confirmSignOut, { confirmSignOut = false }, onSignOut)
}

/** The Account sheet opened from the roster's top-left capsule. */
@Composable
internal fun AccountSheet(open: Boolean, onDismiss: () -> Unit, state: HomeUiState, onSignOut: () -> Unit) {
    var confirmSignOut by remember { mutableStateOf(false) }
    DesignSheet(open, onDismiss) {
        Column(Modifier.fillMaxSize().background(Palette.background)) {
            ModalHeader("Account")
            CardForm(Modifier.weight(1f)) {
                Column(
                    Modifier.fillMaxWidth().padding(vertical = 8.dp),
                    horizontalAlignment = Alignment.CenterHorizontally,
                    verticalArrangement = Arrangement.spacedBy(10.dp),
                ) {
                    Box(
                        Modifier.size(76.dp).clip(Capsule()).background(Palette.bubbleUser).clearAndSetSemantics {},
                        contentAlignment = Alignment.Center,
                    ) {
                        val initial = state.ownerName?.trim()?.take(1)?.uppercase()
                        if (initial.isNullOrEmpty()) Symbol("person.fill", size = 36.dp, tint = Palette.tertiary)
                        else Text(initial, style = Type.system(32f, FontWeight.SemiBold), color = Palette.text)
                    }
                    Text(
                        state.ownerName ?: "Signed in",
                        style = Type.title3.semibold,
                        color = Palette.text,
                        maxLines = 1,
                        overflow = TextOverflow.Ellipsis,
                    )
                    Text(
                        when (state.link) {
                            Link.Connected -> "Connected to Renoa"
                            Link.Connecting -> "Connecting to Renoa…"
                            Link.Lost -> "Reconnecting to Renoa…"
                            Link.SignedOut -> "Signed out"
                        },
                        style = Type.subheadline,
                        color = Palette.secondary,
                    )
                }
                if (state.known) {
                    CardSection {
                        row { ValueRow("Agents", "${state.roster.size}") }
                        row { ValueRow("Routines", "${state.routines.count { it.on }} on") }
                    }
                }
                CardSection {
                    row { SignOutRow { confirmSignOut = true } }
                }
            }
        }
    }
    SignOutDialog(confirmSignOut, { confirmSignOut = false }) {
        onDismiss()
        onSignOut()
    }
}

@Composable
private fun SignOutRow(onClick: () -> Unit) {
    Row(
        Modifier.fillMaxWidth().plainButton(action = onClick),
        verticalAlignment = Alignment.CenterVertically,
        horizontalArrangement = Arrangement.spacedBy(10.dp),
    ) {
        Symbol("rectangle.portrait.and.arrow.right", size = 20.dp, tint = Palette.danger)
        Text("Sign out", style = Type.body, color = Palette.danger)
    }
}

@Composable
private fun SignOutDialog(open: Boolean, onDismiss: () -> Unit, onConfirm: () -> Unit) {
    DesignDialog(
        "Sign out of Renoa?",
        open,
        onDismiss,
        "This phone stops getting notifications from your agents, and messages that didn't send are dropped. Your agents and chats stay where they are; sign in again with a new code to return.",
    ) {
        listOf(DialogAction("Sign out", destructive = true, action = onConfirm))
    }
}

@Composable
private fun NotificationsSection(onNotificationsChanged: () -> Unit) {
    val preview = LocalInspectionMode.current
    val appContext = LocalContext.current
    var updates by remember { mutableIntStateOf(0) }
    val request =
        if (preview) {
            null
        } else {
            rememberLauncherForActivityResult(ActivityResultContracts.RequestPermission()) {
                updates++
                onNotificationsChanged()
            }
        }
    val asked =
        if (preview) {
            false
        } else {
            rememberOnResume(updates) { appContext.getSharedPreferences(PUSH_PREFS, Context.MODE_PRIVATE).getBoolean(ASKED_TO_NOTIFY, false) }
        }
    val allowed =
        if (preview) {
            true
        } else {
            rememberOnResume(updates) {
                Notices.wanted(appContext) &&
                    (Build.VERSION.SDK_INT < Build.VERSION_CODES.TIRAMISU ||
                        ContextCompat.checkSelfPermission(appContext, Manifest.permission.POST_NOTIFICATIONS) == PackageManager.PERMISSION_GRANTED)
            }
        }
    CardSection(
        title = "Notifications",
        footer = "Hear when an agent answers you, needs you, or gets stuck. Notification previews follow your Android settings.",
    ) {
        row {
            ValueRow(
                "Notifications",
                when {
                    allowed -> "On"
                    asked -> "Off in Settings"
                    else -> "Turn on"
                },
                Modifier.plainButton {
                    if (!allowed && !asked && Build.VERSION.SDK_INT >= Build.VERSION_CODES.TIRAMISU && request != null) {
                        appContext.getSharedPreferences(PUSH_PREFS, Context.MODE_PRIVATE).edit { putBoolean(ASKED_TO_NOTIFY, true) }
                        request.launch(Manifest.permission.POST_NOTIFICATIONS)
                    } else {
                        openNotificationSettings(appContext)
                    }
                },
            )
        }
    }
}

@Composable
private fun <T> rememberOnResume(key: Any? = Unit, read: () -> T): T {
    val owner = LocalLifecycleOwner.current
    var value by remember(key) { mutableStateOf(read()) }
    DisposableEffect(owner, key) {
        val observer = LifecycleEventObserver { _, event -> if (event == Lifecycle.Event.ON_RESUME) value = read() }
        owner.lifecycle.addObserver(observer)
        onDispose { owner.lifecycle.removeObserver(observer) }
    }
    return value
}

private fun openNotificationSettings(appContext: Context) {
    runCatching {
        appContext.startActivity(
            Intent(Settings.ACTION_APP_NOTIFICATION_SETTINGS)
                .putExtra(Settings.EXTRA_APP_PACKAGE, appContext.packageName)
                .addFlags(Intent.FLAG_ACTIVITY_NEW_TASK),
        )
    }
}
