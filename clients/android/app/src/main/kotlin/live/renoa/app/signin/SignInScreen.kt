package live.renoa.app.signin

import androidx.compose.foundation.background
import androidx.compose.foundation.border
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.safeDrawingPadding
import androidx.compose.foundation.layout.widthIn
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.foundation.text.BasicTextField
import androidx.compose.foundation.text.KeyboardActions
import androidx.compose.foundation.text.KeyboardOptions
import androidx.compose.material3.Text
import androidx.compose.runtime.Composable
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.graphics.SolidColor
import androidx.compose.ui.semantics.contentDescription
import androidx.compose.ui.semantics.semantics
import androidx.compose.ui.text.input.ImeAction
import androidx.compose.ui.text.input.KeyboardType
import androidx.compose.ui.text.style.TextAlign
import androidx.compose.ui.unit.dp
import androidx.compose.ui.unit.sp
import live.renoa.app.theme.Renoa
import live.renoa.app.theme.RenoaShapes
import live.renoa.app.theme.RenoaType
import live.renoa.app.ui.Glyphs
import live.renoa.app.ui.RenoaButton

/**
 * The first screen. The person enters the code from their Renoa, and the passkey prompt does the
 * rest; [state] says whether that is under way or why it stopped.
 */
@Composable
internal fun SignInScreen(state: SignInState, code: String, onCodeChange: (String) -> Unit, onSubmit: () -> Unit) {
    val colors = Renoa.colors
    val working = state == SignInState.Working

    Column(Modifier.fillMaxSize().background(colors.background).safeDrawingPadding()) {
        SignInBoard(linking = working, modifier = Modifier.weight(1f).fillMaxWidth())
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
                    // The button keeps 4dp above and below for its 48dp tap area; these make 20 and 28.
                    Modifier.padding(top = 16.dp, bottom = 24.dp).fillMaxWidth(),
                    enabled = ready,
                    icon = Glyphs.PASSKEY,
                )
            }
        }
    }
}
