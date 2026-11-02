package live.renoa.app.push

import android.Manifest
import android.app.Notification
import android.app.NotificationManager
import org.junit.Test
import org.junit.runner.RunWith
import org.robolectric.RobolectricTestRunner
import org.robolectric.RuntimeEnvironment
import org.robolectric.Shadows.shadowOf
import org.robolectric.annotation.Config
import kotlin.test.assertEquals
import kotlin.test.assertFalse
import kotlin.test.assertNull
import kotlin.test.assertTrue
import kotlin.uuid.Uuid

@RunWith(RobolectricTestRunner::class)
@Config(sdk = [36])
class NoticesTest {
    private val app = RuntimeEnvironment.getApplication()
    private val manager = shadowOf(app.getSystemService(NotificationManager::class.java))
    private val chat = Uuid.random()

    private fun shown() = manager.allNotifications

    @Test
    fun aReplyShowsUnderItsAgentsNameAndTheNextNoticeFromThatChatReplacesIt() {
        shadowOf(app).grantPermissions(Manifest.permission.POST_NOTIFICATIONS)
        Notices.post(app, Notice(chat, "Arcee", null, News.Replied("Dinner is at seven.")))
        val reply = shown().single()
        assertEquals("Arcee", reply.extras.getString(Notification.EXTRA_TITLE))
        assertEquals("Dinner is at seven.", reply.extras.getCharSequence(Notification.EXTRA_TEXT).toString())
        assertEquals("replies", reply.channelId)

        Notices.post(app, Notice(chat, null, null, News.Stuck))
        val stuck = shown().single()
        assertEquals("Renoa", stuck.extras.getString(Notification.EXTRA_TITLE), "an agent the app does not know goes under the app's name")
        assertEquals(STUCK_WORDS, stuck.extras.getCharSequence(Notification.EXTRA_TEXT).toString())

        Notices.cancel(app, chat)
        assertTrue(shown().isEmpty(), "taken back when the chat comes on screen")
    }

    @Test
    fun aReplyReadsAsPlainWordsAndATapOpensItsChat() {
        shadowOf(app).grantPermissions(Manifest.permission.POST_NOTIFICATIONS)
        val other = Uuid.random()
        Notices.post(app, Notice(chat, "Arcee", "node/agent:arcee", News.Replied("**Dinner** is at [seven](https://example.com).")))
        Notices.post(app, Notice(other, "Ledger", null, News.Stuck))

        val reply = shown().single { it.extras.getString(Notification.EXTRA_TITLE) == "Arcee" }
        assertEquals("Dinner is at seven.", reply.extras.getCharSequence(Notification.EXTRA_TEXT).toString())
        assertTrue(reply.flags and Notification.FLAG_ONLY_ALERT_ONCE != 0, "a notice shown again does not ring again")
        val tap = shadowOf(reply.contentIntent).savedIntent
        assertEquals(OPEN_CHAT, tap.action)
        assertEquals("node/agent:arcee", tap.getStringExtra(CHAT))

        val stuck = shown().single { it.extras.getString(Notification.EXTRA_TITLE) == "Ledger" }
        assertNull(shadowOf(stuck.contentIntent).savedIntent.getStringExtra(CHAT), "a task that is no chat opens the app, and each notice keeps its own tap")
    }

    @Test
    fun nothingShowsWithoutPermissionOrWithNotificationsTurnedOff() {
        Notices.post(app, Notice(chat, "Arcee", null, News.Replied("Unseen.")))
        assertTrue(shown().isEmpty(), "Android 13 and later need the owner's permission")

        shadowOf(app).grantPermissions(Manifest.permission.POST_NOTIFICATIONS)
        manager.setNotificationsEnabled(false)
        assertFalse(Notices.allowed(app))
        Notices.post(app, Notice(chat, "Arcee", null, News.Replied("Unseen.")))
        assertTrue(shown().isEmpty())
    }
}
