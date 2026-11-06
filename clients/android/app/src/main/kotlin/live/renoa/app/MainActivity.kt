package live.renoa.app

import android.app.Application
import android.content.Intent
import android.net.ConnectivityManager
import android.os.Bundle
import androidx.activity.ComponentActivity
import androidx.activity.compose.setContent
import androidx.activity.enableEdgeToEdge
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.setValue
import kotlinx.coroutines.CoroutineScope
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.SupervisorJob
import kotlinx.coroutines.cancel
import kotlinx.coroutines.launch
import java.io.File
import live.renoa.app.agents.AgentRepository
import live.renoa.app.connect.HeldSetupLinks
import live.renoa.app.connect.turnsEnded
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
import live.renoa.app.push.markRead
import live.renoa.app.push.sender
import live.renoa.app.push.firebaseToken
import live.renoa.app.signin.DeviceKeys
import live.renoa.app.signin.NoticeKeys
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
    private var pushing: Pushing? = null
    private var setupLinks: HeldSetupLinks? = null
    private val shownChats = ShownChats()

    /** One sign-in's wake cursors and registration, shared by the app's connection and every wake-up, so their changes queue on one lock each. */
    private class Pushing(val owner: String, val cursors: WakeCursors, val registration: PushRegistration)

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
            val registration = pushing(saved).registration
            val links = HeldSetupLinks(client, saved.credentials.deviceId, noticeKeys::getOrCreate)
            val onConnected: suspend () -> Unit = {
                // Each is told on its own: a failed push registration must not keep the setup links unread.
                running.launch { runCatching { links.connected() } }
                registration.register(client)
            }
            RcpAgentRepository(client, state, running, home = kept, onConnected = onConnected).also {
                it.start()
                // A setup link ends its turn, so a turn that ends may have left one.
                running.launch { it.agents.turnsEnded().collect { runCatching { links.read() } } }
                setupLinks = links
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
        val links = setupLinks ?: return
        scope?.launch { runCatching { links.read() } }
    }

    /** The setup links waiting for the owner, held by the connection [repository] made. */
    @Synchronized
    fun setupLinks(): HeldSetupLinks {
        repository()
        return checkNotNull(setupLinks)
    }

    /** The owner may have turned notifications on or off: Renoa hears it on the connection that is open, if one is. */
    @Synchronized
    fun notificationsChanged() {
        val client = surface ?: return
        val registration = pushing?.registration ?: return
        // Not told now, told on the next connection.
        scope?.launch { runCatching { registration.register(client) } }
    }

    /**
     * The chat with [agentId] came on screen, or left it. While it is on screen, a wake-up shows nothing
     * for it; what the app showed of it, on coming and on leaving, counts as read for later wake-ups.
     */
    @Synchronized
    fun chatShown(agentId: String, shown: Boolean) {
        val (node, target) = agentAddress(agentId) ?: return
        val taskId = chatTaskId(node, target)
        if (shown) {
            shownChats.show(taskId)
            Notices.cancel(this, taskId)
        } else {
            shownChats.hide(taskId)
        }
        // Read now, as the chat moves, so nothing applied after it left the screen counts as read.
        val applied = outbox?.applied(taskId) ?: return
        val cursors = pushing?.cursors ?: return
        // Not kept, read again: the next wake-up may show it once more.
        scope?.launch { runCatching { markRead(taskId, applied, cursors) } }
    }

    /** The chat a tapped notice asks for, set only by [live.renoa.app.push.NoticeTap] in this process. */
    private var chatToOpen: String? = null

    @Synchronized
    fun openChatNext(agentId: String) {
        chatToOpen = agentId
    }

    /** Takes the chat a tapped notice asked for, once. */
    @Synchronized
    fun takeChatToOpen(): String? = chatToOpen.also { chatToOpen = null }

    /**
     * What a wake-up does on its own connection as this phone, or null while the phone is not signed
     * in. Throws when a sign-in is stored but cannot be read now, which the wake-up tries again.
     */
    @Synchronized
    internal fun wakeUp(): WakeUp? {
        val saved = SignInStore(this).read()?.takeIf { it.origin == RENOA_ORIGIN } ?: return null
        val agents = SavedHome(homeFile(), owner(saved)).load()?.agents.orEmpty()
        val kept = pushing(saved)
        return WakeUp(
            kept.cursors,
            connect = { state -> WebSocketRcpSurface(BuildConfig.RENOA_CONNECT, authentication(saved), state) },
            register = kept.registration::register,
            sender = { task -> sender(task, agents) },
            onScreen = { it in shownChats },
            // Not after a sign-out: the check and the post share the lock signOut drops the sign-in under.
            post = { synchronized(this) { if (pushing === kept) Notices.post(this, it) } },
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
                setupLinks = null
                // Closed, so the old socket stops pinging Renoa.
                surface?.close()
                surface = null
                val watching = network
                network = null
                runCatching { watching?.let { getSystemService(ConnectivityManager::class.java).unregisterNetworkCallback(it) } }
                (outbox to home).also {
                    outbox = null
                    home = null
                }
            }
        val told =
            synchronized(this) {
                Notices.cancelAll(this)
                pushing?.cursors.also { pushing = null }
            }
        DeviceKeys.forget()
        // A Keystore that refuses now must not keep the phone signed in.
        runCatching { noticeKeys.forget() }
        SignInStore(this).clear()
        SavedSignUp(this).operationId = null
        (held ?: SavedOutbox(outboxFile())).forget()
        // Forgetting drops whatever is kept, whoever it was kept for.
        (kept ?: SavedHome(homeFile(), owner = "")).forget()
        (told ?: WakeCursors(wakeFile(), owner = "")).forget()
    }

    private fun signedIn(): SavedSignIn? = SignInStore(this).load()?.takeIf { it.origin == RENOA_ORIGIN }

    /** Kept per sign-in: a device id belongs to one principal, so another owner's Home never shows. */
    private fun owner(saved: SavedSignIn): String {
        val device = saved.credentials.deviceId
        return "${saved.origin} $device"
    }

    private fun authentication(saved: SavedSignIn) = Authentication.Device(saved.credentials) { bytes -> DeviceKeys.getOrCreate().sign(bytes) }

    @Synchronized
    private fun pushing(saved: SavedSignIn): Pushing =
        pushing?.takeIf { it.owner == owner(saved) } ?: run {
            val cursors = WakeCursors(wakeFile(), owner(saved))
            Pushing(owner(saved), cursors, PushRegistration({ firebaseToken(this) }, { Notices.wanted(this) }, cursors)).also { pushing = it }
        }

    private fun outboxFile() = File(filesDir, "outbox")

    private fun homeFile() = File(filesDir, "home")

    private fun wakeFile() = File(filesDir, "wake")

    private val noticeKeys by lazy { NoticeKeys(File(filesDir, "notice-key")) }
}


class MainActivity : ComponentActivity() {
    /** The chat a tapped notice asks for, until the screens open it. */
    private var opening by mutableStateOf<String?>(null)

    override fun onCreate(savedInstanceState: Bundle?) {
        super.onCreate(savedInstanceState)
        // The theme the app resolves sets the bars' colours again on its first frame.
        enableEdgeToEdge()
        val application = application as RenoaApplication
        val store = SignInStore(applicationContext)
        // A credential from another Renoa, as after a build that signs in elsewhere, does not count.
        val signedIn = store.load()?.origin == RENOA_ORIGIN
        opening = application.takeChatToOpen()
        setContent {
            RenoaApp(
                application::repository, signedIn, SignInViewModel.signingIn(applicationContext, store), application::signOut,
                chatShown = application::chatShown, notificationsAnswered = application::notificationsChanged,
                openChat = opening, chatOpened = { opening = null }, setupLinks = application::setupLinks,
                retryNow = application::retryNow,
            )
        }
    }

    /** A tap on a notice while the app runs. No chat is read from [intent]: only the app's own notices name one. */
    override fun onNewIntent(intent: Intent) {
        super.onNewIntent(intent)
        (application as RenoaApplication).takeChatToOpen()?.let { opening = it }
    }

    override fun onStart() {
        super.onStart()
        // The owner is here, so the ring has done its work.
        Notices.cancelRing(this)
        (application as RenoaApplication).run {
            retryNow()
            // Notifications may have been turned on or off in the phone's settings meanwhile.
            notificationsChanged()
        }
    }
}
