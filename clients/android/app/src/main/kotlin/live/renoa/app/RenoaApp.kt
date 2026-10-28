package live.renoa.app

import androidx.activity.compose.BackHandler
import androidx.compose.animation.AnimatedContent
import androidx.compose.animation.core.tween
import androidx.compose.animation.fadeIn
import androidx.compose.animation.fadeOut
import androidx.compose.animation.slideInHorizontally
import androidx.compose.animation.togetherWith
import androidx.compose.foundation.background
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.runtime.Composable
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
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
import live.renoa.app.nav.Route
import live.renoa.app.nav.rememberNavigator
import live.renoa.app.signin.SignInScreen
import live.renoa.app.theme.RenoaTheme
import live.renoa.app.theme.Renoa
import live.renoa.app.theme.ThemeName

@Composable
fun RenoaApp(repository: AgentRepository) {
    var theme by rememberSaveable { mutableStateOf(ThemeName.Midnight) }
    val navigator = rememberNavigator(Route.SignIn)

    RenoaTheme(theme) {
        BackHandler(enabled = navigator.stack.size > 1) { navigator.pop() }
        Box(Modifier.fillMaxSize().background(Renoa.colors.background)) {
            AnimatedContent(
                targetState = navigator.top,
                transitionSpec = {
                    (slideInHorizontally(tween(300)) { it / 8 } + fadeIn(tween(300))) togetherWith fadeOut(tween(150))
                },
                label = "screen",
            ) { route ->
                when (route) {
                    Route.SignIn -> SignInScreen(onContinue = { navigator.resetTo(Route.Home) })
                    Route.Home -> {
                        val model: HomeViewModel = viewModel(factory = viewModelFactory { initializer { HomeViewModel(repository) } })
                        HomeScreen(
                            model,
                            onOpenChat = { navigator.push(Route.Chat(it)) },
                            onNewAgent = { navigator.push(Route.Create) },
                            onChangeColours = { theme = theme.next() },
                        )
                    }
                    is Route.Chat -> {
                        val model: ChatViewModel = viewModel(key = route.encode(), factory = viewModelFactory { initializer { ChatViewModel(repository, route.agentId) } })
                        ChatScreen(model, onBack = { navigator.pop() }, onOpenProfile = { navigator.push(Route.Edit(it)) })
                    }
                    is Route.Edit -> {
                        val model: AgentEditorViewModel =
                            viewModel(key = route.encode(), factory = viewModelFactory { initializer { AgentEditorViewModel(repository, EditorMode.Editing(route.agentId)) } })
                        AgentEditorScreen(model, onClose = { navigator.pop() }, onFinished = { navigator.pop() })
                    }
                    Route.Create -> {
                        val model: AgentEditorViewModel =
                            viewModel(key = route.encode(), factory = viewModelFactory { initializer { AgentEditorViewModel(repository, EditorMode.Creating) } })
                        AgentEditorScreen(model, onClose = { navigator.pop() }, onFinished = { navigator.replaceTop(Route.Chat(it)) })
                    }
                }
            }
        }
    }
}
