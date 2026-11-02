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
        Notices.post(app, Notice(chat, "Arcee", News.Replied("Dinner is at seven.")))
        val reply = shown().single()
        assertEquals("Arcee", reply.extras.getString(Notification.EXTRA_TITLE))
        assertEquals("Dinner is at seven.", reply.extras.getCharSequence(Notification.EXTRA_TEXT).toString())
        assertEquals("replies", reply.channelId)

        Notices.post(app, Notice(chat, null, News.Stuck))
        val stuck = shown().single()
        assertEquals("Renoa", stuck.extras.getString(Notification.EXTRA_TITLE), "an agent the app does not know goes under the app's name")
        assertEquals(STUCK_WORDS, stuck.extras.getCharSequence(Notification.EXTRA_TEXT).toString())

        Notices.cancel(app, chat)
        assertTrue(shown().isEmpty(), "taken back when the chat comes on screen")
    }

    @Test
    fun nothingShowsWithoutPermissionOrWithNotificationsTurnedOff() {
        Notices.post(app, Notice(chat, "Arcee", News.Replied("Unseen.")))
        assertTrue(shown().isEmpty(), "Android 13 and later need the owner's permission")

        shadowOf(app).grantPermissions(Manifest.permission.POST_NOTIFICATIONS)
        manager.setNotificationsEnabled(false)
        assertFalse(Notices.allowed(app))
        Notices.post(app, Notice(chat, "Arcee", News.Replied("Unseen.")))
        assertTrue(shown().isEmpty())
    }
}
