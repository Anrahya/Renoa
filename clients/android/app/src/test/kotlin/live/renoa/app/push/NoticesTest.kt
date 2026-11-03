package live.renoa.app.push

import android.Manifest
import android.app.Notification
import android.app.NotificationManager
import android.content.ComponentName
import android.content.Intent
import live.renoa.app.MainActivity
import live.renoa.app.RenoaApplication
import org.robolectric.Robolectric
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
        Notices.post(app, Notice(chat, "Arcee", null, News.Replied("Dinner is at seven.", 3)))
        val reply = shown().single()
        assertEquals("Arcee", reply.extras.getString(Notification.EXTRA_TITLE))
        assertEquals("Dinner is at seven.", reply.extras.getCharSequence(Notification.EXTRA_TEXT).toString())
        assertEquals("replies", reply.channelId)

        Notices.post(app, Notice(chat, null, null, News.Stuck(4)))
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
        Notices.post(app, Notice(chat, "Arcee", "node/agent:arcee", News.Replied("**Dinner** is at [seven](https://example.com).", 3)))
        Notices.post(app, Notice(other, "Ledger", null, News.Stuck(2)))

        val reply = shown().single { it.extras.getString(Notification.EXTRA_TITLE) == "Arcee" }
        assertEquals("Dinner is at seven.", reply.extras.getCharSequence(Notification.EXTRA_TEXT).toString())
        val tap = shadowOf(reply.contentIntent).savedIntent
        assertEquals(NoticeTap::class.java.name, tap.component?.className, "a tap goes through the entry only this app can start")
        assertEquals("node/agent:arcee", tap.getStringExtra(CHAT))

        val stuck = shown().single { it.extras.getString(Notification.EXTRA_TITLE) == "Ledger" }
        assertNull(shadowOf(stuck.contentIntent).savedIntent.getStringExtra(CHAT), "a task that is no chat opens the app, and each notice keeps its own tap")
    }

    @Test
    fun aNewReplyRingsEvenOverAnUnreadNoticeAndTheSameReplyPostedAgainDoesNot() {
        shadowOf(app).grantPermissions(Manifest.permission.POST_NOTIFICATIONS)
        fun rings() = shown().single().flags and Notification.FLAG_ONLY_ALERT_ONCE == 0

        Notices.post(app, Notice(chat, "Arcee", null, News.Replied("Dinner is at seven.", 3)))
        assertTrue(rings())
        Notices.post(app, Notice(chat, "Arcee", null, News.Replied("Dinner is at seven.", 3)))
        assertFalse(rings(), "a retry that posts the same reply again is silent")
        Notices.post(app, Notice(chat, "Arcee", null, News.Replied("Bring a dessert.", 5)))
        assertTrue(rings(), "a new reply rings although the last notice was never opened")
    }

    @Test
    fun aTapHandsTheChatToTheAppAndNoOtherAppCanStartIt() {
        val tap = Robolectric.buildActivity(NoticeTap::class.java, Intent(app, NoticeTap::class.java).putExtra(CHAT, "node/agent:arcee")).create().get()
        val renoa = app as RenoaApplication
        assertEquals("node/agent:arcee", renoa.takeChatToOpen())
        assertNull(renoa.takeChatToOpen(), "opened once")
        val next = shadowOf(tap).nextStartedActivity
        assertEquals(MainActivity::class.java.name, next.component?.className)
        assertNull(next.extras?.getString(CHAT), "the chat does not travel in an intent MainActivity would read")
        assertFalse(app.packageManager.getActivityInfo(ComponentName(app, NoticeTap::class.java), 0).exported)
    }

    @Test
    fun aRingSaysOnlyThatRenoaNeedsTheOwnerRingsAsARingtoneAndALaterRingReplacesIt() {
        shadowOf(app).grantPermissions(Manifest.permission.POST_NOTIFICATIONS)
        Notices.post(app, Notice(chat, "Arcee", null, News.Replied("Dinner is at seven.", 3)))
        Notices.ring(app)
        Notices.ring(app)
        val ring = shown().single { it.channelId == "needs_you" }
        assertEquals(RING_TITLE, ring.extras.getString(Notification.EXTRA_TITLE))
        assertEquals(RING_WORDS, ring.extras.getCharSequence(Notification.EXTRA_TEXT).toString())
        assertEquals(2, shown().size, "one ring beside the reply, which it does not replace")
        val channel = app.getSystemService(NotificationManager::class.java).getNotificationChannel("needs_you")
        assertEquals(NotificationManager.IMPORTANCE_HIGH, channel.importance)
        assertEquals(android.media.AudioAttributes.USAGE_NOTIFICATION_RINGTONE, channel.audioAttributes.usage)
        val tap = shadowOf(ring.contentIntent).savedIntent
        assertEquals(NoticeTap::class.java.name, tap.component?.className)
        assertNull(tap.getStringExtra(CHAT), "a ring opens the app, not a chat")

        Notices.cancelRing(app)
        assertEquals("replies", shown().single().channelId, "opening the app takes back the ring and only the ring")
    }

    @Test
    fun aRingStaysQuietWithoutPermissionOrWithNotificationsTurnedOff() {
        Notices.ring(app)
        assertTrue(shown().isEmpty(), "Android 13 and later need the owner's permission")
        shadowOf(app).grantPermissions(Manifest.permission.POST_NOTIFICATIONS)
        manager.setNotificationsEnabled(false)
        Notices.ring(app)
        assertTrue(shown().isEmpty(), "the owner turned notifications off")
    }

    @Test
    fun nothingShowsWithoutPermissionOrWithNotificationsTurnedOff() {
        Notices.post(app, Notice(chat, "Arcee", null, News.Replied("Unseen.", 1)))
        assertTrue(shown().isEmpty(), "Android 13 and later need the owner's permission")

        shadowOf(app).grantPermissions(Manifest.permission.POST_NOTIFICATIONS)
        manager.setNotificationsEnabled(false)
        assertFalse(Notices.allowed(app))
        assertFalse(Notices.wanted(app), "push is not registered when nothing could be shown")
        Notices.post(app, Notice(chat, "Arcee", null, News.Replied("Unseen.", 1)))
        assertTrue(shown().isEmpty())
    }
}
