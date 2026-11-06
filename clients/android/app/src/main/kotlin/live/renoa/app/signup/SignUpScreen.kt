// Portions copyright 2026 Po Kai Lee and the Codync contributors.
// Licensed under the Apache License, Version 2.0; see licenses/Apache-2.0-Codync.txt.
// Step chrome, springs and controls adapted from Codync apps/android `PairingView.kt` at ed8daf11f89398e598ef1f791abc3014ded68496.
package live.renoa.app.signup

import androidx.activity.compose.BackHandler
import androidx.compose.animation.AnimatedContent
import androidx.compose.animation.core.Animatable
import androidx.compose.animation.core.tween
import androidx.compose.animation.fadeIn
import androidx.compose.animation.fadeOut
import androidx.compose.animation.slideInHorizontally
import androidx.compose.animation.slideOutHorizontally
import androidx.compose.animation.togetherWith
import androidx.compose.foundation.background
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.ColumnScope
import androidx.compose.foundation.layout.ExperimentalLayoutApi
import androidx.compose.foundation.layout.FlowRow
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.Spacer
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.imePadding
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.safeDrawingPadding
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.rememberScrollState
import androidx.compose.foundation.text.KeyboardActions
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
import androidx.compose.ui.draw.clip
import androidx.compose.ui.focus.FocusRequester
import androidx.compose.ui.focus.focusRequester
import androidx.compose.ui.graphics.graphicsLayer
import androidx.compose.ui.semantics.contentDescription
import androidx.compose.ui.semantics.heading
import androidx.compose.ui.semantics.semantics
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.text.input.ImeAction
import androidx.compose.ui.text.input.KeyboardCapitalization
import androidx.compose.ui.text.style.TextAlign
import androidx.compose.ui.unit.dp
import androidx.compose.ui.unit.sp
import com.kyant.shapes.Capsule
import com.kyant.shapes.RoundedRectangle
import live.renoa.app.agents.AgentState
import live.renoa.app.badge.AgentHue
import live.renoa.app.badge.Badge
import live.renoa.app.theme.LocalReducedMotion
import live.renoa.app.theme.RenoaEasing
import live.renoa.design.BackButton
import live.renoa.design.FieldBox
import live.renoa.design.Motion
import live.renoa.design.Palette
import live.renoa.design.PlainTextField
import live.renoa.design.PrimaryButton
import live.renoa.design.ScreenHeader
import live.renoa.design.SecondaryButton
import live.renoa.design.ThinkingOrb
import live.renoa.design.ThinkingOrbState
import live.renoa.design.Type
import live.renoa.design.WideButton

/** Sign-up's three screens, after which [onDone] opens Home on Renoa. */
@Composable
fun SignUpScreen(viewModel: SignUpViewModel, onDone: () -> Unit) {
    val state by viewModel.state.collectAsState()
    LaunchedEffect(state.done) { if (state.done) onDone() }
    BackHandler(enabled = state.step == SignUpStep.Day && !state.busy) { viewModel.back() }
    val stepIndex =
        when (state.step) {
            SignUpStep.Checking -> 0
            SignUpStep.Name -> 1
            SignUpStep.Day -> 2
            SignUpStep.Meet -> 3
        }
    Column(Modifier.fillMaxSize().background(Palette.background).safeDrawingPadding().imePadding()) {
        if (stepIndex > 0) {
            ScreenHeader(
                leading = { if (state.step == SignUpStep.Day && !state.busy) BackButton(viewModel::back) },
                title = { StepPills(stepIndex) },
            )
        }
        AnimatedContent(
            targetState = state.step,
            modifier = Modifier.weight(1f),
            transitionSpec = {
                val forward = targetState.ordinal >= initialState.ordinal
                (slideInHorizontally(Motion.spring(0.45f)) { if (forward) it else -it } + fadeIn(Motion.spring(0.45f))) togetherWith
                    (slideOutHorizontally(Motion.spring(0.45f)) { if (forward) -it else it } + fadeOut(Motion.spring(0.45f)))
            },
            label = "Sign-up step",
        ) { step ->
            when (step) {
                SignUpStep.Checking -> CheckingStep(state, viewModel)
                SignUpStep.Name -> NameStep(state, viewModel)
                SignUpStep.Day -> DayStep(state, viewModel)
                SignUpStep.Meet ->
                    MeetStep(state, viewModel) {
                        viewModel.finish()
                        onDone()
                    }
            }
        }
    }
}

@Composable
private fun StepPills(step: Int) {
    Row(
        Modifier.semantics(mergeDescendants = true) { contentDescription = "Step $step of 3" },
        horizontalArrangement = Arrangement.spacedBy(6.dp),
    ) {
        for (i in 1..3) {
            Box(Modifier.size(18.dp, 4.dp).clip(Capsule()).background(if (i <= step) Palette.text else Palette.accentDim))
        }
    }
}

@Composable
private fun CheckingStep(state: SignUpUiState, viewModel: SignUpViewModel) {
    Column(Modifier.fillMaxSize().padding(horizontal = 24.dp).padding(bottom = 12.dp)) {
        Spacer(Modifier.weight(1f))
        Column(Modifier.fillMaxWidth(), horizontalAlignment = Alignment.CenterHorizontally, verticalArrangement = Arrangement.spacedBy(16.dp)) {
            Tile(96)
            if (state.busy) ThinkingOrb(state = ThinkingOrbState.Connecting, size = 22.dp, color = Palette.secondary)
        }
        Spacer(Modifier.weight(1f))
        state.notice?.let {
            Notice(it)
            WideButton("Try again", action = viewModel::resume)
        }
    }
}

@Composable
private fun NameStep(state: SignUpUiState, viewModel: SignUpViewModel) {
    val focus = remember { FocusRequester() }
    LaunchedEffect(Unit) { focus.requestFocus() }
    StepBody(
        footer = {
            state.notice?.let { Notice(it) }
            WideButton("Continue", enabled = state.canName, busy = state.busy, action = viewModel::saveName)
        },
    ) {
        Box(Modifier.fillMaxWidth().padding(top = 8.dp), contentAlignment = Alignment.Center) { Tile(64) }
        Heading("Hi. What should I call you?")
        Box(
            Modifier
                .fillMaxWidth()
                .clip(RoundedRectangle(12.dp))
                .background(Palette.surface)
                .padding(horizontal = 14.dp, vertical = 12.dp),
        ) {
            PlainTextField(
                text = state.name,
                onChange = viewModel::setName,
                modifier = Modifier.fillMaxWidth().focusRequester(focus).semantics { contentDescription = "Your name" },
                prompt = "Your name",
                singleLine = true,
                style = Type.title3,
                enabled = !state.busy,
                keyboard = KeyboardOptions(capitalization = KeyboardCapitalization.Words, imeAction = ImeAction.Next),
                actions = KeyboardActions(onNext = { viewModel.saveName() }),
            )
        }
    }
}

@OptIn(ExperimentalLayoutApi::class)
@Composable
private fun DayStep(state: SignUpUiState, viewModel: SignUpViewModel) {
    StepBody(
        footer = {
            state.notice?.let { Notice(it) }
            Column(verticalArrangement = Arrangement.spacedBy(10.dp)) {
                WideButton("Continue", enabled = !state.busy, busy = state.busy) { viewModel.saveDay() }
                WideButton("Skip", primary = false, enabled = !state.busy) { viewModel.saveDay(skip = true) }
            }
        },
    ) {
        Box(Modifier.fillMaxWidth().padding(top = 8.dp), contentAlignment = Alignment.Center) { Tile(64) }
        Column(verticalArrangement = Arrangement.spacedBy(10.dp)) {
            Heading("How does a usual day go for you, ${state.name}?")
            Text("A sentence or two is plenty. I'll learn the rest as we talk.", style = Type.body, color = Palette.secondary)
        }
        FieldBox(
            text = state.day,
            onChange = viewModel::setDay,
            prompt = "Up around 7, out till 6. I cook most nights and I'm trying to eat more protein.",
            fieldModifier = Modifier.semantics { contentDescription = "Your usual day" },
            minLines = 4,
            maxLines = 6,
            fill = Palette.surface,
            enabled = !state.busy,
        )
        FlowRow(horizontalArrangement = Arrangement.spacedBy(8.dp), verticalArrangement = Arrangement.spacedBy(8.dp)) {
            val picked = state.phrases
            for (phrase in DAY_PHRASES) {
                if (phrase in picked) {
                    PrimaryButton(phrase, enabled = !state.busy) { viewModel.togglePhrase(phrase) }
                } else {
                    SecondaryButton(phrase, enabled = !state.busy) { viewModel.togglePhrase(phrase) }
                }
            }
        }
    }
}

@Composable
private fun MeetStep(state: SignUpUiState, viewModel: SignUpViewModel, onDone: () -> Unit) {
    val reduced = LocalReducedMotion.current
    val arrival = remember { Animatable(if (reduced) 1f else 0f) }
    LaunchedEffect(Unit) { arrival.animateTo(1f, tween(800, easing = RenoaEasing.Ease)) }
    StepBody(
        align = Alignment.CenterHorizontally,
        arrange = Arrangement.spacedBy(18.dp, Alignment.CenterVertically),
        footer = {
            when {
                state.notice != null -> {
                    Notice(state.notice)
                    WideButton("Try again", action = viewModel::meet)
                }
                else -> {
                    if (state.busy) {
                        Row(
                            Modifier.fillMaxWidth().padding(bottom = 10.dp),
                            horizontalArrangement = Arrangement.spacedBy(8.dp, Alignment.CenterHorizontally),
                            verticalAlignment = Alignment.CenterVertically,
                        ) {
                            ThinkingOrb(state = ThinkingOrbState.Shaping, size = 18.dp, color = Palette.secondary)
                            Text("Getting Renoa ready…", style = Type.footnote, color = Palette.secondary)
                        }
                    }
                    WideButton("Go to Home", enabled = state.ready, busy = state.busy, action = onDone)
                }
            }
        },
    ) {
        Badge(
            RENOA,
            AgentState.Resting,
            112.dp,
            primary = true,
            hue = AgentHue.Ink,
            modifier =
                Modifier.graphicsLayer {
                    alpha = arrival.value
                    translationY = (1f - arrival.value) * -28.dp.toPx()
                },
        )
        Column(horizontalAlignment = Alignment.CenterHorizontally, verticalArrangement = Arrangement.spacedBy(10.dp)) {
            Text("I'm Renoa.", Modifier.semantics { heading() }, style = PAGE_TITLE, color = Palette.text, textAlign = TextAlign.Center)
            Text(
                "I keep track of things, remind you and find things out. Whenever you want to talk, tap me on Home.",
                style = Type.body,
                color = Palette.secondary,
                textAlign = TextAlign.Center,
            )
        }
    }
}

@Composable
private fun StepBody(
    align: Alignment.Horizontal = Alignment.Start,
    arrange: Arrangement.Vertical = Arrangement.spacedBy(20.dp),
    footer: @Composable ColumnScope.() -> Unit,
    content: @Composable ColumnScope.() -> Unit,
) {
    Column(Modifier.fillMaxSize()) {
        Column(
            Modifier.weight(1f).fillMaxWidth().verticalScroll(rememberScrollState()).padding(horizontal = 24.dp).padding(bottom = 16.dp),
            verticalArrangement = arrange,
            horizontalAlignment = align,
            content = content,
        )
        Column(Modifier.fillMaxWidth().padding(horizontal = 24.dp).padding(bottom = 12.dp), content = footer)
    }
}

@Composable
private fun Tile(size: Int) = Badge(RENOA, AgentState.Resting, size.dp, primary = true, hue = AgentHue.Ink, label = "")

@Composable
private fun Heading(text: String) = Text(text, Modifier.semantics { heading() }, style = PAGE_TITLE, color = Palette.text)

@Composable
private fun Notice(text: String) = Text(text, Modifier.padding(bottom = 10.dp), style = Type.footnote, color = Palette.danger)

private val PAGE_TITLE = Type.system(30f, FontWeight.SemiBold).copy(letterSpacing = (-0.5).sp)
