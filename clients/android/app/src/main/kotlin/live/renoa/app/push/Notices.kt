package live.renoa.app.push

import android.Manifest
import android.app.Notification
import android.app.PendingIntent
import android.content.Context
import android.content.Intent
import android.content.pm.PackageManager
import android.net.Uri
import android.os.Build
import androidx.core.app.NotificationChannelCompat
import androidx.core.app.NotificationCompat
import androidx.core.app.NotificationManagerCompat
import androidx.core.content.ContextCompat
import live.renoa.app.MainActivity
import live.renoa.app.R
import kotlin.uuid.Uuid

/** The words a notice shows when the agent's run failed. */
internal const val STUCK_WORDS = "It got stuck. Open the chat to try again."

/** What a tap on a notice asks [MainActivity] to do, and the agent whose chat it opens. */
internal const val OPEN_CHAT = "live.renoa.app.OPEN_CHAT"
internal const val CHAT = "chat"

/** What a tap on [notice] opens: the agent's chat when the notice came from it, else the app as it was. */
internal fun noticeTap(context: Context, notice: Notice): Intent =
    Intent(context, MainActivity::class.java)
        .setAction(OPEN_CHAT)
        // One per task, so each notice keeps its own tap.
        .setData(Uri.fromParts("renoa", notice.taskId.toString(), null))
        .apply { notice.chat?.let { putExtra(CHAT, it) } }
        // MainActivity is singleTop: a running app gets the tap in onNewIntent, with its screens kept.
        .addFlags(Intent.FLAG_ACTIVITY_NEW_TASK or Intent.FLAG_ACTIVITY_CLEAR_TOP)

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
                News.Stuck -> STUCK_WORDS
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
                // A notice replaced by the next from the same chat, or shown again by a retry, does not ring twice.
                .setOnlyAlertOnce(true)
                .build()
        NotificationManagerCompat.from(context).notify(notice.taskId.toString(), NOTICE, notification)
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
