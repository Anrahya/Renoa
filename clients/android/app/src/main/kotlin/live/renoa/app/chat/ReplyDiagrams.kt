package live.renoa.app.chat

import androidx.compose.runtime.Composable
import androidx.compose.runtime.movableContentOf
import com.mikepenz.markdown.compose.components.CurrentComponentsBridge
import com.mikepenz.markdown.compose.components.MarkdownComponents
import com.mikepenz.markdown.compose.components.markdownComponents
import org.intellij.markdown.MarkdownTokenTypes
import org.intellij.markdown.ast.ASTNode
import org.intellij.markdown.ast.findChildOfType
import org.intellij.markdown.ast.getTextInNode

/** A fence whose language is `mermaid`: the diagram's [source], and whether its closing fence has arrived. */
internal class MermaidFence(val source: String, val closed: Boolean)

/** The Mermaid diagram in the code fence [node] of [content], or null when the fence holds anything else. */
internal fun mermaidFence(content: String, node: ASTNode): MermaidFence? {
    val language = node.findChildOfType(MarkdownTokenTypes.FENCE_LANG)?.getTextInNode(content)?.trim()?.split(Regex("\\s+"))?.first()
    if (!language.equals("mermaid", ignoreCase = true)) return null
    val parts = node.children
    val end = parts.indexOfFirst { it.type == MarkdownTokenTypes.CODE_FENCE_END }
    // The source is everything after the opening line, up to the closing fence or, without one, the end.
    val body = parts.subList(0, if (end < 0) parts.size else end).dropWhile { it.type != MarkdownTokenTypes.EOL }.drop(1)
    val source = if (body.isEmpty()) "" else content.substring(body.first().startOffset, body.last().endOffset).replaceIndent()
    return MermaidFence(source, closed = end >= 0)
}

/**
 * A reply's diagrams, one for each fence, told apart by where the fence starts. A diagram is movable
 * content, so it keeps its view, already drawn, as the reply around it changes shape: from the
 * part of an arriving reply still being parsed to the settled part, and from the arriving reply to
 * the finished message. Main thread only.
 */
class ReplyDiagrams {
    private val diagrams = HashMap<Int, @Composable (String, @Composable () -> Unit) -> Unit>()

    @Composable
    internal fun Show(at: Int, source: String, fallback: @Composable () -> Unit) =
        diagrams.getOrPut(at) { movableContentOf { source: String, fallback: @Composable () -> Unit -> Diagram(source, fallback) } }(source, fallback)
}

/**
 * The markdown components for a reply whose diagrams [diagrams] keeps. While the reply is
 * [arriving], a diagram whose fence is still open is a placeholder rather than half a diagram;
 * any other code fence draws as the library draws it.
 */
internal fun replyComponents(diagrams: ReplyDiagrams, arriving: Boolean): MarkdownComponents =
    markdownComponents(
        codeFence = { model ->
            val fence = mermaidFence(model.content, model.node)
            when {
                fence == null -> CurrentComponentsBridge.codeFence(model)
                arriving && !fence.closed -> DiagramPlaceholder()
                else -> diagrams.Show(model.node.startOffset, fence.source) { CurrentComponentsBridge.codeFence(model) }
            }
        },
    )
