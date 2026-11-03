package live.renoa.app.push

import android.content.Context
import com.google.firebase.FirebaseApp
import com.google.firebase.messaging.FirebaseMessaging
import com.google.firebase.messaging.FirebaseMessagingService
import com.google.firebase.messaging.RemoteMessage
import kotlinx.coroutines.suspendCancellableCoroutine
import kotlinx.coroutines.withTimeoutOrNull
import java.io.IOException
import kotlin.coroutines.resume
import kotlin.coroutines.resumeWithException
import kotlin.time.Duration.Companion.seconds

/** One of Renoa's wake-ups. It carries nothing else: what changed is read over RCP. */
internal enum class Wake {
    /** Something changed: read it, and tell the owner of any reply. */
    Changed,

    /** Something needs the owner within minutes: ring, then read as for [Changed]. */
    Ring,
}

/** Which of Renoa's wake-ups a push is, or null for anything else, which is ignored. */
internal fun wakeOf(data: Map<String, String>): Wake? =
    when (data["renoa"]) {
        "changed" -> Wake.Changed
        "ring" -> Wake.Ring
        else -> null
    }

/** How long the app waits for Firebase to issue a token before it gives up until the next try. */
private val TOKEN_WAIT = 30.seconds

/**
 * Where Firebase delivers Renoa's wake-ups and this install's new tokens. Both start a [WakeWork]:
 * Firebase allows the callback a few seconds and no network, and a new token reaches Renoa on the
 * job's connection. A ring also rings at once, since it needs no network to say Renoa needs the owner.
 */
class PushMessages : FirebaseMessagingService() {
    override fun onMessageReceived(message: RemoteMessage) {
        when (wakeOf(message.data)) {
            Wake.Changed -> WakeWork.enqueue(this)
            Wake.Ring -> {
                Notices.ring(this)
                WakeWork.enqueue(this)
            }
            null -> {}
        }
    }

    // Registration tokens, which firebase-messaging 26 deprecates for installation IDs, are what
    // Renoa's FCM HTTP v1 sender addresses as `message.token`; see firebaseToken.
    @Suppress("OVERRIDE_DEPRECATION")
    override fun onNewToken(token: String) {
        WakeWork.enqueue(this)
    }
}

/**
 * This install's Firebase Cloud Messaging registration token, or null when the build has no Firebase
 * configuration. Throws when Firebase cannot issue one right now, or does not within [TOKEN_WAIT].
 *
 * firebase-messaging 26 deprecates registration tokens in favour of installation IDs, which it
 * issues only once the app opts in (`firebase_messaging_installation_id_enabled`). Whether HTTP v1
 * accepts an installation ID as `message.token` is not settled (issue #116, "Left open"), so the app
 * keeps the token the sender is known to accept.
 */
internal suspend fun firebaseToken(context: Context): String? {
    if (FirebaseApp.getApps(context).isEmpty()) return null
    @Suppress("DEPRECATION")
    val issuing = FirebaseMessaging.getInstance().token
    return withTimeoutOrNull(TOKEN_WAIT) {
        suspendCancellableCoroutine { waiting ->
            issuing.addOnCompleteListener { done ->
                if (done.isSuccessful) waiting.resume(done.result) else waiting.resumeWithException(done.exception ?: IOException("Firebase issued nothing"))
            }
        }
    } ?: throw IOException("Firebase did not answer within $TOKEN_WAIT")
}
