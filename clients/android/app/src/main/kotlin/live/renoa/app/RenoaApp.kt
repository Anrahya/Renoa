package live.renoa.app

import androidx.activity.compose.BackHandler
import androidx.activity.compose.LocalActivity
import androidx.compose.animation.AnimatedContent
import androidx.compose.animation.core.tween
import androidx.compose.animation.fadeIn
import androidx.compose.animation.fadeOut
import androidx.compose.animation.slideInHorizontally
import androidx.compose.animation.togetherWith
import androidx.compose.foundation.background
import androidx.compose.foundation.isSystemInDarkTheme
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.fillMaxHeight
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.widthIn
import androidx.compose.runtime.Composable
import androidx.compose.runtime.DisposableEffect
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.saveable.rememberSaveable
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.platform.LocalContext
import androidx.compose.ui.unit.dp
import androidx.lifecycle.viewmodel.compose.viewModel
import androidx.lifecycle.viewmodel.initializer
import androidx.lifecycle.viewmodel.viewModelFactory
import kotlinx.coroutines.flow.first
import live.renoa.app.agents.AgentRepository
import live.renoa.app.agents.Link
import live.renoa.app.chat.ChatScreen
import live.renoa.app.chat.ChatViewModel
import live.renoa.app.editor.AgentEditorScreen
import live.renoa.app.editor.AgentEditorViewModel
import live.renoa.app.editor.EditorMode
import live.renoa.app.home.HomeScreen
import live.renoa.app.home.HomeViewModel
import live.renoa.app.nav.EntryStores
import live.renoa.app.nav.Route
import live.renoa.app.nav.rememberNavigator
import live.renoa.app.signin.Passkeys
import live.renoa.app.signin.SignInCode
import live.renoa.app.signin.SignInScreen
import live.renoa.app.signin.SignInState
import live.renoa.app.signin.SignInViewModel
import live.renoa.app.signup.SavedSignUp
import live.renoa.app.signup.SignUpScreen
import live.renoa.app.signup.SignUpViewModel
import live.renoa.app.theme.RenoaTheme
import live.renoa.app.theme.SystemBars
import live.renoa.app.theme.Renoa
import live.renoa.app.theme.ThemeName

/** What sign-in says after Renoa stopped accepting this phone. */
internal const val SIGNED_OUT = "This phone was signed out. Sign in again to carry on."

/** The widest the screens grow; past it, the theme's ground fills the sides. */
internal val MAX_CONTENT_WIDTH = 600.dp

/**
 * The app. It opens on Home once [signedIn], and on sign-in, which runs [signingIn], until then.
 * When Renoa refuses this phone's sign-in, [signedOut] forgets it and the app goes back to sign-in.
 */
@Composable
internal fun RenoaApp(repository: () -> AgentRepository, signedIn: Boolean, signingIn: suspend (SignInCode, Passkeys) -> Unit, signedOut: suspend () -> Unit = {}) {
    // The phone's light or dark setting decides, until the owner taps the mark to pick the other theme.
    var picked by rememberSaveable { mutableStateOf<ThemeName?>(null) }
    val theme = picked ?: if (isSystemInDarkTheme()) ThemeName.Graphite else ThemeName.Paper
    val navigator = rememberNavigator(if (signedIn) Route.Home else Route.SignIn)
    val stores: EntryStores = viewModel()
    // Starting over drops every screen below the top at once; the top one lets go of its own as it leaves.
    val startOver = { route: Route ->
        val below = navigator.entries.dropLast(1)
        navigator.resetTo(route)
        below.forEach { stores.release(it.id) }
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

    RenoaTheme(theme) {
        SystemBars(theme)
        BackHandler(enabled = navigator.stack.size > 1) { navigator.pop() }
        Box(Modifier.fillMaxSize().background(Renoa.colors.background)) {
            AnimatedContent(
                targetState = navigator.topEntry,
                // A tablet keeps a phone's column, centred, so lines stay readable and share one left edge.
                modifier = Modifier.align(Alignment.TopCenter).widthIn(max = MAX_CONTENT_WIDTH).fillMaxHeight(),
                transitionSpec = {
                    (slideInHorizontally(tween(300)) { it / 8 } + fadeIn(tween(300))) togetherWith fadeOut(tween(150))
                },
                label = "screen",
            ) { entry ->
                // A screen keeps its view models only while its entry is on the stack. A rotation
                // disposes this too, but the entry is still on the stack then, so nothing is lost.
                val owner = remember { stores.ownerFor(entry.id) }
                DisposableEffect(Unit) { onDispose { if (navigator.entries.none { it.id == entry.id }) stores.release(entry.id) } }
                when (val route = entry.route) {
                    Route.SignIn -> {
                        val model: SignInViewModel = viewModel(viewModelStoreOwner = owner, factory = viewModelFactory { initializer { SignInViewModel(signingIn, if (wasSignedOut) SignInState.Failed(SIGNED_OUT) else SignInState.Ready) } })
                        val activity = checkNotNull(LocalActivity.current) { "sign-in needs its activity" }
                        LaunchedEffect(model.state) { if (model.state == SignInState.SignedIn) startOver(Route.Home) }
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
                            onChangeColours = { picked = theme.next() },
                            onSignUp = { startOver(Route.SignUp) },
                        )
                    }
                    is Route.Chat -> {
                        val model: ChatViewModel = viewModel(viewModelStoreOwner = owner, factory = viewModelFactory { initializer { ChatViewModel(repository(), route.agentId) } })
                        ChatScreen(model, onBack = { navigator.pop() }, onOpenProfile = { navigator.push(Route.Edit(it)) })
                    }
                    is Route.Edit -> {
                        val model: AgentEditorViewModel =
                            viewModel(viewModelStoreOwner = owner, factory = viewModelFactory { initializer { AgentEditorViewModel(repository(), EditorMode.Editing(route.agentId)) } })
                        AgentEditorScreen(model, onClose = { navigator.pop() }, onFinished = { navigator.pop() })
                    }
                    Route.Create -> {
                        val model: AgentEditorViewModel =
                            viewModel(viewModelStoreOwner = owner, factory = viewModelFactory { initializer { AgentEditorViewModel(repository(), EditorMode.Creating) } })
                        AgentEditorScreen(model, onClose = { navigator.pop() }, onFinished = { navigator.replaceTop(Route.Chat(it)) })
                    }
                }
            }
        }
    }
}
