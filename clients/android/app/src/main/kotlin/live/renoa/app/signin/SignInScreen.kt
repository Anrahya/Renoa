package live.renoa.app.signin

import androidx.compose.animation.core.Animatable
import androidx.compose.foundation.border
import androidx.compose.foundation.layout.widthIn
import androidx.compose.foundation.text.BasicTextField
import androidx.compose.foundation.text.KeyboardActions
import androidx.compose.foundation.text.KeyboardOptions
import androidx.compose.ui.graphics.SolidColor
import androidx.compose.ui.text.input.ImeAction
import androidx.compose.ui.text.input.KeyboardType
import live.renoa.app.theme.RenoaShapes
import androidx.compose.animation.core.tween
import androidx.compose.foundation.Canvas
import androidx.compose.foundation.background
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.height
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.safeDrawingPadding
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.material3.Text
import androidx.compose.runtime.Composable
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.remember
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.geometry.Offset
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.graphics.drawscope.Stroke
import androidx.compose.ui.graphics.drawscope.withTransform
import androidx.compose.ui.semantics.contentDescription
import androidx.compose.ui.semantics.semantics
import androidx.compose.ui.text.style.TextAlign
import androidx.compose.ui.unit.dp
import androidx.compose.ui.unit.sp
import kotlinx.coroutines.delay
import kotlinx.coroutines.launch
import live.renoa.app.badge.HexSpiral
import live.renoa.app.badge.hexPath
import live.renoa.app.theme.LocalReducedMotion
import live.renoa.app.theme.Renoa
import live.renoa.app.theme.RenoaEasing
import live.renoa.app.theme.RenoaType
import live.renoa.app.ui.Glyphs
import live.renoa.app.ui.RenoaButton
import kotlin.math.abs

private val TILE_TONES = listOf(Color(0xFFFF8A4C), Color(0xFFE046B4), Color(0xFF2FD0E6), Color(0xFFC9485B), Color(0xFFF2B8C6), Color(0xFF4FB3FF))

/**
 * The first screen. The person enters the code from their Renoa, and the passkey prompt does the
 * rest; [state] says whether that is under way or why it stopped.
 */
@Composable
internal fun SignInScreen(state: SignInState, code: String, onCodeChange: (String) -> Unit, onSubmit: () -> Unit) {
    val colors = Renoa.colors
    val reduced = LocalReducedMotion.current
    // Seven tiles drop into place one after the other: the core first, then the six around it.
    val drops = remember { List(7) { Animatable(if (reduced) 1f else 0f) } }
    LaunchedEffect(Unit) {
        if (reduced) return@LaunchedEffect
        drops.forEachIndexed { i, drop ->
            launch {
                delay(100L + i * 90L)
                drop.animateTo(1f, tween(800, easing = RenoaEasing.Ease))
            }
        }
    }

    Column(Modifier.fillMaxSize().background(colors.background).safeDrawingPadding()) {
        Box(Modifier.weight(1f).fillMaxWidth().semantics { contentDescription = "The Renoa mark: an ink core with six tiles around it" }, contentAlignment = Alignment.Center) {
            Canvas(Modifier.fillMaxSize()) {
                val s = size.minDimension.coerceAtMost(420.dp.toPx()) / 420f
                withTransform({ translate(size.width / 2, size.height / 2); scale(s, s, Offset.Zero) }) {
                    // The board: faint empty sockets the tiles could fill.
                    for (index in 7 until 60) {
                        val c = HexSpiral.centre(HexSpiral.seat(index), 46f)
                        if (abs(c.x) < 220 && abs(c.y) < 230) {
                            drawPath(hexPath(c.x, c.y, 42f, 3.5f), colors.border.copy(alpha = 0.5f), style = Stroke(1f))
                        }
                    }
                    for (index in 0 until 7) {
                        val c = HexSpiral.centre(HexSpiral.seat(index), 46f)
                        val progress = drops[index].value
                        val tone = if (index == 0) colors.foreground else TILE_TONES[index - 1]
                        withTransform({ translate(0f, (1f - progress) * -30f) }) {
                            drawPath(hexPath(c.x, c.y + 5, 42f, 3.5f), (if (index == 0) colors.foregroundSoft else Color.Black.copy(alpha = 0.35f)).copy(alpha = progress))
                            drawPath(hexPath(c.x, c.y, 42f, 3.5f), tone.copy(alpha = progress))
                            drawPath(hexPath(c.x, c.y, 42f, 3.5f), Color.White.copy(alpha = 0.18f * progress), style = Stroke(1.2f))
                        }
                    }
                }
            }
        }
        val working = state == SignInState.Working
        val ready = !working && SignInCode.parse(code) != null
        val shape = RoundedCornerShape(RenoaShapes.Control)
        // One centred column, no wider than a phone, so a tablet keeps one left edge for every line.
        Box(Modifier.fillMaxWidth().padding(horizontal = 24.dp), contentAlignment = Alignment.TopCenter) {
            Column(Modifier.widthIn(max = 440.dp).fillMaxWidth(), horizontalAlignment = Alignment.CenterHorizontally) {
                Text("Renoa", style = RenoaType.Display.copy(fontSize = 44.sp), color = colors.foreground, textAlign = TextAlign.Center)
                Text("Your agents, in your pocket.", Modifier.padding(top = 8.dp), style = RenoaType.Body.copy(fontSize = 17.sp), color = colors.mutedForeground, textAlign = TextAlign.Center)
                Box(
                    Modifier
                        .padding(top = 32.dp)
                        .fillMaxWidth()
                        .background(colors.card, shape)
                        .border(1.dp, colors.input, shape)
                        .padding(horizontal = 16.dp, vertical = 14.dp),
                ) {
                    if (code.isEmpty()) Text("Sign-in code", style = RenoaType.Body, color = colors.mutedForeground)
                    BasicTextField(
                        value = code,
                        onValueChange = onCodeChange,
                        enabled = !working,
                        singleLine = true,
                        textStyle = RenoaType.Body.copy(color = colors.foreground),
                        cursorBrush = SolidColor(colors.ring),
                        keyboardOptions = KeyboardOptions(autoCorrectEnabled = false, keyboardType = KeyboardType.Ascii, imeAction = ImeAction.Go),
                        keyboardActions = KeyboardActions(onGo = { if (ready) onSubmit() }),
                        modifier = Modifier.fillMaxWidth().semantics { contentDescription = "Sign-in code" },
                    )
                }
                val failure = (state as? SignInState.Failed)?.message
                Text(
                    failure ?: "Enter the code from your Renoa. Your passkey does the rest.",
                    Modifier.padding(top = 10.dp).fillMaxWidth(),
                    style = RenoaType.Caption,
                    color = if (failure != null) colors.stateFailed else colors.mutedForeground,
                    textAlign = TextAlign.Center,
                )
                RenoaButton(
                    if (working) "Waiting for your passkey…" else "Continue with passkey",
                    onSubmit,
                    Modifier.padding(top = 20.dp, bottom = 28.dp).fillMaxWidth(),
                    enabled = ready,
                    icon = Glyphs.PASSKEY,
                )
            }
        }
    }
}
