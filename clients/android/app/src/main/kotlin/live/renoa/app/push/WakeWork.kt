package live.renoa.app.push

import android.content.Context
import androidx.work.Constraints
import androidx.work.CoroutineWorker
import androidx.work.ExistingWorkPolicy
import androidx.work.ForegroundInfo
import androidx.work.NetworkType
import androidx.work.OneTimeWorkRequestBuilder
import androidx.work.OutOfQuotaPolicy
import androidx.work.WorkManager
import androidx.work.WorkerParameters
import kotlinx.coroutines.CancellationException
import live.renoa.app.RenoaApplication
import live.renoa.rcp.RcpErrorCode
import live.renoa.rcp.RcpException

/**
 * Runs a [WakeUp] after Renoa woke the phone or Firebase issued a new token. Wake-ups run one after
 * another, each after the one before. A refused sign-in is not retried; any other failure is, a few
 * times, and after that the next wake-up reads what this one missed.
 */
class WakeWork(context: Context, parameters: WorkerParameters) : CoroutineWorker(context, parameters) {
    override suspend fun doWork(): Result {
        val wake = (applicationContext as RenoaApplication).wakeUp() ?: return Result.success()
        return try {
            wake.run()
            Result.success()
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
        private const val RETRIES = 3
        private const val CHECKING = 2

        fun enqueue(context: Context) {
            val request =
                OneTimeWorkRequestBuilder<WakeWork>()
                    .setExpedited(OutOfQuotaPolicy.RUN_AS_NON_EXPEDITED_WORK_REQUEST)
                    .setConstraints(Constraints.Builder().setRequiredNetworkType(NetworkType.CONNECTED).build())
                    .build()
            WorkManager.getInstance(context).enqueueUniqueWork(NAME, ExistingWorkPolicy.APPEND_OR_REPLACE, request)
        }
    }
}
