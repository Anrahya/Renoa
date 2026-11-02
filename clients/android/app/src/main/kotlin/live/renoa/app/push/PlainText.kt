package live.renoa.app.push

/** The most of a reply a notice carries; the chat has the rest. */
internal const val NOTICE_CHARACTERS = 1_000

/** What a notice says for a reply that has no words once its marks are gone, such as a lone rule. */
internal const val WORDLESS_REPLY = "New reply"

/**
 * How much of a reply is read for a notice. Marks take room the words do not, so it is several times
 * what a notice shows, and it bounds the work the patterns below can do on a line made to be slow.
 */
private const val READ_CHARACTERS = 4 * NOTICE_CHARACTERS

private val FENCE = Regex("^\\s*(```|~~~)")
private val HEADING = Regex("^\\s{0,3}#{1,6}\\s+")
private val QUOTE = Regex("^\\s{0,3}(>\\s?)+")
private val BULLET = Regex("^(\\s*)[-*+]\\s+")
private val RULE = Regex("^\\s{0,3}([-*_]\\s*){3,}$")
private val IMAGE = Regex("!\\[([^\\]]*)]\\([^)]*\\)")
private val LINK = Regex("\\[([^\\]]+)]\\([^)]*\\)")
private val CODE = Regex("`([^`]+)`")
private val STRONG = Regex("(\\*\\*|__)(?=\\S)(.+?)(?<=\\S)\\1")
private val EMPHASIS = Regex("(?<![\\w*])([*_])(?=\\S)(.+?)(?<=\\S)\\1(?![\\w*])")
private val STRUCK = Regex("~~(?=\\S)(.+?)(?<=\\S)~~")

/**
 * A markdown reply as a notice shows it: plain words, without the marks the chat would draw. Fences,
 * rules, heading and quote marks go; a bullet becomes "•"; links and images keep their words; code
 * keeps its text. Blank lines fold into one, and a reply longer than [limit] characters is cut to fit
 * [limit] with "…" at the end. Only the first [READ_CHARACTERS] are read. A reply with no words left
 * says [WORDLESS_REPLY].
 */
internal fun plainText(markdown: String, limit: Int = NOTICE_CHARACTERS): String {
    val lines =
        markdown.take(if (markdown.length > READ_CHARACTERS && markdown[READ_CHARACTERS - 1].isHighSurrogate()) READ_CHARACTERS - 1 else READ_CHARACTERS).lines().filterNot { FENCE.containsMatchIn(it) || RULE.matches(it) }.map { line ->
            val bare = BULLET.replace(QUOTE.replace(HEADING.replace(line, ""), ""), "$1• ")
            var text = IMAGE.replace(bare, "$1")
            text = LINK.replace(text, "$1")
            text = CODE.replace(text, "$1")
            text = STRONG.replace(text, "$2")
            text = STRUCK.replace(text, "$1")
            EMPHASIS.replace(text, "$2").trimEnd()
        }
    val folded = lines.joinToString("\n").replace(Regex("\n{3,}"), "\n\n").trim().ifEmpty { WORDLESS_REPLY }
    if (folded.length <= limit) return folded
    // Room for the ellipsis, and never inside a surrogate pair, so the cut leaves no half a character.
    val room = limit - 1
    val cut = if (folded[room - 1].isHighSurrogate()) room - 1 else room
    return folded.substring(0, cut).trimEnd() + "…"
}
