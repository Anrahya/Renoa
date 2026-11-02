package live.renoa.app.push

import kotlin.test.Test
import kotlin.test.assertEquals
import kotlin.test.assertTrue

class PlainTextTest {
    @Test
    fun aRepliesMarksGoAndItsWordsStay() {
        val reply =
            """
            ## Dinner plan

            I found **two** recipes for _tonight_:

            - [Lentil soup](https://example.com/soup), about `30 min`
            * ~~Pasta~~ Risotto
            > Picked from what is in your fridge.

            ```
            shopping list
            ```
            ---
            ![a bowl of soup](https://example.com/soup.png)
            """.trimIndent()
        assertEquals(
            """
            Dinner plan

            I found two recipes for tonight:

            • Lentil soup, about 30 min
            • Pasta Risotto
            Picked from what is in your fridge.

            shopping list
            a bowl of soup
            """.trimIndent(),
            plainText(reply),
        )
    }

    @Test
    fun wordsThatOnlyLookLikeMarksAreLeftAlone() {
        assertEquals("snake_case_name costs 2 * 3 = 6", plainText("snake_case_name costs 2 * 3 = 6"))
    }

    @Test
    fun aLongReplyIsCutToFitWithAnEllipsis() {
        val long = "word ".repeat(400)
        val shown = plainText(long, limit = 100)
        assertEquals("word ".repeat(19) + "word…", shown)
        assertEquals("short", plainText("short", limit = 100))

        val faces = "😀".repeat(60)
        val cut = plainText(faces, limit = 100)
        assertEquals("😀".repeat(49) + "…", cut, "no face is cut in half")
    }

    @Test
    fun aNoticeNeverCarriesMoreThanItsLimit() {
        assertTrue(plainText("x".repeat(10_000)).length <= NOTICE_CHARACTERS)
    }
}
