package live.renoa.app.push

import kotlin.uuid.Uuid

/**
 * The chats on screen right now, by task, so a wake-up does not notify about what the owner is
 * reading. A chat counts while its screen is started; two screens of one chat, as mid-transition,
 * count until both have gone.
 */
internal class ShownChats {
    private val shown = HashMap<Uuid, Int>()

    @Synchronized
    fun show(taskId: Uuid) {
        shown[taskId] = (shown[taskId] ?: 0) + 1
    }

    @Synchronized
    fun hide(taskId: Uuid) {
        val left = (shown[taskId] ?: return) - 1
        if (left == 0) shown.remove(taskId) else shown[taskId] = left
    }

    @Synchronized
    operator fun contains(taskId: Uuid): Boolean = taskId in shown
}
