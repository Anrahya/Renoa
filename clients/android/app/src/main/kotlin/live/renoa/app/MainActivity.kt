package live.renoa.app

import android.app.Application
import android.os.Bundle
import androidx.activity.ComponentActivity
import androidx.activity.compose.setContent
import androidx.activity.enableEdgeToEdge
import kotlinx.coroutines.CoroutineScope
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.SupervisorJob
import live.renoa.app.agents.AgentRepository
import live.renoa.app.agents.FakeAgentRepository
import live.renoa.app.signin.RENOA_ORIGIN
import live.renoa.app.signin.SignInStore
import live.renoa.app.signin.SignInViewModel

/** Owns what outlives a screen. The repository is a fake until the backend work lands. */
class RenoaApplication : Application() {
    val repository: AgentRepository by lazy { FakeAgentRepository(CoroutineScope(SupervisorJob() + Dispatchers.Default)) }
}

class MainActivity : ComponentActivity() {
    override fun onCreate(savedInstanceState: Bundle?) {
        super.onCreate(savedInstanceState)
        enableEdgeToEdge()
        val repository = (application as RenoaApplication).repository
        val store = SignInStore(applicationContext)
        // A credential from another Renoa, as after a build that signs in elsewhere, does not count.
        val signedIn = store.load()?.origin == RENOA_ORIGIN
        setContent { RenoaApp(repository, signedIn, SignInViewModel.signingIn(applicationContext, store)) }
    }
}
