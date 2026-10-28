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
import androidx.credentials.exceptions.domerrors.InvalidStateError
import androidx.credentials.exceptions.publickeycredential.CreatePublicKeyCredentialDomException

/** The person closed the passkey prompt. */
internal class PasskeyCancelled : Exception()

/** This phone holds no passkey for the Renoa it was asked about. */
internal class NoPasskey : Exception()

/** This phone already holds a passkey for this Renoa, so it was not asked to make another. */
internal class PasskeyExists : Exception()

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
                    } catch (refused: CreatePublicKeyCredentialDomException) {
                        // The ceremony lists the owner's passkeys, and the platform refuses to make a second one here.
                        throw if (refused.domError is InvalidStateError) PasskeyExists().initCause(refused) else refused
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
