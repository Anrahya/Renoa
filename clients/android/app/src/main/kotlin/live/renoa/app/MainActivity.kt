package live.renoa.app

import android.app.Application
import android.os.Bundle
import androidx.activity.ComponentActivity
import androidx.activity.compose.setContent
import androidx.activity.enableEdgeToEdge
import kotlinx.coroutines.CoroutineScope
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.SupervisorJob
import kotlinx.coroutines.cancel
import java.io.File
import live.renoa.app.agents.AgentRepository
import live.renoa.app.agents.RcpAgentRepository
import live.renoa.app.agents.SavedOutbox
import live.renoa.app.signin.DeviceKeys
import live.renoa.app.signin.RENOA_ORIGIN
import live.renoa.app.signin.SignInStore
import live.renoa.app.signin.SignInViewModel
import live.renoa.app.signup.SavedSignUp
import live.renoa.rcp.Authentication
import live.renoa.rcp.WebSocketRcpSurface

/** Owns what outlives a screen. */
class RenoaApplication : Application() {
    private var repository: AgentRepository? = null
    private var scope: CoroutineScope? = null
    private var outbox: SavedOutbox? = null

    /**
     * The owner's Renoa, connected on first use and kept while the process lives. The first use
     * comes after sign-in, because every screen that asks for it lies past the sign-in screen.
     */
    @Synchronized
    fun repository(): AgentRepository =
        repository ?: run {
            val saved = checkNotNull(SignInStore(this).load()?.takeIf { it.origin == RENOA_ORIGIN }) { "not signed in" }
            val state = SavedOutbox(outboxFile())
            val authentication = Authentication.Device(saved.credentials) { bytes -> DeviceKeys.getOrCreate().sign(bytes) }
            val surface = WebSocketRcpSurface(BuildConfig.RENOA_CONNECT, authentication, state)
            val running = CoroutineScope(SupervisorJob() + Dispatchers.Default)
            RcpAgentRepository(surface, state, running).also {
                it.start()
                repository = it
                scope = running
                outbox = state
            }
        }

    /**
     * Forgets this phone's sign-in, the Renoa it connected to, its unsent messages, and its device
     * key, which Renoa refuses for good once it signed the phone out; signing in again starts afresh
     * with a new key.
     */
    suspend fun signOut() {
        val held =
            synchronized(this) {
                scope?.cancel()
                scope = null
                repository = null
                outbox.also { outbox = null }
            }
        DeviceKeys.forget()
        SignInStore(this).clear()
        SavedSignUp(this).operationId = null
        (held ?: SavedOutbox(outboxFile())).forget()
    }

    private fun outboxFile() = File(filesDir, "outbox")
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
        setContent { RenoaApp(application::repository, signedIn, SignInViewModel.signingIn(applicationContext, store), application::signOut) }
    }
}
