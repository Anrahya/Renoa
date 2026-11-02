package live.renoa.app.push

import android.Manifest
import android.app.Activity
import android.app.Notification
import android.app.NotificationManager
import android.app.PendingIntent
import android.content.Context
import android.content.Intent
import android.content.pm.PackageManager
import android.net.Uri
import android.os.Build
import android.os.Bundle
import androidx.core.app.NotificationChannelCompat
import androidx.core.app.NotificationCompat
import androidx.core.app.NotificationManagerCompat
import androidx.core.content.ContextCompat
import live.renoa.app.MainActivity
import live.renoa.app.R
import live.renoa.app.RenoaApplication
import kotlin.uuid.Uuid

/** The words a notice shows when the agent's run failed. */
internal const val STUCK_WORDS = "It got stuck. Open the chat to try again."

/** The agent whose chat a tapped notice opens, as [NoticeTap] receives it. */
internal const val CHAT = "chat"

/**
 * What a tap on [notice] starts: [NoticeTap], which only this app can start, with the agent whose
 * chat to open when the notice came from it.
 */
internal fun noticeTap(context: Context, notice: Notice): Intent =
    Intent(context, NoticeTap::class.java)
        // One per task, so each notice keeps its own tap.
        .setData(Uri.fromParts("renoa", notice.taskId.toString(), null))
        .apply { notice.chat?.let { putExtra(CHAT, it) } }
        .addFlags(Intent.FLAG_ACTIVITY_NEW_TASK)

/** The record a posted notice shows, kept on it so the same record posted again stays silent. */
private const val SEQUENCE = "live.renoa.app.sequence"

/**
 * The tap of a notice, not exported, so no other app can open a chat through it. It hands the chat
 * to the app in this process and brings [MainActivity] forward, which never reads a chat from an
 * intent, so an intent from outside opens nothing.
 */
class NoticeTap : Activity() {
    override fun onCreate(savedInstanceState: Bundle?) {
        super.onCreate(savedInstanceState)
        intent.getStringExtra(CHAT)?.let { (application as RenoaApplication).openChatNext(it) }
        // MainActivity is singleTop: a running app gets this in onNewIntent, with its screens kept.
        startActivity(Intent(this, MainActivity::class.java).addFlags(Intent.FLAG_ACTIVITY_NEW_TASK or Intent.FLAG_ACTIVITY_CLEAR_TOP))
        finish()
    }
}

/**
 * The app's notifications: one per task, replaced by the next notice from the same task, on the
 * replies channel the owner can turn off in the phone's settings.
 */
internal object Notices {
    private const val REPLIES = "replies"
    private const val CHECKING = "checking"
    private const val NOTICE = 1

    fun createChannels(context: Context) {
        NotificationManagerCompat.from(context).createNotificationChannel(
            NotificationChannelCompat.Builder(REPLIES, NotificationManagerCompat.IMPORTANCE_HIGH)
                .setName("Replies")
                .setDescription("When an agent answers you or gets stuck")
                .build(),
        )
    }

    /** Whether the owner lets the app show replies: notifications on for the app and for its replies. */
    fun allowed(context: Context): Boolean {
        val manager = NotificationManagerCompat.from(context)
        if (!manager.areNotificationsEnabled()) return false
        return manager.getNotificationChannelCompat(REPLIES)?.importance != NotificationManagerCompat.IMPORTANCE_NONE
    }

    fun post(context: Context, notice: Notice) {
        if (!allowed(context)) return
        if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.TIRAMISU &&
            ContextCompat.checkSelfPermission(context, Manifest.permission.POST_NOTIFICATIONS) != PackageManager.PERMISSION_GRANTED
        ) {
            return
        }
        val text =
            when (val news = notice.news) {
                is News.Replied -> plainText(news.text)
                is News.Stuck -> STUCK_WORDS
            }
        val tag = notice.taskId.toString()
        // The same record again, as on a retry, replaces its notice silently; a newer one rings.
        val shownAlready =
            context.getSystemService(NotificationManager::class.java).activeNotifications.any {
                it.tag == tag && it.id == NOTICE && it.notification.extras.getLong(SEQUENCE, -1) == notice.news.sequence
            }
        val open = PendingIntent.getActivity(context, 0, noticeTap(context, notice), PendingIntent.FLAG_IMMUTABLE or PendingIntent.FLAG_UPDATE_CURRENT)
        val notification =
            NotificationCompat.Builder(context, REPLIES)
                .setSmallIcon(R.drawable.ic_notification)
                .setContentTitle(notice.agent ?: context.getString(R.string.app_name))
                .setContentText(text)
                .setStyle(NotificationCompat.BigTextStyle().bigText(text))
                .setCategory(NotificationCompat.CATEGORY_MESSAGE)
                .setContentIntent(open)
                .setAutoCancel(true)
                .setOnlyAlertOnce(shownAlready)
                .addExtras(Bundle().apply { putLong(SEQUENCE, notice.news.sequence) })
                .build()
        NotificationManagerCompat.from(context).notify(tag, NOTICE, notification)
    }

    /** Takes back the notice from [taskId], as when its chat comes on screen. */
    fun cancel(context: Context, taskId: Uuid) = NotificationManagerCompat.from(context).cancel(taskId.toString(), NOTICE)

    fun cancelAll(context: Context) = NotificationManagerCompat.from(context).cancelAll()

    /** What Android 11 and earlier show while a wake-up runs, which they run as a foreground service. */
    fun checking(context: Context): Notification {
        NotificationManagerCompat.from(context).createNotificationChannel(
            NotificationChannelCompat.Builder(CHECKING, NotificationManagerCompat.IMPORTANCE_MIN)
                .setName("Checking for replies")
                .setDescription("Shown for a moment while the app checks for new replies")
                .build(),
        )
        return NotificationCompat.Builder(context, CHECKING)
            .setSmallIcon(R.drawable.ic_notification)
            .setContentTitle("Checking for replies")
            .setSilent(true)
            .build()
    }
}
