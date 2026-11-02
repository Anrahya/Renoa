package live.renoa.app.push

import kotlinx.coroutines.CancellationException
import live.renoa.rcp.RcpSurface

/**
 * Tells Renoa, on a connection it accepted, whether and where to wake this phone: the push [token]
 * while the owner [allows] notifications, and no token otherwise, since a wake-up that shows nothing
 * is one Firebase holds against the app. [token] is null when this build has no push, and throws
 * when the token cannot be had right now; that leaves Renoa's registration as it was.
 *
 * It then takes the [cursors] baseline if none was taken, so the first wake-up shows only what is new.
 * Nothing it meets fails the caller: the next connection registers again.
 */
internal class PushRegistration(
    private val token: suspend () -> String?,
    private val allows: () -> Boolean,
    private val cursors: WakeCursors,
) {
    suspend fun register(surface: RcpSurface) {
        quietly {
            val issued = if (allows()) token() else null
            if (issued == null) surface.clearPushToken() else surface.setPushToken(issued)
        }
        quietly { if (!cursors.baselined()) cursors.baseline(surface.listTasks()) }
    }

    private suspend fun quietly(step: suspend () -> Unit) {
        try {
            step()
        } catch (cancelled: CancellationException) {
            throw cancelled
        } catch (_: Exception) {
            // Done again on the next connection.
        }
    }
}
