package live.renoa.app.signup

import android.annotation.SuppressLint
import android.content.Context

/** [SignUpMemory] on this phone. */
internal class SavedSignUp(context: Context) : SignUpMemory {
    private val preferences = context.getSharedPreferences("sign-up", Context.MODE_PRIVATE)

    override var operationId: String?
        get() = preferences.getString(OPERATION, null)

        // commit, not apply: the id must be on disk before the request that carries it leaves.
        @SuppressLint("UseKtx", "ApplySharedPref")
        set(value) {
            check(preferences.edit().putString(OPERATION, value).commit()) { "could not keep the sign-up's operation id" }
        }

    private companion object {
        const val OPERATION = "renoa-operation"
    }
}
