// Portions copyright 2026 Po Kai Lee and the Codync contributors.
// Licensed under the Apache License, Version 2.0; see licenses/Apache-2.0-Codync.txt.
// Adapted from Codync apps/android `NewBotView.kt`, `BotEditor.kt` and `BotSettingsForm.kt`
// at ed8daf11f89398e598ef1f791abc3014ded68496.
package live.renoa.app.editor

import androidx.compose.foundation.background
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.PaddingValues
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.rememberScrollState
import androidx.compose.foundation.text.KeyboardOptions
import androidx.compose.foundation.verticalScroll
import androidx.compose.material3.Text
import androidx.compose.runtime.Composable
import androidx.compose.runtime.CompositionLocalProvider
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.collectAsState
import androidx.compose.runtime.getValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.draw.clip
import androidx.compose.ui.semantics.contentDescription
import androidx.compose.ui.semantics.semantics
import androidx.compose.ui.text.input.ImeAction
import androidx.compose.ui.text.input.KeyboardCapitalization
import androidx.compose.ui.text.style.TextAlign
import androidx.compose.ui.unit.dp
import com.kyant.shapes.RoundedRectangle
import live.renoa.app.agents.AbilityKind
import live.renoa.app.agents.AbilitySwitch
import live.renoa.app.agents.AgentState
import live.renoa.app.badge.Badge
import live.renoa.design.CardForm
import live.renoa.design.CardSection
import live.renoa.design.Field
import live.renoa.design.FieldBox
import live.renoa.design.IconButton
import live.renoa.design.LinkButton
import live.renoa.design.LocalDismissModal
import live.renoa.design.ModalHeader
import live.renoa.design.Palette
import live.renoa.design.PlainTextField
import live.renoa.design.Spinner
import live.renoa.design.SwitchRow
import live.renoa.design.Type
import live.renoa.design.ValueRow
import live.renoa.design.WideButton
import live.renoa.design.semibold

/** Make an agent (`NewBotView` sheet), or edit one (`BotEditor` + `BotSettingsForm` sheet). */
@Composable
fun AgentEditorScreen(viewModel: AgentEditorViewModel, onClose: () -> Unit, onFinished: (agentId: String) -> Unit) {
    val state by viewModel.state.collectAsState()
    val called = state.name.trim().ifEmpty { "this one" }

    LaunchedEffect(state.finishedAgentId) { state.finishedAgentId?.let(onFinished) }

    CompositionLocalProvider(LocalDismissModal provides onClose) {
        if (state.creating) {
            CreateAgentSheet(state, called, viewModel)
        } else {
            EditAgentSheet(state, called, viewModel)
        }
    }
}

@Composable
private fun CreateAgentSheet(state: EditorUiState, called: String, viewModel: AgentEditorViewModel) {
    Column(Modifier.fillMaxSize().background(Palette.background)) {
        ModalHeader("New agent") {
            if (state.saving) {
                Box(Modifier.size(36.dp), contentAlignment = Alignment.Center) { Spinner() }
            } else {
                IconButton("Create", "checkmark", enabled = state.canSave, action = viewModel::save)
            }
        }
        Column(
            Modifier
                .weight(1f)
                .verticalScroll(rememberScrollState())
                .padding(start = 20.dp, end = 20.dp, top = 8.dp, bottom = 20.dp),
            horizontalAlignment = Alignment.CenterHorizontally,
            verticalArrangement = Arrangement.spacedBy(22.dp),
        ) {
            Badge(state.name.ifBlank { "?" }, AgentState.Resting, 96.dp, state.main, state.look, label = "")
            Column(Modifier.fillMaxWidth(), horizontalAlignment = Alignment.CenterHorizontally, verticalArrangement = Arrangement.spacedBy(6.dp)) {
                PlainTextField(
                    text = state.name,
                    onChange = viewModel::setName,
                    modifier = Modifier.fillMaxWidth().semantics { contentDescription = "Name" },
                    prompt = "Name your agent",
                    singleLine = true,
                    style = Type.title2.semibold.copy(textAlign = TextAlign.Center),
                    keyboard = KeyboardOptions(capitalization = KeyboardCapitalization.Words, imeAction = ImeAction.Next),
                )
                if (state.name.isNotEmpty() && state.nameProblem != null) {
                    Text(
                        "A name needs at least one letter and fewer than 512 bytes.",
                        style = Type.caption,
                        color = Palette.danger,
                        textAlign = TextAlign.Center,
                    )
                }
            }
            CardSection {
                row {
                    Column(verticalArrangement = Arrangement.spacedBy(6.dp)) {
                        Field("What $called is for") {
                            FieldBox(
                                text = state.purpose,
                                onChange = viewModel::setPurpose,
                                prompt = "Keep the house running. Groceries, bills, reminders.",
                                fieldModifier = Modifier.semantics { contentDescription = "What $called is for" },
                                minLines = 2,
                                maxLines = 5,
                            )
                        }
                        if (state.purposeProblem != null) {
                            Text("That is too long, or has characters that cannot be kept.", style = Type.caption, color = Palette.danger)
                        }
                    }
                }
            }
            state.notice?.let { Text(it, Modifier.fillMaxWidth(), style = Type.footnote, color = Palette.danger) }
            if (state.abilities.isNotEmpty()) {
                AbilitiesSection(state.abilities, creating = true, called = called, onToggle = viewModel::toggle)
            }
        }
        Box(Modifier.padding(start = 20.dp, end = 20.dp, bottom = 18.dp)) {
            WideButton(
                title = "Bring ${state.name.trim().ifEmpty { "them" }} to life",
                enabled = state.canSave,
                busy = state.saving,
                action = viewModel::save,
            )
        }
    }
}

@Composable
private fun EditAgentSheet(state: EditorUiState, called: String, viewModel: AgentEditorViewModel) {
    Column(Modifier.fillMaxSize().background(Palette.background)) {
        ModalHeader("Profile") {
            if (state.saving) {
                Box(Modifier.size(36.dp), contentAlignment = Alignment.Center) { Spinner() }
            } else {
                LinkButton("Done", enabled = state.canSave, style = Type.body.semibold, action = viewModel::save)
            }
        }
        CardForm(
            Modifier.weight(1f),
            padding = PaddingValues(start = 20.dp, end = 20.dp, top = 8.dp, bottom = 24.dp),
        ) {
            CardSection {
                row {
                    ValueRow("Avatar") {
                        Box(Modifier.clip(RoundedRectangle(10.dp)).background(Palette.bubbleAgent).padding(4.dp)) {
                            Badge(state.name.ifBlank { "?" }, AgentState.Resting, 32.dp, state.main, state.look, label = "")
                        }
                    }
                }
                row {
                    Column(verticalArrangement = Arrangement.spacedBy(6.dp)) {
                        Field("Name") {
                            FieldBox(
                                text = state.name,
                                onChange = viewModel::setName,
                                prompt = "Name",
                                fieldModifier = Modifier.semantics { contentDescription = "Name" },
                                imeAction = ImeAction.Next,
                            )
                        }
                        if (state.name.isNotEmpty() && state.nameProblem != null) {
                            Text("A name needs at least one letter and fewer than 512 bytes.", style = Type.caption, color = Palette.danger)
                        }
                    }
                }
                row {
                    Column(verticalArrangement = Arrangement.spacedBy(6.dp)) {
                        Field("What $called is for") {
                            FieldBox(
                                text = state.purpose,
                                onChange = viewModel::setPurpose,
                                prompt = "Keep the house running. Groceries, bills, reminders.",
                                fieldModifier = Modifier.semantics { contentDescription = "What $called is for" },
                                minLines = 2,
                                maxLines = 6,
                            )
                        }
                        if (state.purposeProblem != null) {
                            Text("That is too long, or has characters that cannot be kept.", style = Type.caption, color = Palette.danger)
                        }
                    }
                }
            }
            state.notice?.let { Text(it, style = Type.footnote, color = Palette.danger) }
            // Renoa does not list abilities to the app yet; an empty card would only look broken.
            if (state.abilities.isNotEmpty()) {
                AbilitiesSection(state.abilities, creating = false, called = called, onToggle = viewModel::toggle)
            }
        }
    }
}

@Composable
private fun AbilitiesSection(abilities: List<AbilitySwitch>, creating: Boolean, called: String, onToggle: (String) -> Unit) {
    CardSection(title = "What $called can do") {
        for (ability in abilities) {
            row {
                SwitchRow(
                    title = ability.title,
                    detail = abilityDetail(ability, creating, called),
                    isOn = ability.lit,
                    onChange = { onToggle(ability.id) },
                    enabled = ability.canSwitch(creating),
                )
            }
        }
    }
}

private fun abilityDetail(ability: AbilitySwitch, creating: Boolean, called: String): String? =
    when {
        ability.addedByAgent -> "$called found and added ${ability.title} itself"
        ability.kind == AbilityKind.Soon -> "Coming soon"
        ability.kind == AbilityKind.Machine && creating -> "Only you can switch this on"
        ability.kind == AbilityKind.Machine -> "Chosen when an agent is made"
        else -> null
    }
