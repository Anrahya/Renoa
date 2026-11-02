package live.renoa.app.push

import kotlinx.coroutines.CancellationException
import kotlinx.coroutines.sync.Mutex
import kotlinx.coroutines.sync.withLock
import live.renoa.rcp.RcpSurface

/**
 * Tells Renoa, on a connection it accepted, whether and where to wake this phone: the push [token]
 * while the owner [allows] notifications, and no token otherwise, since a wake-up that shows nothing
 * is one Firebase holds against the app. [token] is null when this build has no push, and throws
 * when the token cannot be had right now, which leaves Renoa's registration as it was.
 *
 * It then takes the [cursors] baseline if none was taken, so the first wake-up shows only what is new.
 * Both steps are tried; [register] throws the first failure after that, for the caller to retry now
 * or on the next connection. One registration runs at a time, each reading [allows] once it runs,
 * so an older answer never lands after a newer one.
 */
internal class PushRegistration(
    private val token: suspend () -> String?,
    private val allows: () -> Boolean,
    private val cursors: WakeCursors,
) {
    private val registering = Mutex()

    suspend fun register(surface: RcpSurface) {
        val told =
            attempt {
                registering.withLock {
                    val issued = if (allows()) token() else null
                    if (issued == null) surface.clearPushToken() else surface.setPushToken(issued)
                }
            }
        val baselined = attempt { if (!cursors.baselined()) cursors.baseline(surface.listTasks()) }
        (told ?: baselined)?.let { throw it }
    }

    /** Runs [step] and returns what it failed with, or null. */
    private suspend fun attempt(step: suspend () -> Unit): Exception? =
        try {
            step()
            null
        } catch (cancelled: CancellationException) {
            throw cancelled
        } catch (failure: Exception) {
            failure
        }
}
