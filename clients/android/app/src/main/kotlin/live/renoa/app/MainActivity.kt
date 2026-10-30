package live.renoa.app

import android.app.Application
import android.net.ConnectivityManager
import android.net.Network
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
import live.renoa.app.agents.SavedHome
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
    private var repository: RcpAgentRepository? = null
    private var scope: CoroutineScope? = null
    private var outbox: SavedOutbox? = null
    private var home: SavedHome? = null
    private var network: ConnectivityManager.NetworkCallback? = null

    /**
     * The owner's Renoa, connected on first use and kept while the process lives. The first use
     * comes after sign-in, because every screen that asks for it lies past the sign-in screen.
     */
    @Synchronized
    fun repository(): AgentRepository =
        repository ?: run {
            val saved = checkNotNull(SignInStore(this).load()?.takeIf { it.origin == RENOA_ORIGIN }) { "not signed in" }
            val state = SavedOutbox(outboxFile())
            // Kept per sign-in: a device id belongs to one principal, so another owner's Home never shows.
            val device = saved.credentials.deviceId
            val kept = SavedHome(homeFile(), owner = "${saved.origin} $device")
            val authentication = Authentication.Device(saved.credentials) { bytes -> DeviceKeys.getOrCreate().sign(bytes) }
            val surface = WebSocketRcpSurface(BuildConfig.RENOA_CONNECT, authentication, state)
            val running = CoroutineScope(SupervisorJob() + Dispatchers.Default)
            RcpAgentRepository(surface, state, running, home = kept).also {
                it.start()
                repository = it
                scope = running
                outbox = state
                home = kept
                network = retryOnNetwork(it)
            }
        }

    /** A reconnect waiting out its backoff tries at once when the owner comes back to the app. */
    @Synchronized
    fun retryNow() {
        repository?.retryNow()
    }

    /**
     * A reconnect waiting out its backoff tries at once when a network comes up or the default one
     * changes. Some Android 11 builds refuse the registration; the backoff then runs as before.
     */
    private fun retryOnNetwork(repository: RcpAgentRepository): ConnectivityManager.NetworkCallback? {
        val callback =
            object : ConnectivityManager.NetworkCallback() {
                override fun onAvailable(network: Network) = repository.retryNow()
            }
        return runCatching { getSystemService(ConnectivityManager::class.java).registerDefaultNetworkCallback(callback) }.map { callback }.getOrNull()
    }

    /**
     * Forgets this phone's sign-in, the Renoa it connected to, its unsent messages, the Home it kept,
     * and its device key, which Renoa refuses for good once it signed the phone out; signing in again
     * starts afresh with a new key.
     */
    suspend fun signOut() {
        val (held, kept) =
            synchronized(this) {
                scope?.cancel()
                scope = null
                repository = null
                network?.let { getSystemService(ConnectivityManager::class.java).unregisterNetworkCallback(it) }
                network = null
                (outbox to home).also {
                    outbox = null
                    home = null
                }
            }
        DeviceKeys.forget()
        SignInStore(this).clear()
        SavedSignUp(this).operationId = null
        (held ?: SavedOutbox(outboxFile())).forget()
        // Forgetting drops whatever is kept, whoever it was kept for.
        (kept ?: SavedHome(homeFile(), owner = "")).forget()
    }

    private fun outboxFile() = File(filesDir, "outbox")

    private fun homeFile() = File(filesDir, "home")
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

    override fun onStart() {
        super.onStart()
        (application as RenoaApplication).retryNow()
    }
}
