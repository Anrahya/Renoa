package live.renoa.app.chat

import androidx.compose.ui.platform.UriHandler
import org.junit.Test
import org.junit.runner.RunWith
import org.robolectric.RobolectricTestRunner
import org.robolectric.annotation.Config
import com.mikepenz.markdown.model.State
import kotlin.test.assertEquals
import kotlin.test.assertIs
import kotlin.test.assertNotSame
import kotlin.test.assertSame

@RunWith(RobolectricTestRunner::class)
@Config(sdk = [36])
class ReplyTest {
    private class Browser : UriHandler {
        val opened = mutableListOf<String>()

        override fun openUri(uri: String) {
            opened += uri
        }
    }

    @Test
    fun onlyWebAddressesLeaveTheAppAndABareWwwOneOpensOverHttps() {
        val browser = Browser()
        val links = WebLinks(browser)
        val tried =
            listOf(
                "https://example.com/a", "HTTP://example.com/b", "www.example.com", "WWW.example.com/c",
                "javascript:alert(1)", "content://media/1", "intent://scan#Intent;end", "tel:5551234", "mailto:me@example.com", "file:///sdcard/a", "example.com",
            )
        tried.forEach(links::openUri)
        assertEquals(listOf("https://example.com/a", "http://example.com/b", "https://www.example.com", "https://WWW.example.com/c"), browser.opened)
    }

    @Test
    fun aPhoneWithNoBrowserIgnoresTheTap() {
        val none = object : UriHandler { override fun openUri(uri: String) = throw IllegalArgumentException("no activity for $uri") }
        WebLinks(none).openUri("https://example.com")
    }

    @Test
    fun aFinishedReplyIsParsedOnceUntilItsTextChangesOrItFallsOutOfTheCache() {
        val parses = ParsedReplies(capacity = 2)
        val first = parses.of("a", "**one**")
        assertSame(first, parses.of("a", "**one**"), "shown again, the parse is reused")
        assertNotSame(first, parses.of("a", "**one, edited**"), "changed text is parsed afresh")
        val b = parses.of("b", "two")
        parses.of("a", "**one, edited**")
        parses.of("c", "three")
        assertNotSame(b, parses.of("b", "two"), "the least recently shown went first")
    }

    @Test
    fun aReplyTheParserCannotReadIsNotKept() {
        val parses = ParsedReplies()
        // Quotes nested this deep overflow the parser's stack.
        val deep = ">".repeat(100_000) + " x"
        val first = parses.of("a", deep)
        assertIs<State.Error>(first)
        assertNotSame(first, parses.of("a", deep), "an unreadable reply is tried again, not kept")
    }
}
