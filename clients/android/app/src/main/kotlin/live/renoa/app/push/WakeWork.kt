package live.renoa.app.push

import android.content.Context
import android.os.SystemClock
import androidx.work.Constraints
import androidx.work.CoroutineWorker
import androidx.work.ExistingWorkPolicy
import androidx.work.ForegroundInfo
import androidx.work.NetworkType
import androidx.work.OneTimeWorkRequestBuilder
import androidx.work.OutOfQuotaPolicy
import androidx.work.WorkManager
import androidx.work.WorkerParameters
import androidx.work.workDataOf
import kotlinx.coroutines.CancellationException
import live.renoa.app.RenoaApplication
import live.renoa.rcp.RcpErrorCode
import live.renoa.rcp.RcpException

/**
 * Which wake-ups a catch-up already covered. A catch-up that started after a wake-up was enqueued
 * and finished read everything Renoa had committed when it woke the phone, so that wake-up has
 * nothing left to do. Readings come from [now], a clock that only moves forward while the phone is
 * on; it starts again after a reboot, and this record starts again with the process, so a reading
 * from before either is never taken as covered.
 */
internal class Coverage(private val now: () -> Long) {
    private var through = Long.MIN_VALUE

    fun now(): Long = now.invoke()

    /** Whether a catch-up that started after [enqueuedAt] has finished. */
    @Synchronized
    fun covers(enqueuedAt: Long): Boolean = through > enqueuedAt

    /** A catch-up that started at [startedAt] finished. */
    @Synchronized
    fun finished(startedAt: Long) {
        through = maxOf(through, startedAt)
    }
}

/**
 * Runs a [WakeUp] after Renoa woke the phone or Firebase issued a new token. Wake-ups run one after
 * another in the order they came; one whose wake a later catch-up already covered ends at once, so
 * however many wait, at most one reads. A sign-in Renoa refused is not retried; anything else that
 * failed, including a sign-in the Keystore could not read just now, is, a few times, and after that
 * the next wake-up reads what this one missed.
 */
class WakeWork(context: Context, parameters: WorkerParameters) : CoroutineWorker(context, parameters) {
    override suspend fun doWork(): Result {
        if (coverage.covers(inputData.getLong(ENQUEUED_AT, Long.MAX_VALUE))) return Result.success()
        val startedAt = coverage.now()
        return try {
            val wake = (applicationContext as RenoaApplication).wakeUp() ?: return Result.success()
            if (wake.run()) {
                coverage.finished(startedAt)
                Result.success()
            } else {
                again()
            }
        } catch (cancelled: CancellationException) {
            throw cancelled
        } catch (refused: RcpException) {
            if (refused.code == RcpErrorCode.AuthenticationFailed) Result.failure() else again()
        } catch (_: Exception) {
            again()
        }
    }

    private fun again(): Result = if (runAttemptCount < RETRIES) Result.retry() else Result.failure()

    override suspend fun getForegroundInfo(): ForegroundInfo = ForegroundInfo(CHECKING, Notices.checking(applicationContext))

    companion object {
        private const val NAME = "wake"
        private const val ENQUEUED_AT = "enqueued_at"
        private const val RETRIES = 3
        private const val CHECKING = 2
        private val coverage = Coverage { SystemClock.elapsedRealtimeNanos() }

        fun enqueue(context: Context) {
            val request =
                OneTimeWorkRequestBuilder<WakeWork>()
                    .setExpedited(OutOfQuotaPolicy.RUN_AS_NON_EXPEDITED_WORK_REQUEST)
                    .setConstraints(Constraints.Builder().setRequiredNetworkType(NetworkType.CONNECTED).build())
                    .setInputData(workDataOf(ENQUEUED_AT to coverage.now()))
                    .build()
            WorkManager.getInstance(context).enqueueUniqueWork(NAME, ExistingWorkPolicy.APPEND_OR_REPLACE, request)
        }
    }
}
