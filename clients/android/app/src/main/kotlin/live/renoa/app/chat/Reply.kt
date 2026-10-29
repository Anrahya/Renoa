package live.renoa.app.chat

import android.webkit.URLUtil
import androidx.compose.foundation.layout.PaddingValues
import androidx.compose.runtime.Composable
import androidx.compose.runtime.CompositionLocalProvider
import androidx.compose.runtime.remember
import androidx.compose.ui.Modifier
import androidx.compose.ui.platform.LocalUriHandler
import androidx.compose.ui.platform.UriHandler
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
import com.mikepenz.markdown.model.markdownDimens
import com.mikepenz.markdown.model.markdownPadding
import com.mikepenz.markdown.model.parseMarkdown
import live.renoa.app.theme.Renoa
import live.renoa.app.theme.RenoaShapes
import live.renoa.app.theme.RenoaType

/**
 * An agent's reply, rendered as markdown in Renoa's type and colours. It is parsed as it is
 * composed rather than in the background, so a reply still arriving never blanks between words:
 * each new word lays out against the reply so far, unfinished markdown and all.
 */
@Composable
fun Reply(text: String, modifier: Modifier = Modifier) {
    val colors = Renoa.colors
    val parsed = remember(text) { parseMarkdown(text) }
    val body = RenoaType.Body.copy(color = colors.foregroundSoft)
    val heading = RenoaType.Heading.copy(color = colors.foreground)
    val code = RenoaType.Code.copy(color = colors.foreground)
    val web = webOnly(LocalUriHandler.current)
    CompositionLocalProvider(LocalUriHandler provides web) {
        Markdown(
            state = parsed,
            colors =
                markdownColor(
                    text = colors.foregroundSoft,
                    codeBackground = colors.muted,
                    inlineCodeBackground = colors.muted,
                    dividerColor = colors.border,
                    tableBackground = colors.card,
                ),
            typography =
                markdownTypography(
                    h1 = heading.copy(fontSize = 20.sp, lineHeight = 26.sp),
                    h2 = heading,
                    h3 = heading.copy(fontSize = 16.sp, lineHeight = 22.sp),
                    h4 = body.copy(color = colors.foreground, fontWeight = FontWeight.SemiBold),
                    h5 = body.copy(color = colors.foreground, fontWeight = FontWeight.SemiBold),
                    h6 = body.copy(color = colors.foreground, fontWeight = FontWeight.SemiBold),
                    text = body,
                    code = code,
                    inlineCode = code.copy(fontSize = 14.sp, lineHeight = body.lineHeight),
                    quote = body.copy(color = colors.mutedForeground, fontStyle = FontStyle.Italic),
                    paragraph = body,
                    ordered = body.copy(color = colors.mutedForeground),
                    // Instrument Sans draws its bullet small, so it keeps the text's colour rather than fading further.
                    bullet = body,
                    list = body,
                    textLink = TextLinkStyles(SpanStyle(color = colors.foreground, fontWeight = FontWeight.Medium, textDecoration = TextDecoration.Underline)),
                    table = body,
                    alertTitle = body.copy(color = colors.foreground, fontWeight = FontWeight.SemiBold),
                ),
            padding = markdownPadding(block = 3.dp, list = 2.dp, listItemTop = 2.dp, listItemBottom = 2.dp, listIndent = 12.dp, codeBlock = PaddingValues(horizontal = 12.dp, vertical = 10.dp)),
            dimens = markdownDimens(codeBackgroundCornerSize = RenoaShapes.Control),
            modifier = modifier,
        )
    }
}

/** Only web links leave the app, to the phone's browser through [platform]; any other scheme in a reply does nothing. */
@Composable
private fun webOnly(platform: UriHandler): UriHandler =
    remember(platform) {
        object : UriHandler {
            override fun openUri(uri: String) {
                if (URLUtil.isNetworkUrl(uri)) platform.openUri(uri)
            }
        }
    }
