package live.renoa.app.push

import android.content.Context
import com.google.firebase.FirebaseApp
import com.google.firebase.messaging.FirebaseMessaging
import com.google.firebase.messaging.FirebaseMessagingService
import com.google.firebase.messaging.RemoteMessage
import kotlinx.coroutines.suspendCancellableCoroutine
import java.io.IOException
import kotlin.coroutines.resume
import kotlin.coroutines.resumeWithException

/** Whether a push is Renoa's wake-up. It carries nothing else: what changed is read over RCP. */
internal fun isWake(data: Map<String, String>): Boolean = data["renoa"] == "changed"

/**
 * Where Firebase delivers Renoa's wake-ups and this install's new tokens. Both only start a
 * [WakeWork]: Firebase allows the callback a few seconds and no network, and a new token reaches
 * Renoa on the job's connection.
 */
class PushMessages : FirebaseMessagingService() {
    override fun onMessageReceived(message: RemoteMessage) {
        if (isWake(message.data)) WakeWork.enqueue(this)
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
 * configuration. Throws when Firebase cannot issue one right now.
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
    return suspendCancellableCoroutine { waiting ->
        issuing.addOnCompleteListener { done ->
            if (done.isSuccessful) waiting.resume(done.result) else waiting.resumeWithException(done.exception ?: IOException("Firebase issued nothing"))
        }
    }
}
