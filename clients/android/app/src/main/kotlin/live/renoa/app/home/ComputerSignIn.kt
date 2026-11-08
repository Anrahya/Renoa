package live.renoa.app.home

import androidx.compose.animation.AnimatedVisibility
import androidx.compose.animation.fadeIn
import androidx.compose.animation.fadeOut
import androidx.compose.foundation.background
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.text.KeyboardActions
import androidx.compose.foundation.text.KeyboardOptions
import androidx.compose.material3.Text
import androidx.compose.runtime.Composable
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.rememberCoroutineScope
import androidx.compose.runtime.saveable.rememberSaveable
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.draw.clip
import androidx.compose.ui.semantics.contentDescription
import androidx.compose.ui.semantics.semantics
import androidx.compose.ui.text.input.ImeAction
import androidx.compose.ui.text.input.KeyboardCapitalization
import androidx.compose.ui.text.input.KeyboardType
import androidx.compose.ui.unit.dp
import com.kyant.shapes.RoundedRectangle
import kotlinx.coroutines.CancellationException
import kotlinx.coroutines.launch
import live.renoa.app.agents.Problem
import live.renoa.app.agents.RepositoryException
import live.renoa.design.DesignSheet
import live.renoa.design.ModalHeader
import live.renoa.design.Motion
import live.renoa.design.Palette
import live.renoa.design.PlainTextField
import live.renoa.design.Symbol
import live.renoa.design.Type
import live.renoa.design.WideButton
import live.renoa.design.monospaced
import live.renoa.design.semibold

/** Symbols in the code a computer shows. */
internal const val COMPUTER_CODE_LENGTH = 8

/** What Renoa reads in a code: Crockford's base32, with O, I and L for the digits they look like. */
private const val CODE_SYMBOLS = "0123456789ABCDEFGHJKMNPQRSTVWXYZOIL"

/** Whether [typed] could be a whole code, in either case and spaced with spaces or dashes, as Renoa reads one. */
internal fun isComputerCode(typed: String): Boolean {
    val symbols = typed.filterNot { it == ' ' || it == '-' }
    return symbols.length == COMPUTER_CODE_LENGTH && symbols.all { it.uppercaseChar() in CODE_SYMBOLS }
}

/** What the owner reads when a computer could not be signed in. */
internal fun computerProblem(problem: Problem): String =
    when (problem) {
        Problem.NotFound -> "No computer is showing that code. Check it, or start again on the computer."
        Problem.Invalid -> "A code is the 8 letters and digits your computer shows."
        Problem.Conflict -> "This computer can't be signed in now. If several codes were wrong, wait five minutes; otherwise start again on the computer."
        Problem.Unavailable, Problem.NotYet -> problem.ownerWords
    }

private sealed interface Approval {
    data object Typing : Approval

    data object Working : Approval

    data class Failed(val message: String) : Approval

    data class Done(val name: String) : Approval
}

/**
 * Signs in a computer that shows a code: the owner types it and Renoa lets that computer in as them.
 * [approve] returns the name the computer gave.
 */
@Composable
internal fun ComputerSheet(open: Boolean, onDismiss: () -> Unit, approve: suspend (String) -> String) {
    var code by rememberSaveable { mutableStateOf("") }
    var approval by remember { mutableStateOf<Approval>(Approval.Typing) }
    val scope = rememberCoroutineScope()
    // Starts afresh each time the sheet opens, not as it closes, so its slide-out keeps what it showed. An
    // approval still on its way when the sheet closed keeps going, and shows when the sheet opens again.
    LaunchedEffect(open) {
        if (open && approval != Approval.Working) {
            code = ""
            approval = Approval.Typing
        }
    }
    val submit = {
        if (approval != Approval.Working && isComputerCode(code)) {
            approval = Approval.Working
            scope.launch {
                approval =
                    try {
                        Approval.Done(approve(code))
                    } catch (cancelled: CancellationException) {
                        throw cancelled
                    } catch (refused: RepositoryException) {
                        Approval.Failed(computerProblem(refused.problem))
                    }
            }
        }
    }
    DesignSheet(open, onDismiss) {
        Column(Modifier.fillMaxSize().background(Palette.background)) {
            ModalHeader("Sign in a computer")
            when (val shown = approval) {
                is Approval.Done -> Signed(shown.name, onDismiss)
                else -> Entry(code, { code = it; if (shown is Approval.Failed) approval = Approval.Typing }, shown, submit)
            }
        }
    }
}

@Composable
private fun Entry(code: String, onCode: (String) -> Unit, approval: Approval, submit: () -> Unit) {
    val working = approval == Approval.Working
    Column(Modifier.padding(horizontal = 24.dp).padding(top = 8.dp, bottom = 12.dp), verticalArrangement = Arrangement.spacedBy(20.dp)) {
        Text("Open Renoa on your computer. Type the code it shows here, and it signs in as you.", style = Type.body, color = Palette.secondary)
        Text(
            "Only enter a code from a computer in front of you. Whoever's computer shows it can use Renoa as you.",
            style = Type.footnote,
            color = Palette.secondary,
        )
        Box(
            Modifier.fillMaxWidth().clip(RoundedRectangle(12.dp)).background(Palette.surface).padding(horizontal = 14.dp, vertical = 12.dp),
        ) {
            PlainTextField(
                text = code,
                onChange = onCode,
                modifier = Modifier.fillMaxWidth().semantics { contentDescription = "Computer code" },
                prompt = "Code",
                singleLine = true,
                style = Type.title3.monospaced,
                enabled = !working,
                keyboard = KeyboardOptions(
                    capitalization = KeyboardCapitalization.Characters,
                    autoCorrectEnabled = false,
                    keyboardType = KeyboardType.Ascii,
                    imeAction = ImeAction.Go,
                ),
                actions = KeyboardActions(onGo = { submit() }),
            )
        }
        AnimatedVisibility(approval is Approval.Failed, enter = fadeIn(Motion.fade()), exit = fadeOut(Motion.fade())) {
            Text((approval as? Approval.Failed)?.message.orEmpty(), style = Type.footnote, color = Palette.danger)
        }
        WideButton("Sign in", enabled = isComputerCode(code) && !working, busy = working, action = submit)
    }
}

@Composable
private fun Signed(name: String, onDone: () -> Unit) {
    Column(Modifier.padding(horizontal = 24.dp).padding(top = 8.dp, bottom = 12.dp), verticalArrangement = Arrangement.spacedBy(20.dp)) {
        Row(verticalAlignment = Alignment.CenterVertically, horizontalArrangement = Arrangement.spacedBy(12.dp)) {
            Symbol("checkmark.circle.fill", size = 28.dp, tint = Palette.accent)
            Text("$name can sign in", style = Type.title3.semibold, color = Palette.text)
        }
        Text("Go back to your computer. It finishes signing in as you there.", style = Type.body, color = Palette.secondary)
        WideButton("Done", action = onDone)
    }
}
