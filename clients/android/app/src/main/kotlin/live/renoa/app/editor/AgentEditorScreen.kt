package live.renoa.app.editor

import androidx.compose.foundation.Canvas
import androidx.compose.foundation.background
import androidx.compose.foundation.border
import androidx.compose.foundation.clickable
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.Spacer
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.height
import androidx.compose.foundation.layout.heightIn
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.safeDrawingPadding
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.rememberScrollState
import androidx.compose.foundation.shape.CircleShape
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.foundation.text.BasicTextField
import androidx.compose.foundation.text.KeyboardOptions
import androidx.compose.foundation.verticalScroll
import androidx.compose.material3.Text
import androidx.compose.runtime.Composable
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.collectAsState
import androidx.compose.runtime.getValue
import androidx.compose.runtime.remember
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.draw.alpha
import androidx.compose.ui.draw.clip
import androidx.compose.ui.draw.drawBehind
import androidx.compose.ui.geometry.Offset
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.graphics.SolidColor
import androidx.compose.ui.graphics.graphicsLayer
import androidx.compose.ui.semantics.Role
import androidx.compose.ui.semantics.contentDescription
import androidx.compose.ui.semantics.semantics
import androidx.compose.ui.text.input.ImeAction
import androidx.compose.ui.text.input.KeyboardCapitalization
import androidx.compose.ui.text.style.TextAlign
import androidx.compose.ui.unit.Dp
import androidx.compose.ui.unit.dp
import androidx.compose.ui.unit.sp
import live.renoa.app.agents.AbilityKind
import live.renoa.app.agents.AgentState
import live.renoa.app.badge.AgentLook
import live.renoa.app.badge.Badge
import live.renoa.app.theme.Renoa
import live.renoa.app.theme.RenoaType
import live.renoa.app.ui.Glyphs
import live.renoa.app.ui.RenoaButton
import live.renoa.app.ui.PathIcon

/** Make an agent, or edit one. The two share one screen: a picture, a name, what it is for, and its switches. */
@Composable
fun AgentEditorScreen(viewModel: AgentEditorViewModel, onClose: () -> Unit, onFinished: (agentId: String) -> Unit) {
    val state by viewModel.state.collectAsState()
    val colors = Renoa.colors
    val look = remember(state.name) { AgentLook.forName(state.name.ifBlank { "?" }) }
    val called = state.name.trim().ifEmpty { "this one" }

    LaunchedEffect(state.finishedAgentId) { state.finishedAgentId?.let(onFinished) }

    Box(Modifier.fillMaxSize().background(colors.background)) {
        Column(Modifier.fillMaxSize().safeDrawingPadding()) {
            Row(Modifier.fillMaxWidth().padding(start = 4.dp, end = 12.dp, top = 10.dp), verticalAlignment = Alignment.CenterVertically) {
                Box(Modifier.size(44.dp).clip(CircleShape).clickable(role = Role.Button, onClickLabel = "Close", onClick = onClose), contentAlignment = Alignment.Center) {
                    PathIcon(Glyphs.CLOSE, colors.foreground, size = 20.dp)
                }
                Spacer(Modifier.weight(1f))
                if (!state.creating) {
                    Pill("Done", state.canSave, onClick = viewModel::save)
                }
            }
            Column(Modifier.weight(1f).verticalScroll(rememberScrollState())) {
                if (state.creating) CreateHeader(state, look, viewModel) else EditHeader(state, look, viewModel)
                Purpose(state, called, viewModel)
                Row(Modifier.fillMaxWidth().padding(start = 24.dp, end = 24.dp, top = 22.dp), horizontalArrangement = Arrangement.SpaceBetween, verticalAlignment = Alignment.Bottom) {
                    Text("What $called can do", style = RenoaType.Heading, color = colors.foreground)
                    Text("Tap to switch", style = RenoaType.Caption, color = colors.mutedForeground)
                }
                Box(Modifier.fillMaxWidth().padding(top = 4.dp), contentAlignment = Alignment.TopCenter) {
                    AbilityGrid(state.abilities, state.creating, look.main, viewModel::toggle)
                }
                Legend(state, called)
                state.notice?.let { Text(it, Modifier.padding(horizontal = 28.dp, vertical = 8.dp), style = RenoaType.Caption, color = colors.stateFailed) }
                Spacer(Modifier.height(16.dp))
            }
            if (state.creating) {
                Box(Modifier.padding(start = 20.dp, end = 20.dp, bottom = 22.dp)) {
                    Pill("Bring ${state.name.trim().ifEmpty { "them" }} to life", state.canSave, onClick = viewModel::save, wide = true)
                }
            }
        }
    }
}

@Composable
private fun Pill(text: String, enabled: Boolean, onClick: () -> Unit, wide: Boolean = false) {
    RenoaButton(text, onClick, if (wide) Modifier.fillMaxWidth() else Modifier, enabled = enabled)
}

@Composable
private fun CreateHeader(state: EditorUiState, look: AgentLook, viewModel: AgentEditorViewModel) {
    val colors = Renoa.colors
    Column(Modifier.fillMaxWidth().padding(top = 6.dp), horizontalAlignment = Alignment.CenterHorizontally) {
        Badge(look, AgentState.Resting, 164.dp, label = state.name.trim())
        Swatches(look, Modifier.padding(top = 4.dp), 20.dp)
        Box(Modifier.fillMaxWidth().padding(horizontal = 24.dp, vertical = 18.dp), contentAlignment = Alignment.Center) {
            NameField(state, viewModel, TextAlign.Center)
        }
        if (state.name.isNotEmpty() && state.nameProblem != null) {
            Text("A name needs at least one letter and fewer than 512 bytes.", style = RenoaType.Caption, color = colors.stateFailed)
        }
    }
}

@Composable
private fun EditHeader(state: EditorUiState, look: AgentLook, viewModel: AgentEditorViewModel) {
    Row(Modifier.fillMaxWidth().padding(start = 20.dp, end = 20.dp, top = 2.dp), verticalAlignment = Alignment.CenterVertically, horizontalArrangement = Arrangement.spacedBy(16.dp)) {
        Badge(look, AgentState.Resting, 104.dp, label = state.name.trim())
        Column(Modifier.weight(1f)) {
            NameField(state, viewModel, TextAlign.Start)
            Swatches(look, Modifier.padding(top = 10.dp), 16.dp)
        }
    }
}

@Composable
private fun NameField(state: EditorUiState, viewModel: AgentEditorViewModel, align: TextAlign) {
    val colors = Renoa.colors
    Box(
        Modifier
            .fillMaxWidth()
            .drawBehind { drawLine(colors.input, Offset(0f, size.height), Offset(size.width, size.height), 1.5.dp.toPx()) }
            .padding(bottom = 6.dp),
    ) {
        if (state.name.isEmpty()) {
            Text("A name", Modifier.fillMaxWidth(), style = RenoaType.Title.copy(fontSize = 30.sp, lineHeight = 36.sp), color = colors.mutedForeground, textAlign = align)
        }
        BasicTextField(
            value = state.name,
            onValueChange = viewModel::setName,
            singleLine = true,
            textStyle = RenoaType.Title.copy(fontSize = 30.sp, lineHeight = 36.sp, color = colors.foreground, textAlign = align),
            cursorBrush = SolidColor(colors.ring),
            keyboardOptions = KeyboardOptions(capitalization = KeyboardCapitalization.Words, imeAction = ImeAction.Next),
            modifier = Modifier.fillMaxWidth().semantics { contentDescription = "Name" },
        )
    }
}

@Composable
private fun Swatches(look: AgentLook, modifier: Modifier, size: Dp) {
    Row(modifier, horizontalArrangement = Arrangement.spacedBy(if (size > 16.dp) 8.dp else 5.dp)) {
        look.tones.forEach { Canvas(Modifier.size(size)) { drawCircle(it) } }
    }
}

@Composable
private fun Purpose(state: EditorUiState, called: String, viewModel: AgentEditorViewModel) {
    val colors = Renoa.colors
    Column(Modifier.padding(start = 20.dp, end = 20.dp, top = 16.dp)) {
        if (state.creating) {
            Text("What is $called for?", Modifier.padding(horizontal = 4.dp), style = RenoaType.Heading, color = colors.foreground)
        }
        Box(
            Modifier
                .padding(top = if (state.creating) 10.dp else 0.dp)
                .fillMaxWidth()
                .heightIn(min = 76.dp)
                .background(colors.card, RoundedCornerShape(if (state.creating) 20.dp else 18.dp))
                .border(1.dp, colors.border, RoundedCornerShape(if (state.creating) 20.dp else 18.dp))
                .padding(horizontal = 16.dp, vertical = 14.dp),
        ) {
            if (state.purpose.isEmpty()) {
                Text("Keep the house running. Groceries, bills, reminders.", style = RenoaType.Body, color = colors.mutedForeground)
            }
            BasicTextField(
                value = state.purpose,
                onValueChange = viewModel::setPurpose,
                textStyle = RenoaType.Body.copy(color = colors.foregroundSoft),
                cursorBrush = SolidColor(colors.ring),
                keyboardOptions = KeyboardOptions(capitalization = KeyboardCapitalization.Sentences),
                modifier = Modifier.fillMaxWidth().semantics { contentDescription = "What $called is for" },
            )
        }
        if (state.purposeProblem != null) {
            Text("That is too long, or has characters that cannot be kept.", Modifier.padding(start = 4.dp, top = 6.dp), style = RenoaType.Caption, color = colors.stateFailed)
        }
    }
}

@Composable
private fun Legend(state: EditorUiState, called: String) {
    val colors = Renoa.colors
    val added = state.abilities.filter { it.addedByAgent }
    val fixed = state.abilities.any { it.kind != AbilityKind.Everyday }
    Column(Modifier.padding(start = 28.dp, end = 28.dp, top = 6.dp), verticalArrangement = Arrangement.spacedBy(8.dp)) {
        if (added.isNotEmpty()) {
            LegendLine("$called found and added ${added.joinToString(" and ") { it.title }} itself", colors.foreground)
        }
        if (fixed) {
            LegendLine(if (state.creating) "Only you can switch these on" else "These are chosen when an agent is made", colors.foreground)
        }
    }
}

@Composable
private fun LegendLine(text: String, dot: Color) {
    Row(verticalAlignment = Alignment.CenterVertically, horizontalArrangement = Arrangement.spacedBy(10.dp)) {
        Canvas(Modifier.size(14.dp)) { drawCircle(dot) }
        Text(text, style = RenoaType.Caption, color = Renoa.colors.mutedForeground)
    }
}
