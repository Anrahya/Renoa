package live.renoa.app.signin

import live.renoa.rcp.SignInRefused
import live.renoa.rcp.SignInUncertain
import java.io.IOException
import kotlin.test.Test
import kotlin.test.assertEquals
import kotlin.test.assertFalse
import kotlin.test.assertNull
import kotlin.uuid.Uuid

/** What the sign-in code means, the device's name, and the words a failed sign-in shows. */
class SignInCodeTest {
    private val setup = SignInCode.Setup("ab".repeat(32))
    private val existing = SignInCode.Existing(Uuid.random())

    @Test
    fun anIdSignsInWithAnExistingPasskeyAndAnythingElseIsASetupCode() {
        val principal = Uuid.random()
        assertEquals(SignInCode.Existing(principal), SignInCode.parse(" $principal "))
        assertEquals(setup, SignInCode.parse("ab".repeat(32) + "\n"))
        assertNull(SignInCode.parse("   "))
        assertNull(SignInCode.parse("two words"))
        assertFalse("ab".repeat(32) in setup.toString(), "a setup code is never printed")
    }

    @Test
    fun eachFailureSaysWhatToDoNext() {
        assertEquals("Sign-in stopped. Try again when you're ready.", signInFailure(PasskeyCancelled(), setup))
        assertEquals("Renoa may have signed you in, but the answer was lost. Use a new setup code to finish.", signInFailure(SignInUncertain(IOException()), setup))
        assertEquals(
            "This device already has a passkey for your Renoa. Sign in with your Renoa ID instead of a setup code.",
            signInFailure(PasskeyExists(), setup),
        )
        assertEquals("This device needed a fresh key and has made one. Try again.", signInFailure(SignInRefused(409, "conflict", "this device key was signed out; create a new key to sign in again"), setup))
        assertEquals(
            "Your Renoa already has the most devices it allows. Sign one out, then try again.",
            signInFailure(SignInRefused(409, "conflict", "this account already has 32 signed-in devices; sign one out first"), setup),
        )
        assertEquals("That code didn't work. A setup code works once, for 30 minutes.", signInFailure(SignInRefused(401, "authentication_failed", ""), setup))
        assertEquals("Renoa didn't accept that passkey. Check your Renoa ID and try again.", signInFailure(SignInRefused(401, "authentication_failed", ""), existing))
        assertEquals("Your Renoa isn't ready for app sign-in yet.", signInFailure(SignInRefused(404, "not_found", ""), setup))
        assertEquals("Can't reach Renoa. Check your connection and try again.", signInFailure(IOException("offline"), setup))
    }

    @Test
    fun aDevicesNameIsCutAtSixtyFourCharactersWithoutSplittingOne() {
        assertEquals("Android", deviceLabel(null))
        assertEquals("Yash's tablet", deviceLabel(" Yash's\u0007 tablet "))
        val long = "a".repeat(63) + "\uD83D\uDE00" + "b"
        val label = deviceLabel(long)
        assertEquals(64, label.codePointCount(0, label.length))
        assertEquals("\uD83D\uDE00", label.takeLast(2))
    }
}
