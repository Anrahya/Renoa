package live.renoa.app.signin

import live.renoa.rcp.SignInRefused
import java.io.IOException
import kotlin.test.Test
import kotlin.test.assertEquals
import kotlin.test.assertNull
import kotlin.uuid.Uuid

/** What the sign-in code means, and the words a failed sign-in shows. */
class SignInCodeTest {
    @Test
    fun anIdSignsInWithAnExistingPasskeyAndAnythingElseIsASetupCode() {
        val principal = Uuid.random()
        assertEquals(SignInCode.Existing(principal), SignInCode.parse(" $principal "))
        assertEquals(SignInCode.Setup("ab".repeat(32)), SignInCode.parse("ab".repeat(32) + "\n"))
        assertNull(SignInCode.parse("   "))
        assertNull(SignInCode.parse("two words"))
    }

    @Test
    fun eachFailureSaysWhatToDoNext() {
        assertEquals("Sign-in stopped. Try again when you're ready.", signInFailure(PasskeyCancelled()))
        assertEquals("This device needs a fresh key. Try again and it will make one.", signInFailure(SignInRefused(409, "conflict", "")))
        for (code in listOf("authentication_failed", "not_found", "invalid_request")) {
            assertEquals("That code didn't work. A setup code works once, for 30 minutes.", signInFailure(SignInRefused(400, code, "")))
        }
        assertEquals("Can't reach Renoa. Check your connection and try again.", signInFailure(IOException("offline")))
    }
}
