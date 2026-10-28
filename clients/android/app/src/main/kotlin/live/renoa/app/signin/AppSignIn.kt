package live.renoa.app.signin

import android.content.Context
import android.os.Build
import android.provider.Settings
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.setValue
import androidx.lifecycle.ViewModel
import androidx.lifecycle.viewModelScope
import kotlinx.coroutines.CancellationException
import kotlinx.coroutines.launch
import live.renoa.app.BuildConfig
import live.renoa.rcp.PasskeySignIn
import live.renoa.rcp.SignInRefused
import java.io.IOException
import kotlin.uuid.Uuid

/** The Renoa this app signs in to, set by the build. Its passkeys belong to this origin's relying party. */
internal const val RENOA_ORIGIN = BuildConfig.RENOA_ORIGIN

/** What the person typed: the ID of a passkey they already have, or a one-use setup code for a new one. */
internal sealed interface SignInCode {
    data class Existing(val principalId: Uuid) : SignInCode

    data class Setup(val token: String) : SignInCode

    companion object {
        fun parse(text: String): SignInCode? {
            val trimmed = text.trim()
            if (trimmed.isEmpty() || trimmed.any(Char::isWhitespace)) return null
            return runCatching { Existing(Uuid.parse(trimmed)) }.getOrElse { Setup(trimmed) }
        }
    }
}

internal sealed interface SignInState {
    data object Ready : SignInState

    data object Working : SignInState

    data class Failed(val message: String) : SignInState

    data object SignedIn : SignInState
}

/** Words for a failed sign-in that tell the person what to do next. */
internal fun signInFailure(failure: Throwable): String =
    when (failure) {
        is PasskeyCancelled -> "Sign-in stopped. Try again when you're ready."
        is NoPasskey -> "This device has no passkey for your Renoa yet. Use a setup code to make one."
        is SignInRefused ->
            when (failure.code) {
                "conflict" -> "This device needs a fresh key. Try again and it will make one."
                "authentication_failed", "not_found", "invalid_request" -> "That code didn't work. A setup code works once, for 30 minutes."
                "capacity_exceeded" -> "Too many sign-ins at once. Wait a minute, then try again."
                else -> "Renoa couldn't sign you in just now. Try again in a moment."
            }
        is IOException -> "Can't reach Renoa. Check your connection and try again."
        else -> "Something went wrong signing in. Try again."
    }

/** The name the owner will recognize in their list of signed-in apps. */
internal fun deviceLabel(context: Context): String {
    val named = Settings.Global.getString(context.contentResolver, Settings.Global.DEVICE_NAME)
    val label = (named?.takeIf { it.isNotBlank() } ?: Build.MODEL).filterNot(Char::isISOControl).trim()
    return label.take(64).ifEmpty { "Android" }
}

/** One sign-in: a passkey and this installation's device key, then the credential kept for later. */
internal class SignInViewModel(
    private val signIn: suspend (SignInCode, Passkeys) -> Unit,
) : ViewModel() {
    var state by mutableStateOf<SignInState>(SignInState.Ready)
        private set

    fun submit(text: String, passkeys: Passkeys) {
        if (state == SignInState.Working) return
        val code = SignInCode.parse(text) ?: return
        state = SignInState.Working
        viewModelScope.launch {
            state =
                try {
                    signIn(code, passkeys)
                    SignInState.SignedIn
                } catch (cancelled: CancellationException) {
                    throw cancelled
                } catch (failure: Exception) {
                    SignInState.Failed(signInFailure(failure))
                }
        }
    }

    companion object {
        /** Signs in to [RENOA_ORIGIN] and keeps the credential in [store]. */
        fun signingIn(context: Context, store: SignInStore): suspend (SignInCode, Passkeys) -> Unit = { code, passkeys ->
            val client = PasskeySignIn(RENOA_ORIGIN, surface = "android", label = deviceLabel(context))
            val key = DeviceKeys.getOrCreate()
            val credentials =
                try {
                    when (code) {
                        is SignInCode.Setup -> client.register(code.token, key, passkeys.create)
                        is SignInCode.Existing -> client.authenticate(code.principalId, key, passkeys.get)
                    }
                } catch (refused: SignInRefused) {
                    // A key the coordinator refuses as revoked or another account's is never accepted
                    // again; the next attempt makes a new one.
                    if (refused.code == "conflict") DeviceKeys.forget()
                    throw refused
                }
            store.save(SavedSignIn(RENOA_ORIGIN, credentials))
        }
    }
}
