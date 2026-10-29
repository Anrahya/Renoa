package live.renoa.app.chat

import androidx.compose.foundation.layout.PaddingValues
import androidx.compose.runtime.Composable
import androidx.compose.runtime.CompositionLocalProvider
import androidx.compose.material3.Text
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.collectAsState
import androidx.compose.runtime.getValue
import androidx.compose.runtime.key
import androidx.compose.runtime.mutableIntStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.setValue
import androidx.compose.ui.Modifier
import androidx.compose.ui.platform.LocalUriHandler
import androidx.compose.ui.text.SpanStyle
import androidx.compose.ui.text.TextLinkStyles
import androidx.compose.ui.text.font.FontStyle
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.text.style.TextDecoration
import androidx.compose.ui.unit.dp
import androidx.compose.ui.unit.sp
import com.mikepenz.markdown.m3.Markdown
import com.mikepenz.markdown.m3.markdownColor
import com.mikepenz.markdown.m3.markdownTypography
import com.mikepenz.markdown.model.MarkdownAnnotator
import com.mikepenz.markdown.model.MarkdownColors
import com.mikepenz.markdown.model.MarkdownDimens
import com.mikepenz.markdown.model.MarkdownPadding
import com.mikepenz.markdown.model.MarkdownTypography
import com.mikepenz.markdown.model.ReferenceLinkHandler
import com.mikepenz.markdown.model.State
import com.mikepenz.markdown.model.markdownDimens
import com.mikepenz.markdown.model.markdownPadding
import com.mikepenz.markdown.model.parseMarkdown
import com.mikepenz.markdown.model.rememberStreamingMarkdownState
import live.renoa.app.theme.Renoa
import live.renoa.app.theme.RenoaShapes
import live.renoa.app.theme.RenoaType

/**
 * A finished reply, [text] as [parsed], rendered as markdown in Renoa's type and colours. A reply
 * the parser could not read shows as its plain text.
 */
@Composable
fun Reply(text: String, parsed: State, modifier: Modifier = Modifier) {
    if (parsed !is State.Success) {
        Text(text, modifier, style = RenoaType.Body, color = Renoa.colors.foregroundSoft)
        return
    }
    ReplyStyle(parsed.referenceLinkHandler) { look ->
        Markdown(state = parsed, colors = look.colors, typography = look.typography, padding = look.padding, dimens = look.dimens, annotator = look.annotator, modifier = modifier)
    }
}

/**
 * A reply still arriving. Each new piece of [text] is parsed on its own, against what came before,
 * so the reply is not parsed whole for every word. Text that does not carry on from what was parsed,
 * as when the agent starts its reply again, starts a new parse; until that parse has something to
 * show, the whole text is parsed at once instead, so the reply never blanks.
 */
@Composable
fun ArrivingReply(text: String, modifier: Modifier = Modifier) {
    // Text that does not carry on from what was parsed starts a new parse.
    var restarts by remember { mutableIntStateOf(0) }
    key(restarts) {
        val stream = rememberStreamingMarkdownState()
        LaunchedEffect(stream, text) {
            val parsed = stream.content
            if (!text.startsWith(parsed)) restarts++ else if (text.length > parsed.length) stream.append(text.substring(parsed.length))
        }
        val snapshot by stream.snapshot.collectAsState()
        if (snapshot.stableAst.isEmpty() && snapshot.unstableAstTail.isEmpty()) {
            Reply(text, remember(text) { parseMarkdown(text) }, modifier)
        } else {
            ReplyStyle(stream.referenceLinkHandler) { look ->
                Markdown(
                    streamingMarkdownState = stream, colors = look.colors, typography = look.typography, padding = look.padding, dimens = look.dimens, annotator = look.annotator, modifier = modifier,
                )
            }
        }
    }
}

/**
 * Finished replies' parses by message id, so a reply scrolled back into view is not parsed again.
 * It holds the [capacity] most recently shown; a message whose text changed is parsed afresh, and
 * one the parser could not read is never kept. Main thread only: it is not synchronised.
 */
internal class ParsedReplies(private val capacity: Int = 200) {
    private val parses =
        object : LinkedHashMap<String, Pair<String, State>>(16, 0.75f, true) {
            override fun removeEldestEntry(eldest: MutableMap.MutableEntry<String, Pair<String, State>>) = size > capacity
        }

    fun of(id: String, text: String): State =
        parses[id]?.takeIf { it.first == text }?.second ?: parseMarkdown(text).also { if (it is State.Success) parses[id] = text to it }
}

/** Everything the markdown renderer takes to draw a reply Renoa's way. */
private class Look(val colors: MarkdownColors, val typography: MarkdownTypography, val padding: MarkdownPadding, val dimens: MarkdownDimens, val annotator: MarkdownAnnotator)

/** Renoa's colours, type, spacing and link handling for a reply whose references [links] resolves. */
@Composable
private fun ReplyStyle(links: ReferenceLinkHandler, content: @Composable (Look) -> Unit) {
    val colors = Renoa.colors
    val body = RenoaType.Body.copy(color = colors.foregroundSoft)
    val heading = RenoaType.Heading.copy(color = colors.foreground)
    val code = RenoaType.Code.copy(color = colors.foreground)
    val inlineCode = code.copy(fontSize = 14.sp, lineHeight = body.lineHeight)
    val markdownColors =
        markdownColor(text = colors.foregroundSoft, codeBackground = colors.muted, inlineCodeBackground = colors.muted, dividerColor = colors.border, tableBackground = colors.card)
    val typography =
        markdownTypography(
            h1 = heading.copy(fontSize = 20.sp, lineHeight = 26.sp),
            h2 = heading,
            h3 = heading.copy(fontSize = 16.sp, lineHeight = 22.sp),
            h4 = body.copy(color = colors.foreground, fontWeight = FontWeight.SemiBold),
            h5 = body.copy(color = colors.foreground, fontWeight = FontWeight.SemiBold),
            h6 = body.copy(color = colors.foreground, fontWeight = FontWeight.SemiBold),
            text = body,
            code = code,
            inlineCode = inlineCode,
            quote = body.copy(color = colors.mutedForeground, fontStyle = FontStyle.Italic),
            paragraph = body,
            ordered = body.copy(color = colors.mutedForeground),
            // Instrument Sans draws its bullet small, so it keeps the text's colour rather than fading further.
            bullet = body,
            list = body,
            textLink = TextLinkStyles(SpanStyle(color = colors.foreground, fontWeight = FontWeight.Medium, textDecoration = TextDecoration.Underline)),
            table = body,
            alertTitle = body.copy(color = colors.foreground, fontWeight = FontWeight.SemiBold),
        )
    val annotator =
        remember(links, inlineCode, markdownColors) {
            ReplyAnnotator(inlineCode.copy(background = colors.muted).toSpanStyle(), SpanStyle(color = markdownColors.highlightText, background = markdownColors.highlightBackground), links).annotator
        }
    val platform = LocalUriHandler.current
    val web = remember(platform) { WebLinks(platform) }
    val padding = markdownPadding(block = 3.dp, list = 2.dp, listItemTop = 2.dp, listItemBottom = 2.dp, listIndent = 12.dp, codeBlock = PaddingValues(horizontal = 12.dp, vertical = 10.dp))
    val dimens = markdownDimens(codeBackgroundCornerSize = RenoaShapes.Control)
    CompositionLocalProvider(LocalUriHandler provides web) { content(Look(markdownColors, typography, padding, dimens, annotator)) }
}
