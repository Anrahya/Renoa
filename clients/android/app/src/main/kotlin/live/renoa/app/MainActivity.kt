package live.renoa.app

import android.app.Application
import android.net.ConnectivityManager
import android.os.Bundle
import androidx.activity.ComponentActivity
import androidx.activity.compose.setContent
import androidx.activity.enableEdgeToEdge
import kotlinx.coroutines.CoroutineScope
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.SupervisorJob
import kotlinx.coroutines.cancel
import kotlinx.coroutines.launch
import java.io.File
import live.renoa.app.agents.AgentRepository
import live.renoa.app.agents.RcpAgentRepository
import live.renoa.app.agents.SavedHome
import live.renoa.app.agents.SavedOutbox
import live.renoa.app.agents.agentAddress
import live.renoa.app.agents.chatTaskId
import live.renoa.app.push.Notices
import live.renoa.app.push.PushRegistration
import live.renoa.app.push.ShownChats
import live.renoa.app.push.WakeCursors
import live.renoa.app.push.WakeUp
import live.renoa.app.push.agentName
import live.renoa.app.push.firebaseToken
import live.renoa.app.signin.DeviceKeys
import live.renoa.app.signin.RENOA_ORIGIN
import live.renoa.app.signin.SavedSignIn
import live.renoa.app.signin.SignInStore
import live.renoa.app.signin.SignInViewModel
import live.renoa.app.signup.SavedSignUp
import live.renoa.rcp.Authentication
import live.renoa.rcp.WebSocketRcpSurface

/** Owns what outlives a screen. */
class RenoaApplication : Application() {
    private var repository: RcpAgentRepository? = null
    private var surface: WebSocketRcpSurface? = null
    private var scope: CoroutineScope? = null
    private var outbox: SavedOutbox? = null
    private var home: SavedHome? = null
    private var network: ConnectivityManager.NetworkCallback? = null
    private var push: PushRegistration? = null
    private var cursors: WakeCursors? = null
    private val shownChats = ShownChats()

    override fun onCreate() {
        super.onCreate()
        Notices.createChannels(this)
    }

    /**
     * The owner's Renoa, connected on first use and kept while the process lives. The first use
     * comes after sign-in, because every screen that asks for it lies past the sign-in screen.
     */
    @Synchronized
    fun repository(): AgentRepository =
        repository ?: run {
            val saved = checkNotNull(signedIn()) { "not signed in" }
            val state = SavedOutbox(outboxFile())
            val kept = SavedHome(homeFile(), owner(saved))
            val client = WebSocketRcpSurface(BuildConfig.RENOA_CONNECT, authentication(saved), state)
            val running = CoroutineScope(SupervisorJob() + Dispatchers.Default)
            val registration = push(saved)
            RcpAgentRepository(client, state, running, home = kept, onConnected = { registration.register(client) }).also {
                it.start()
                push = registration
                repository = it
                surface = client
                scope = running
                outbox = state
                home = kept
                network = watchNetwork(getSystemService(ConnectivityManager::class.java), it::retryNow)
            }
        }

    /** The owner came back to the app: a link that looks up is checked, and one that is down tries again now. */
    @Synchronized
    fun retryNow() {
        repository?.retryNow()
    }

    /** The owner may have turned notifications on or off: Renoa hears it on the connection that is open, if one is. */
    @Synchronized
    fun notificationsChanged() {
        val client = surface ?: return
        val registration = push ?: return
        scope?.launch { registration.register(client) }
    }

    /** The chat with [agentId] came on screen, or left it. While it is on screen, a wake-up shows nothing for it. */
    fun chatShown(agentId: String, shown: Boolean) {
        val (node, target) = agentAddress(agentId) ?: return
        val taskId = chatTaskId(node, target)
        if (shown) {
            shownChats.show(taskId)
            Notices.cancel(this, taskId)
        } else {
            shownChats.hide(taskId)
        }
    }

    /** What a wake-up does on its own connection as this phone, or null while the phone is not signed in. */
    @Synchronized
    internal fun wakeUp(): WakeUp? {
        val saved = signedIn() ?: return null
        val agents = SavedHome(homeFile(), owner(saved)).load()?.agents.orEmpty()
        return WakeUp(
            wakeCursors(saved),
            connect = { state -> WebSocketRcpSurface(BuildConfig.RENOA_CONNECT, authentication(saved), state) },
            register = push(saved)::register,
            agentName = { task -> agentName(task, agents) },
            onScreen = { it in shownChats },
            post = { Notices.post(this, it) },
        )
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
                // Closed, so the old socket stops pinging Renoa.
                surface?.close()
                surface = null
                val watching = network
                network = null
                runCatching { watching?.let { getSystemService(ConnectivityManager::class.java).unregisterNetworkCallback(it) } }
                push = null
                (outbox to home).also {
                    outbox = null
                    home = null
                }
            }
        val told = synchronized(this) { cursors.also { cursors = null } }
        DeviceKeys.forget()
        SignInStore(this).clear()
        SavedSignUp(this).operationId = null
        (held ?: SavedOutbox(outboxFile())).forget()
        // Forgetting drops whatever is kept, whoever it was kept for.
        (kept ?: SavedHome(homeFile(), owner = "")).forget()
        (told ?: WakeCursors(wakeFile(), owner = "")).forget()
        Notices.cancelAll(this)
    }

    private fun signedIn(): SavedSignIn? = SignInStore(this).load()?.takeIf { it.origin == RENOA_ORIGIN }

    /** Kept per sign-in: a device id belongs to one principal, so another owner's Home never shows. */
    private fun owner(saved: SavedSignIn): String {
        val device = saved.credentials.deviceId
        return "${saved.origin} $device"
    }

    private fun authentication(saved: SavedSignIn) = Authentication.Device(saved.credentials) { bytes -> DeviceKeys.getOrCreate().sign(bytes) }

    private fun push(saved: SavedSignIn) = PushRegistration({ firebaseToken(this) }, { Notices.allowed(this) }, wakeCursors(saved))

    /** One per sign-in, shared by the app's connection and every wake-up, so their changes queue on one lock. */
    @Synchronized
    private fun wakeCursors(saved: SavedSignIn): WakeCursors =
        cursors?.takeIf { it.owner == owner(saved) } ?: WakeCursors(wakeFile(), owner(saved)).also { cursors = it }

    private fun outboxFile() = File(filesDir, "outbox")

    private fun homeFile() = File(filesDir, "home")

    private fun wakeFile() = File(filesDir, "wake")
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
        setContent {
            RenoaApp(
                application::repository, signedIn, SignInViewModel.signingIn(applicationContext, store), application::signOut,
                chatShown = application::chatShown, notificationsAnswered = application::notificationsChanged,
            )
        }
    }

    override fun onStart() {
        super.onStart()
        (application as RenoaApplication).run {
            retryNow()
            // Notifications may have been turned on or off in the phone's settings meanwhile.
            notificationsChanged()
        }
    }
}
