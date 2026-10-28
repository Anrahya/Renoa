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
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.launch
import kotlinx.coroutines.withContext
import live.renoa.app.BuildConfig
import live.renoa.rcp.PasskeySignIn
import live.renoa.rcp.SignInRefused
import live.renoa.rcp.SignInUncertain
import java.io.IOException
import kotlin.uuid.Uuid

/** The Renoa this app signs in to, set by the build. Its passkeys belong to this origin's relying party. */
internal const val RENOA_ORIGIN = BuildConfig.RENOA_ORIGIN

/** What the person typed: the ID of a passkey they already have, or a one-use setup code for a new one. */
internal sealed interface SignInCode {
    data class Existing(val principalId: Uuid) : SignInCode

    /** The token is a secret until it is spent, so it is never printed. */
    data class Setup(val token: String) : SignInCode {
        override fun toString() = "Setup(<secret>)"
    }

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

/** The coordinator's `conflict` for an account at its limit of signed-in devices (`device_store.rs`). */
private const val DEVICE_LIMIT = "signed-in devices"

/** Words for a failed sign-in with [code] that tell the person what to do next. */
internal fun signInFailure(failure: Throwable, code: SignInCode): String {
    val setup = code is SignInCode.Setup
    return when (failure) {
        is PasskeyCancelled -> "Sign-in stopped. Try again when you're ready."
        is NoPasskey -> "This device has no passkey for your Renoa yet. Use a setup code to make one."
        is PasskeyExists -> "This device already has a passkey for your Renoa. Sign in with your Renoa ID instead of a setup code."
        is SignInUncertain -> "Renoa may have signed you in, but the answer was lost. Use a new setup code to finish."
        is SignInRefused ->
            when {
                failure.code == "conflict" && DEVICE_LIMIT in failure.message.orEmpty() ->
                    "Your Renoa already has the most devices it allows. Sign one out, then try again."
                failure.code == "conflict" -> "This device needed a fresh key and has made one. Try again."
                failure.code == "authentication_failed" && setup -> "That code didn't work. A setup code works once, for 30 minutes."
                failure.code == "authentication_failed" -> "Renoa didn't accept that passkey. Check your Renoa ID and try again."
                failure.code == "capacity_exceeded" -> "Too many sign-ins at once. Wait a minute, then try again."
                failure.code == "not_found" -> "Your Renoa isn't ready for app sign-in yet."
                else -> "Renoa couldn't sign you in just now. Try again in a moment."
            }
        is IOException -> "Can't reach Renoa. Check your connection and try again."
        else -> "Something went wrong signing in. Try again."
    }
}

/** The name the owner will recognize in their list of signed-in apps: at most 64 characters, none of them controls. */
internal fun deviceLabel(name: String?): String {
    val label = name.orEmpty().filterNot(Char::isISOControl).trim()
    if (label.isEmpty()) return "Android"
    val end = if (label.codePointCount(0, label.length) > 64) label.offsetByCodePoints(0, 64) else label.length
    return label.substring(0, end).trim()
}

/** One sign-in: a passkey and this installation's device key, then the credential kept for later. */
internal class SignInViewModel(
    private val signIn: suspend (SignInCode, Passkeys) -> Unit,
) : ViewModel() {
    var state by mutableStateOf<SignInState>(SignInState.Ready)
        private set

    /** What the person typed. It lives here, not in saved state, because a setup code is a secret. */
    var code by mutableStateOf("")

    fun submit(passkeys: Passkeys) {
        if (state == SignInState.Working) return
        val parsed = SignInCode.parse(code) ?: return
        state = SignInState.Working
        viewModelScope.launch {
            state =
                try {
                    signIn(parsed, passkeys)
                    SignInState.SignedIn
                } catch (cancelled: CancellationException) {
                    throw cancelled
                } catch (failure: Exception) {
                    SignInState.Failed(signInFailure(failure, parsed))
                }
        }
    }

    companion object {
        /**
         * Signs in to [RENOA_ORIGIN] and keeps the credential in [store]. The client outlives one
         * attempt, so a cancelled passkey prompt can be tried again without spending a new code.
         */
        fun signingIn(context: Context, store: SignInStore): suspend (SignInCode, Passkeys) -> Unit {
            val name = Settings.Global.getString(context.contentResolver, Settings.Global.DEVICE_NAME) ?: Build.MODEL
            val client = PasskeySignIn(RENOA_ORIGIN, surface = "android", label = deviceLabel(name))
            return { code, passkeys ->
                val key = DeviceKeys.getOrCreate()
                val credentials =
                    try {
                        when (code) {
                            is SignInCode.Setup -> client.register(code.token, key, passkeys.create)
                            is SignInCode.Existing -> client.authenticate(code.principalId, key, passkeys.get)
                        }
                    } catch (refused: SignInRefused) {
                        // A key registered to another sign-in or signed out is never accepted again, so the
                        // next attempt makes a new one. At the device limit a new key changes nothing and costs nothing.
                        if (refused.code == "conflict") DeviceKeys.forget()
                        throw refused
                    }
                withContext(Dispatchers.IO) { store.save(SavedSignIn(RENOA_ORIGIN, credentials)) }
            }
        }
    }
}
