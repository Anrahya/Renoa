package live.renoa.app.chat

import android.webkit.URLUtil
import androidx.compose.ui.platform.UriHandler
import androidx.compose.ui.text.AnnotatedString
import androidx.compose.ui.text.SpanStyle
import androidx.compose.ui.text.TextLinkStyles
import androidx.core.net.toUri
import com.mikepenz.markdown.annotator.DefaultAnnotatorSettings
import com.mikepenz.markdown.annotator.buildMarkdownAnnotatedString
import com.mikepenz.markdown.model.MarkdownAnnotator
import com.mikepenz.markdown.model.ReferenceLinkHandler
import com.mikepenz.markdown.model.markdownAnnotator
import com.mikepenz.markdown.utils.getUnescapedTextInNode
import com.mikepenz.markdown.utils.resolveImageAlt
import org.intellij.markdown.MarkdownElementTypes
import org.intellij.markdown.MarkdownTokenTypes
import org.intellij.markdown.ast.ASTNode
import org.intellij.markdown.ast.findChildOfType
import org.intellij.markdown.flavours.gfm.GFMTokenTypes

/** A web address: http or https in any case, or a bare `www.` one, which opens over https. */
internal fun isWebLink(uri: String): Boolean = URLUtil.isNetworkUrl(uri) || uri.startsWith("www.", ignoreCase = true)

/**
 * Opens a reply's links in the phone's browser through [platform]. Only web addresses leave the
 * app; anything else does nothing, and so does a phone with no browser.
 */
internal class WebLinks(private val platform: UriHandler) : UriHandler {
    override fun openUri(uri: String) {
        if (!isWebLink(uri)) return
        val web = if (URLUtil.isNetworkUrl(uri)) uri else "https://$uri"
        // An intent's scheme matches case-sensitively, so HTTP:// would find no browser.
        runCatching { platform.openUri(web.toUri().normalizeScheme().toString()) }
    }
}

/**
 * How a reply's links and images become text. A link that is not a web address is drawn as its
 * words alone, so nothing looks tappable that is not; an image is drawn as its description, as a
 * reply loads nothing from the network. [links] resolves `[text][label]` references.
 */
internal class ReplyAnnotator(codeSpan: SpanStyle, highlight: SpanStyle, private val links: ReferenceLinkHandler) {
    val annotator: MarkdownAnnotator = markdownAnnotator { content, child -> annotate(content, child) }

    /** For a link's words drawn without the link: formatting kept, nothing tappable. */
    private val plain = DefaultAnnotatorSettings(TextLinkStyles(), codeSpan, annotator, highlightSpanStyle = highlight)

    private fun AnnotatedString.Builder.annotate(content: String, child: ASTNode): Boolean {
        val words: List<ASTNode>
        val destination: String?
        when (child.type) {
            MarkdownElementTypes.IMAGE -> {
                child.resolveImageAlt(content)?.let { append(it) }
                return true
            }
            MarkdownElementTypes.INLINE_LINK -> {
                words = child.findChildOfType(MarkdownElementTypes.LINK_TEXT)?.bracketed() ?: return false
                destination = child.findChildOfType(MarkdownElementTypes.LINK_DESTINATION)?.getUnescapedTextInNode(content)
            }
            MarkdownElementTypes.FULL_REFERENCE_LINK, MarkdownElementTypes.SHORT_REFERENCE_LINK -> {
                val label = child.findChildOfType(MarkdownElementTypes.LINK_LABEL) ?: return false
                val text = if (child.type == MarkdownElementTypes.FULL_REFERENCE_LINK) child.findChildOfType(MarkdownElementTypes.LINK_TEXT) else label
                words = text?.bracketed() ?: return false
                destination = links.find(label.getUnescapedTextInNode(content)).ifEmpty { null }
            }
            MarkdownElementTypes.AUTOLINK -> {
                val address = child.findChildOfType(MarkdownTokenTypes.AUTOLINK)?.getUnescapedTextInNode(content) ?: return false
                if (isWebLink(address)) return false
                append(address)
                return true
            }
            GFMTokenTypes.GFM_AUTOLINK, MarkdownTokenTypes.EMAIL_AUTOLINK -> {
                if (child.parent?.type == MarkdownElementTypes.LINK_TEXT) return false
                val address = child.getUnescapedTextInNode(content)
                if (child.type == GFMTokenTypes.GFM_AUTOLINK && isWebLink(address)) return false
                append(address)
                return true
            }
            else -> return false
        }
        // No destination: the library draws the words alone already.
        if (destination == null || isWebLink(destination)) return false
        buildMarkdownAnnotatedString(content, words, plain)
        return true
    }

    /** A `[ ... ]` node's children without its brackets. */
    private fun ASTNode.bracketed(): List<ASTNode>? = children.takeIf { it.size >= 2 }?.subList(1, children.size - 1)
}
