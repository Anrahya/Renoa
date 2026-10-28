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
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.runtime.Composable
import androidx.compose.runtime.DisposableEffect
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.saveable.rememberSaveable
import androidx.compose.runtime.setValue
import androidx.compose.ui.Modifier
import androidx.lifecycle.viewmodel.compose.viewModel
import androidx.lifecycle.viewmodel.initializer
import androidx.lifecycle.viewmodel.viewModelFactory
import live.renoa.app.agents.AgentRepository
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
import live.renoa.app.theme.RenoaTheme
import live.renoa.app.theme.Renoa
import live.renoa.app.theme.ThemeName

/** The app. It opens on Home once [signedIn], and on sign-in, which runs [signingIn], until then. */
@Composable
internal fun RenoaApp(repository: () -> AgentRepository, signedIn: Boolean, signingIn: suspend (SignInCode, Passkeys) -> Unit) {
    // The phone's light or dark setting decides, until the owner taps the mark to pick the other theme.
    var picked by rememberSaveable { mutableStateOf<ThemeName?>(null) }
    val theme = picked ?: if (isSystemInDarkTheme()) ThemeName.Graphite else ThemeName.Paper
    val navigator = rememberNavigator(if (signedIn) Route.Home else Route.SignIn)
    val stores: EntryStores = viewModel()

    RenoaTheme(theme) {
        BackHandler(enabled = navigator.stack.size > 1) { navigator.pop() }
        Box(Modifier.fillMaxSize().background(Renoa.colors.background)) {
            AnimatedContent(
                targetState = navigator.topEntry,
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
                        val model: SignInViewModel = viewModel(viewModelStoreOwner = owner, factory = viewModelFactory { initializer { SignInViewModel(signingIn) } })
                        val activity = checkNotNull(LocalActivity.current) { "sign-in needs its activity" }
                        LaunchedEffect(model.state) { if (model.state == SignInState.SignedIn) navigator.resetTo(Route.Home) }
                        SignInScreen(model.state, model.code, { model.code = it }) { model.submit(Passkeys.of(activity)) }
                    }
                    Route.Home -> {
                        val model: HomeViewModel = viewModel(viewModelStoreOwner = owner, factory = viewModelFactory { initializer { HomeViewModel(repository()) } })
                        HomeScreen(
                            model,
                            onOpenChat = { navigator.push(Route.Chat(it)) },
                            onNewAgent = { navigator.push(Route.Create) },
                            onChangeColours = { picked = theme.next() },
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
