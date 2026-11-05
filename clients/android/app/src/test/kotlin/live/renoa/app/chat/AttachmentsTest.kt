package live.renoa.app.chat

import live.renoa.rcp.Attachment
import kotlin.test.Test
import kotlin.test.assertEquals
import kotlin.test.assertTrue

class AttachmentsTest {
    @Test
    fun aPickedNameBecomesOneRenoaTakesKeepingWhatItCan() {
        assertEquals("fridge.jpg", sendableName("fridge.jpg"))
        assertEquals("file", sendableName(null))
        assertEquals("file", sendableName(" \u0007 "))
        assertEquals("a b.txt", sendableName("a\nb.txt"))
        val long = sendableName("冷".repeat(100) + ".jpg")
        assertTrue(Attachment.validName(long), long)
        assertEquals("冷".repeat(83) + ".jpg", long, "the stem loses its end, the extension stays")
        val emoji = sendableName("🍅".repeat(70))
        assertTrue(Attachment.validName(emoji) && emoji.all { !it.isSurrogate() || emoji.count { c -> c.isSurrogate() } % 2 == 0 }, emoji)
    }
}
