package live.renoa.app.ui

import androidx.compose.runtime.Composable
import androidx.compose.runtime.remember
import androidx.compose.ui.tooling.preview.Preview
import androidx.compose.ui.unit.dp
import kotlinx.coroutines.CoroutineScope
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.SupervisorJob
import live.renoa.app.agents.FakeAgentRepository
import live.renoa.app.chat.ChatScreen
import live.renoa.app.chat.ChatViewModel
import live.renoa.app.editor.AgentEditorScreen
import live.renoa.app.editor.AgentEditorViewModel
import live.renoa.app.editor.EditorMode
import live.renoa.app.home.HomeScreen
import live.renoa.app.home.HomeViewModel
import live.renoa.app.home.SettingsPage
import live.renoa.app.signin.SignInScreen
import live.renoa.app.signin.SignInState
import live.renoa.app.signup.SignUpMemory
import live.renoa.app.signup.SignUpScreen
import live.renoa.app.signup.SignUpViewModel
import live.renoa.app.theme.Appearance
import live.renoa.app.theme.RenoaTheme
import live.renoa.app.theme.ThemeName

@Composable
private fun rememberPreviewAgents(): FakeAgentRepository =
    remember { FakeAgentRepository(CoroutineScope(SupervisorJob() + Dispatchers.Unconfined)) }

@Preview(name = "Welcome - Paper", showBackground = true, widthDp = 390, heightDp = 844)
@Composable
internal fun WelcomePaperPreview() {
    RenoaTheme(ThemeName.Paper, reducedMotion = true) {
        SignInScreen(SignInState.Ready, code = "", onCodeChange = {}, onSubmit = {})
    }
}

@Preview(name = "Welcome - Graphite", showBackground = true, widthDp = 390, heightDp = 844)
@Composable
internal fun WelcomeGraphitePreview() {
    RenoaTheme(ThemeName.Graphite, reducedMotion = true) {
        SignInScreen(SignInState.Ready, code = "", onCodeChange = {}, onSubmit = {})
    }
}

@Preview(name = "Sign in - Graphite", showBackground = true, widthDp = 390, heightDp = 844)
@Composable
internal fun SignInCodeGraphitePreview() {
    RenoaTheme(ThemeName.Graphite, reducedMotion = true) {
        SignInScreen(SignInState.Ready, code = "ab12cd", onCodeChange = {}, onSubmit = {})
    }
}

@Preview(name = "Sign up - Graphite", showBackground = true, widthDp = 390, heightDp = 844)
@Composable
internal fun SignUpGraphitePreview() {
    val agents = remember { FakeAgentRepository(CoroutineScope(SupervisorJob() + Dispatchers.Unconfined), seeds = emptyList()) }
    val model = remember { SignUpViewModel(agents, object : SignUpMemory { override var operationId: String? = null }) }
    RenoaTheme(ThemeName.Graphite, reducedMotion = true) {
        SignUpScreen(model, onDone = {})
    }
}

@Preview(name = "Home - Paper", showBackground = true, widthDp = 390, heightDp = 844)
@Composable
internal fun HomePaperPreview() {
    val agents = rememberPreviewAgents()
    val model = remember { HomeViewModel(agents) }
    RenoaTheme(ThemeName.Paper, reducedMotion = true) {
        HomeScreen(model, onOpenChat = {}, onNewAgent = {}, appearance = Appearance.Paper)
    }
}

@Preview(name = "Home - Graphite", showBackground = true, widthDp = 390, heightDp = 844)
@Composable
internal fun HomeGraphitePreview() {
    val agents = rememberPreviewAgents()
    val model = remember { HomeViewModel(agents) }
    RenoaTheme(ThemeName.Graphite, reducedMotion = true) {
        HomeScreen(model, onOpenChat = {}, onNewAgent = {}, appearance = Appearance.Graphite)
    }
}

@Preview(name = "Chat - Graphite", showBackground = true, widthDp = 390, heightDp = 844)
@Composable
internal fun ChatGraphitePreview() {
    val agents = rememberPreviewAgents()
    val model = remember { ChatViewModel(agents, "renoa") }
    RenoaTheme(ThemeName.Graphite, reducedMotion = true) {
        ChatScreen(model, onBack = {}, onOpenProfile = {})
    }
}

@Preview(name = "Settings - Paper", showBackground = true, widthDp = 390, heightDp = 844)
@Composable
internal fun SettingsPaperPreview() {
    RenoaTheme(ThemeName.Paper, reducedMotion = true) {
        SettingsPage(
            appearance = Appearance.System,
            onAppearance = {},
            onSignOut = {},
            onNotificationsChanged = {},
            bottomInset = 24.dp,
        )
    }
}

@Preview(name = "New Agent - Graphite", showBackground = true, widthDp = 390, heightDp = 844)
@Composable
internal fun NewAgentGraphitePreview() {
    val agents = rememberPreviewAgents()
    val model = remember { AgentEditorViewModel(agents, EditorMode.Creating) }
    RenoaTheme(ThemeName.Graphite, reducedMotion = true) {
        AgentEditorScreen(model, onClose = {}, onFinished = {})
    }
}
