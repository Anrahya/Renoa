// Portions copyright 2026 Po Kai Lee and the Codync contributors.
// Licensed under the Apache License, Version 2.0; see licenses/Apache-2.0-Codync.txt.
// Adapted from Codync apps/android `ChatRows.kt` at ed8daf11f89398e598ef1f791abc3014ded68496.
package live.renoa.app.chat

import android.content.ClipData
import android.content.ClipboardManager
import androidx.compose.animation.core.Animatable
import androidx.compose.foundation.background
import androidx.compose.foundation.clickable
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.heightIn
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.widthIn
import androidx.compose.foundation.text.selection.SelectionContainer
import androidx.compose.material3.Text
import androidx.compose.runtime.Composable
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.remember
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.draw.clip
import androidx.compose.ui.graphics.graphicsLayer
import androidx.compose.ui.platform.LocalContext
import androidx.compose.ui.platform.LocalFocusManager
import androidx.compose.ui.semantics.Role
import androidx.compose.ui.text.style.TextAlign
import androidx.compose.ui.text.style.TextOverflow
import androidx.compose.ui.unit.dp
import com.kyant.shapes.RoundedRectangle
import live.renoa.app.agents.ChatMessage
import live.renoa.app.agents.Speaker
import live.renoa.app.theme.LocalMotionClock
import live.renoa.app.theme.LocalReducedMotion
import live.renoa.app.theme.RenoaShapes
import live.renoa.design.MenuItem
import live.renoa.design.Motion
import live.renoa.design.Palette
import live.renoa.design.ThinkingOrb
import live.renoa.design.ThinkingOrbState
import live.renoa.design.Type
import live.renoa.design.bold
import live.renoa.design.contextActions
import live.renoa.design.monospacedDigit

/** Live activity bubble at the bottom of the chat while the agent works on its reply. */
@Composable
internal fun WorkingIndicator(doing: String?) {
    val clock = LocalMotionClock.current
    val startedAt = remember { clock.value }
    val elapsed = (clock.value - startedAt).toInt()
    Box(Modifier.fillMaxWidth().padding(start = 16.dp, end = 40.dp)) {
        Row(
            Modifier
                .clip(RoundedRectangle(RenoaShapes.Bubble))
                .background(Palette.bubbleAgent)
                .padding(horizontal = 16.dp, vertical = 12.dp),
            verticalAlignment = Alignment.CenterVertically,
            horizontalArrangement = Arrangement.spacedBy(8.dp),
        ) {
            ThinkingOrb(state = ThinkingOrbState.Working, size = 16.dp, color = Palette.secondary)
            Text(
                workingWords(doing),
                Modifier.weight(1f, fill = false),
                style = Type.subheadline,
                color = Palette.secondary,
                maxLines = 1,
                overflow = TextOverflow.Ellipsis,
            )
            if (elapsed >= 1) {
                val time =
                    if (elapsed >= 3600) "%d:%02d:%02d".format(elapsed / 3600, elapsed / 60 % 60, elapsed % 60)
                    else "%d:%02d".format(elapsed / 60, elapsed % 60)
                Text(time, style = Type.footnote.monospacedDigit, color = Palette.tertiary)
            }
        }
    }
}

private fun workingWords(doing: String?): String {
    val clean = doing?.trim()?.trimEnd('.', '…')?.takeIf { it.isNotEmpty() } ?: return "Working…"
    return "$clean…"
}

@Composable
internal fun Rise(animate: Boolean, content: @Composable () -> Unit) {
    val reduced = LocalReducedMotion.current
    val progress = remember { Animatable(if (animate && !reduced) 0f else 1f) }
    LaunchedEffect(Unit) { if (progress.value < 1f) progress.animateTo(1f, Motion.conversation()) }
    Box(Modifier.graphicsLayer { alpha = progress.value; translationY = (1f - progress.value) * 14.dp.toPx() }) { content() }
}

@Composable
internal fun MessageRow(
    message: ChatMessage,
    streaming: Boolean,
    parses: ParsedReplies,
    onRetry: (String) -> Unit,
    onDiscard: (String) -> Unit,
) {
    val app = LocalContext.current
    val focus = LocalFocusManager.current
    when (message.speaker) {
        Speaker.Owner ->
            Column(
                Modifier.fillMaxWidth().padding(start = 56.dp, end = 16.dp),
                horizontalAlignment = Alignment.End,
                verticalArrangement = Arrangement.spacedBy(4.dp),
            ) {
                if (message.notSent) {
                    // A message that did not go is the retry button, with its line under it.
                    Column(
                        Modifier.clickable(role = Role.Button, onClickLabel = "Retry") { onRetry(message.id) },
                        horizontalAlignment = Alignment.End,
                        verticalArrangement = Arrangement.spacedBy(4.dp),
                    ) {
                        if (message.attachments.isNotEmpty()) SentFiles(message.attachments)
                        if (message.text.isNotBlank()) OwnerBubbleText(message.text)
                        Text("Didn't send · Tap to retry", style = Type.caption2.bold, color = Palette.danger)
                    }
                    Box(
                        Modifier.heightIn(min = 48.dp).clickable(role = Role.Button) { onDiscard(message.id) }.padding(horizontal = 4.dp),
                        contentAlignment = Alignment.Center,
                    ) { Text("Delete", style = Type.footnote, color = Palette.secondary) }
                } else {
                    if (message.attachments.isNotEmpty()) SentFiles(message.attachments)
                    if (message.text.isNotBlank()) Box(
                        Modifier
                            .clip(RoundedRectangle(RenoaShapes.Bubble))
                            .contextActions(
                                mergeSemantics = false,
                                onClick = { focus.clearFocus() },
                                preview = { SelectionContainer { OwnerBubbleText(message.text) } },
                            ) {
                                listOf(
                                    MenuItem("Copy", icon = "square.on.square") {
                                        copyMessage(app.getSystemService(ClipboardManager::class.java), message.text)
                                    },
                                )
                            },
                    ) { OwnerBubbleText(message.text) }
                }
            }
        Speaker.Event ->
            Text(
                message.text,
                Modifier.fillMaxWidth().padding(horizontal = 24.dp, vertical = 2.dp),
                style = Type.footnote,
                color = Palette.tertiary,
                textAlign = TextAlign.Center,
                maxLines = 1,
                overflow = TextOverflow.Ellipsis,
            )
        // The owner's own words stay plain; an agent's reply is markdown, in its bubble.
        Speaker.Agent -> {
            // One for both, so a diagram drawn while the reply arrives stays drawn once the reply is whole.
            val diagrams = remember { ReplyDiagrams() }
            val bubblePadding = Modifier.background(Palette.bubbleAgent).padding(horizontal = 16.dp, vertical = 10.dp)
            if (streaming) {
                ArrivingReply(
                    message.text,
                    Modifier.padding(start = 16.dp, end = 40.dp).clip(RoundedRectangle(RenoaShapes.Bubble)).then(bubblePadding),
                    diagrams,
                )
            } else {
                val parsed = parses.of(message.id, message.text)
                Box(
                    Modifier
                        .padding(start = 16.dp, end = 40.dp)
                        .clip(RoundedRectangle(RenoaShapes.Bubble))
                        .contextActions(
                            mergeSemantics = false,
                            onClick = { focus.clearFocus() },
                            preview = { SelectionContainer { Reply(message.text, parsed, bubblePadding, diagrams) } },
                        ) {
                            listOf(
                                MenuItem("Copy", icon = "square.on.square") {
                                    copyMessage(app.getSystemService(ClipboardManager::class.java), message.text)
                                },
                            )
                        },
                ) {
                    Reply(message.text, parsed, bubblePadding, diagrams)
                }
            }
        }
    }
}

@Composable
private fun OwnerBubbleText(text: String) {
    Text(
        text,
        Modifier
            .widthIn(max = 300.dp)
            .clip(RoundedRectangle(RenoaShapes.Bubble))
            .background(Palette.bubbleUser)
            .padding(horizontal = 16.dp, vertical = 10.dp),
        style = Type.body,
        color = Palette.text,
    )
}

private fun copyMessage(clipboard: ClipboardManager?, text: String) {
    clipboard?.setPrimaryClip(ClipData.newPlainText("Message", text))
}
