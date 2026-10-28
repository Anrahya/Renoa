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
import live.renoa.app.agents.RcpAgentRepository
import live.renoa.app.signin.DeviceKeys
import live.renoa.app.signin.RENOA_ORIGIN
import live.renoa.app.signin.SignInStore
import live.renoa.app.signin.SignInViewModel
import live.renoa.rcp.Authentication
import live.renoa.rcp.InMemoryRcpState
import live.renoa.rcp.WebSocketRcpSurface

/** Owns what outlives a screen. */
class RenoaApplication : Application() {
    private var repository: AgentRepository? = null

    /**
     * The owner's Renoa, connected on first use and kept while the process lives. The first use
     * comes after sign-in, because every screen that asks for it lies past the sign-in screen.
     */
    @Synchronized
    fun repository(): AgentRepository =
        repository ?: run {
            val saved = checkNotNull(SignInStore(this).load()?.takeIf { it.origin == RENOA_ORIGIN }) { "not signed in" }
            // The chat is rebuilt from a full replay on every launch, so the cursors live only as long as it does.
            val state = InMemoryRcpState()
            val authentication = Authentication.Device(saved.credentials) { bytes -> DeviceKeys.getOrCreate().sign(bytes) }
            val surface = WebSocketRcpSurface(BuildConfig.RENOA_CONNECT, authentication, state)
            RcpAgentRepository(surface, state, CoroutineScope(SupervisorJob() + Dispatchers.Default)).also {
                it.start()
                repository = it
            }
        }
}

class MainActivity : ComponentActivity() {
    override fun onCreate(savedInstanceState: Bundle?) {
        super.onCreate(savedInstanceState)
        // The theme the app resolves sets the bars' colours again on its first frame.
        enableEdgeToEdge()
        val application = application as RenoaApplication
        val store = SignInStore(applicationContext)
        // A credential from another Renoa, as after a build that signs in elsewhere, does not count.
        val signedIn = store.load()?.origin == RENOA_ORIGIN
        setContent { RenoaApp(application::repository, signedIn, SignInViewModel.signingIn(applicationContext, store)) }
    }
}
