package live.renoa.app

import java.io.File
import kotlin.test.Test
import kotlin.test.assertEquals
import kotlin.test.assertTrue

/**
 * Nothing the app says uses plumbing words: no tool names, URLs, model names or context. The
 * app's words live in string literals and resources, so this reads every one.
 */
class VocabularyTest {
    private val banned =
        Regex(
            "\\b(rcp|mcp|https?|wss?|tools?|models?|prompts?|tokens?|json|websocket|api|endpoint|sockets?|context|bearer|credentials?|plugins?|extensions?)\\b|://",
            RegexOption.IGNORE_CASE,
        )
    private val literal = Regex("\"((?:[^\"\\\\\\n]|\\\\.)*)\"")

    private fun sources(): List<File> =
        File("src/main/kotlin").walkTopDown().filter { it.extension == "kt" }.toList()

    @Test
    fun theScanFindsTheAppsWords() {
        val literals = sources().flatMap { file -> literal.findAll(file.readText()).map { it.groupValues[1] }.toList() }
        assertTrue(sources().size > 20, "expected to find the app's sources, found ${sources().size}")
        assertTrue("Continue with passkey" in literals && "Busy now" in literals, "the scan is not reading string literals")
    }

    @Test
    fun noStringTheAppCanShowUsesPlumbingVocabulary() {
        val offences = mutableListOf<String>()
        for (file in sources()) {
            for (match in literal.findAll(file.readText())) {
                val text = match.groupValues[1]
                if (banned.containsMatchIn(text)) offences += "${file.name}: \"$text\""
            }
        }
        assertEquals(emptyList(), offences)
    }

    @Test
    fun theResourceStringsAreFreeOfPlumbingVocabularyToo() {
        val strings = File("src/main/res/values/strings.xml").readText()
        assertEquals(emptyList(), banned.findAll(strings.replace(Regex("<[^>]*>"), " ")).map { it.value }.toList())
    }
}
