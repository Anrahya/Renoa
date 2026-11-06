// Portions copyright 2026 Po Kai Lee and the Codync contributors.
// Licensed under the Apache License, Version 2.0; see licenses/Apache-2.0-Codync.txt.
// Push stack and modal sheet presentation adapted from Codync apps/android at ed8daf11f89398e598ef1f791abc3014ded68496.
package live.renoa.app

import androidx.activity.compose.LocalActivity
import androidx.compose.foundation.background
import androidx.compose.foundation.isSystemInDarkTheme
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.fillMaxHeight
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.widthIn
import androidx.compose.runtime.Composable
import androidx.compose.runtime.DisposableEffect
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.collectAsState
import androidx.compose.runtime.getValue
import androidx.compose.runtime.key
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.rememberCoroutineScope
import androidx.compose.runtime.saveable.rememberSaveable
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.platform.LocalContext
import androidx.compose.ui.unit.dp
import androidx.lifecycle.compose.LifecycleStartEffect
import androidx.lifecycle.viewmodel.compose.viewModel
import androidx.lifecycle.viewmodel.initializer
import androidx.lifecycle.viewmodel.viewModelFactory
import kotlinx.coroutines.CancellationException
import kotlinx.coroutines.flow.first
import kotlinx.coroutines.launch
import live.renoa.app.agents.AgentRepository
import live.renoa.app.agents.Link
import live.renoa.app.chat.ChatScreen
import live.renoa.app.chat.ChatViewModel
import live.renoa.app.connect.HeldSetupLinks
import live.renoa.app.connect.SetupLinkCards
import live.renoa.app.editor.AgentEditorScreen
import live.renoa.app.editor.AgentEditorViewModel
import live.renoa.app.editor.EditorMode
import live.renoa.app.home.HomeScreen
import live.renoa.app.home.HomeViewModel
import live.renoa.app.nav.EntryStores
import live.renoa.app.nav.Route
import live.renoa.app.nav.rememberNavigator
import live.renoa.app.push.AskToNotify
import live.renoa.app.signin.Passkeys
import live.renoa.app.signin.SignInCode
import live.renoa.app.signin.SignInScreen
import live.renoa.app.signin.SignInState
import live.renoa.app.signin.SignInViewModel
import live.renoa.app.signup.SavedSignUp
import live.renoa.app.signup.SignUpScreen
import live.renoa.app.signup.SignUpViewModel
import live.renoa.app.theme.Renoa
import live.renoa.app.theme.RenoaTheme
import live.renoa.app.theme.SystemBars
import live.renoa.app.theme.rememberAppearance
import live.renoa.design.DesignSheet
import live.renoa.design.NavStack

/** What sign-in says after Renoa stopped accepting this phone. */
internal const val SIGNED_OUT = "This phone was signed out. Sign in again to carry on."

/** The widest the screens grow; past it, the theme's ground fills the sides. */
internal val MAX_CONTENT_WIDTH = 600.dp

/**
 * The app. It opens on Home once [signedIn], and on sign-in, which runs [signingIn], until then.
 * When Renoa refuses this phone's sign-in, [signedOut] forgets it and the app goes back to sign-in.
 * When the owner signs out, [signOut] tells Renoa and forgets the phone; if Renoa cannot be told, it
 * throws, the phone stays signed in, and Home says so.
 * [chatShown] hears each chat come on screen and leave it; [notificationsAnswered], the owner's
 * answer when asked to allow notifications. [openChat] is a chat a tapped notice asks for, opened
 * as [Navigator.openChat][live.renoa.app.nav.Navigator.openChat] says; [chatOpened] then clears it.
 * Past sign-in and sign-up, the [setupLinks] waiting for the owner show over every screen.
 */
@Composable
internal fun RenoaApp(
    repository: () -> AgentRepository,
    signedIn: Boolean,
    signingIn: suspend (SignInCode, Passkeys) -> Unit,
    signedOut: suspend () -> Unit = {},
    signOut: suspend () -> Unit = signedOut,
    chatShown: (agentId: String, shown: Boolean) -> Unit = { _, _ -> },
    notificationsAnswered: () -> Unit = {},
    openChat: String? = null,
    chatOpened: () -> Unit = {},
    setupLinks: (() -> HeldSetupLinks)? = null,
    retryNow: () -> Unit = {},
) {
    val (appearance, setAppearance) = rememberAppearance()
    val theme = appearance.resolve(isSystemInDarkTheme())
    val navigator = rememberNavigator(if (signedIn) Route.Home else Route.SignIn)
    val stores: EntryStores = viewModel()
    val scope = rememberCoroutineScope()
    // Starting over drops every screen below the top at once; the top one lets go of its own as it leaves.
    val startOver = { route: Route ->
        val below = navigator.entries.dropLast(1)
        navigator.resetTo(route)
        below.forEach { stores.release(it.id) }
    }
    LaunchedEffect(openChat) {
        if (openChat == null) return@LaunchedEffect
        navigator.openChat(openChat)
        chatOpened()
    }
    var wasSignedOut by rememberSaveable { mutableStateOf(false) }
    if (navigator.top != Route.SignIn) {
        LaunchedEffect(Unit) {
            repository().link.first { it == Link.SignedOut }
            signedOut()
            wasSignedOut = true
            startOver(Route.SignIn)
        }
    }

    val stackEntries = navigator.entries.filterNot { it.route.isModal }
    val sheetEntry = navigator.entries.lastOrNull()?.takeIf { it.route.isModal }

    RenoaTheme(theme) {
        SystemBars(theme)
        Box(Modifier.fillMaxSize().background(Renoa.colors.background)) {
            NavStack(
                stack = stackEntries,
                key = { it.id },
                pop = { navigator.pop() },
                // A tablet keeps a phone's column, centred, so lines stay readable and share one left edge.
                modifier = Modifier.align(Alignment.TopCenter).widthIn(max = MAX_CONTENT_WIDTH).fillMaxHeight(),
            ) { entry ->
                val owner = remember(entry.id) { stores.ownerFor(entry.id) }
                DisposableEffect(entry.id) { onDispose { if (navigator.entries.none { it.id == entry.id }) stores.release(entry.id) } }
                when (val route = entry.route) {
                    Route.SignIn -> {
                        val model: SignInViewModel = viewModel(viewModelStoreOwner = owner, factory = viewModelFactory { initializer { SignInViewModel(signingIn, if (wasSignedOut) SignInState.Failed(SIGNED_OUT) else SignInState.Ready) } })
                        val activity = checkNotNull(LocalActivity.current) { "sign-in needs its activity" }
                        LaunchedEffect(model.state) {
                            if (model.state == SignInState.SignedIn) {
                                wasSignedOut = false
                                startOver(Route.Home)
                            }
                        }
                        SignInScreen(model.state, model.code, { model.code = it }) { model.submit(Passkeys.of(activity)) }
                    }
                    Route.SignUp -> {
                        val memory = SavedSignUp(LocalContext.current.applicationContext)
                        val model: SignUpViewModel = viewModel(viewModelStoreOwner = owner, factory = viewModelFactory { initializer { SignUpViewModel(repository(), memory) } })
                        SignUpScreen(model, onDone = { startOver(Route.Home) })
                    }
                    Route.Home -> {
                        val model: HomeViewModel = viewModel(viewModelStoreOwner = owner, factory = viewModelFactory { initializer { HomeViewModel(repository()) } })
                        HomeScreen(
                            model,
                            onOpenChat = { navigator.push(Route.Chat(it)) },
                            onNewAgent = { navigator.push(Route.Create) },
                            appearance = appearance,
                            onAppearance = setAppearance,
                            onSignUp = { startOver(Route.SignUp) },
                            onSignOut = {
                                scope.launch {
                                    try {
                                        signOut()
                                    } catch (cancelled: CancellationException) {
                                        throw cancelled
                                    } catch (_: Exception) {
                                        model.couldNotSignOut()
                                        return@launch
                                    }
                                    wasSignedOut = false
                                    startOver(Route.SignIn)
                                }
                            },
                            onEditAgent = { navigator.push(Route.Edit(it)) },
                            onRefresh = retryNow,
                            onNotificationsChanged = notificationsAnswered,
                        )
                    }
                    is Route.Chat -> {
                        val model: ChatViewModel = viewModel(viewModelStoreOwner = owner, factory = viewModelFactory { initializer { ChatViewModel(repository(), route.agentId) } })
                        ChatScreen(model, onBack = { navigator.pop() }, onOpenProfile = { navigator.push(Route.Edit(it)) }, onRefresh = retryNow)
                        LifecycleStartEffect(route.agentId) {
                            chatShown(route.agentId, true)
                            onStopOrDispose { chatShown(route.agentId, false) }
                        }
                        AskToNotify(model.state.collectAsState().value.sending, notificationsAnswered)
                    }
                    is Route.Edit, Route.Create -> Unit
                }
            }
            DesignSheet(sheetEntry, onDismiss = { sheetEntry?.let(navigator::close) }) { entry ->
                key(entry.id) {
                    val owner = remember(entry.id) { stores.ownerFor(entry.id) }
                    DisposableEffect(entry.id) { onDispose { if (navigator.entries.none { it.id == entry.id }) stores.release(entry.id) } }
                    when (val route = entry.route) {
                        is Route.Edit -> {
                            val model: AgentEditorViewModel =
                                viewModel(viewModelStoreOwner = owner, factory = viewModelFactory { initializer { AgentEditorViewModel(repository(), EditorMode.Editing(route.agentId)) } })
                            AgentEditorScreen(model, onClose = { navigator.close(entry) }, onFinished = { navigator.close(entry) })
                        }
                        Route.Create -> {
                            val model: AgentEditorViewModel =
                                viewModel(viewModelStoreOwner = owner, factory = viewModelFactory { initializer { AgentEditorViewModel(repository(), EditorMode.Creating) } })
                            AgentEditorScreen(model, onClose = { navigator.close(entry) }, onFinished = { navigator.replace(entry, Route.Chat(it)) })
                        }
                        else -> Unit
                    }
                }
            }
            if (setupLinks != null && navigator.top != Route.SignIn && navigator.top != Route.SignUp) {
                SetupLinkCards(remember { setupLinks() }, Modifier.align(Alignment.TopCenter))
            }
        }
    }
}
