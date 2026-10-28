package live.renoa.app.signin

import android.app.Activity
import androidx.credentials.CreatePublicKeyCredentialRequest
import androidx.credentials.CreatePublicKeyCredentialResponse
import androidx.credentials.CredentialManager
import androidx.credentials.GetCredentialRequest
import androidx.credentials.GetPublicKeyCredentialOption
import androidx.credentials.PublicKeyCredential
import androidx.credentials.exceptions.CreateCredentialCancellationException
import androidx.credentials.exceptions.GetCredentialCancellationException
import androidx.credentials.exceptions.NoCredentialException

/** The person closed the passkey prompt. */
internal class PasskeyCancelled : Exception()

/** This phone holds no passkey for the Renoa it was asked about. */
internal class NoPasskey : Exception()

/**
 * The platform's passkey prompts. Each takes the WebAuthn options as JSON and returns the
 * credential the platform made, as WebAuthn JSON.
 */
internal class Passkeys(val create: suspend (String) -> String, val get: suspend (String) -> String) {
    companion object {
        /** Credential Manager, which shows its prompt over [activity]. */
        fun of(activity: Activity): Passkeys {
            val manager = CredentialManager.create(activity)
            return Passkeys(
                create = { options ->
                    try {
                        val response = manager.createCredential(activity, CreatePublicKeyCredentialRequest(options))
                        (response as CreatePublicKeyCredentialResponse).registrationResponseJson
                    } catch (cancelled: CreateCredentialCancellationException) {
                        throw PasskeyCancelled().initCause(cancelled)
                    }
                },
                get = { options ->
                    try {
                        val response = manager.getCredential(activity, GetCredentialRequest(listOf(GetPublicKeyCredentialOption(options))))
                        (response.credential as PublicKeyCredential).authenticationResponseJson
                    } catch (cancelled: GetCredentialCancellationException) {
                        throw PasskeyCancelled().initCause(cancelled)
                    } catch (none: NoCredentialException) {
                        throw NoPasskey().initCause(none)
                    }
                },
            )
        }
    }
}
