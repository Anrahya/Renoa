package live.renoa.app.push

import android.content.Context
import androidx.work.Configuration
import androidx.work.WorkManager
import com.google.firebase.messaging.RemoteMessage
import live.renoa.app.signin.SignInStore
import org.junit.Test
import org.junit.runner.RunWith
import org.robolectric.Robolectric
import org.robolectric.RobolectricTestRunner
import org.robolectric.RuntimeEnvironment
import org.robolectric.annotation.Config
import java.util.concurrent.Executor
import java.util.concurrent.Executors
import kotlin.test.assertEquals
import kotlin.test.assertFails
import kotlin.test.assertNull

/** What Firebase hands the app, through the service it really calls, into WorkManager. */
@RunWith(RobolectricTestRunner::class)
@Config(sdk = [36])
class PushMessagesTest {
    private val app = RuntimeEnvironment.getApplication()

    /** Work is queued and never run, so the test sees what a push asked for. */
    private val queued = mutableListOf<Runnable>()

    /** WorkManager as the app starts it, but with work queued and never run. It lives as long as the test JVM. */
    private fun startWorkManager() {
        if (WorkManager.isInitialized()) return
        WorkManager.initialize(app, Configuration.Builder().setExecutor(Executor { queued += it }).setTaskExecutor(Executors.newSingleThreadExecutor()).build())
    }

    private fun waiting(): Int = WorkManager.getInstance(app).getWorkInfosForUniqueWork("wake").get().count { !it.state.isFinished }

    private fun receive(vararg data: Pair<String, String>) {
        val service = Robolectric.setupService(PushMessages::class.java)
        service.onMessageReceived(RemoteMessage.Builder("renoa@fcm.googleapis.com").setData(mapOf(*data)).build())
    }

    @Test
    fun renoasWakesQueueACatchUpAndAnythingElseIsIgnored() {
        startWorkManager()
        receive("other" to "push")
        receive("renoa" to "unknown")
        assertEquals(0, waiting())

        receive("renoa" to "changed")
        receive("renoa" to "ring")
        assertEquals(2, waiting(), "one catch-up runs and the next waits behind it")
    }

    @Test
    fun aStoredSignInThatCannotBeReadIsAFailureNotNoSignIn() {
        val store = SignInStore(app)
        assertNull(store.read(), "nothing stored is no sign-in")
        app.getSharedPreferences("sign-in", Context.MODE_PRIVATE).edit().putString("signed-in", "AAAA").commit()
        assertFails("a sign-in that is there but unreadable is tried again, not dropped") { store.read() }
        assertNull(store.load())
    }
}
