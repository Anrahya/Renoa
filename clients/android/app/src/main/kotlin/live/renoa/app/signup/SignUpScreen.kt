package live.renoa.app.signup

import androidx.activity.compose.BackHandler
import androidx.compose.animation.AnimatedContent
import androidx.compose.animation.core.Animatable
import androidx.compose.animation.core.tween
import androidx.compose.animation.fadeIn
import androidx.compose.animation.fadeOut
import androidx.compose.animation.slideInHorizontally
import androidx.compose.animation.togetherWith
import androidx.compose.foundation.background
import androidx.compose.foundation.border
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.ColumnScope
import androidx.compose.foundation.layout.ExperimentalLayoutApi
import androidx.compose.foundation.layout.FlowRow
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.RowScope
import androidx.compose.foundation.layout.Spacer
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.heightIn
import androidx.compose.foundation.layout.imePadding
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.safeDrawingPadding
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.rememberScrollState
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.foundation.text.BasicTextField
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
import androidx.compose.ui.draw.drawBehind
import androidx.compose.ui.focus.FocusRequester
import androidx.compose.ui.focus.focusRequester
import androidx.compose.ui.geometry.Offset
import androidx.compose.ui.graphics.SolidColor
import androidx.compose.ui.graphics.graphicsLayer
import androidx.compose.ui.semantics.contentDescription
import androidx.compose.ui.semantics.heading
import androidx.compose.ui.semantics.semantics
import androidx.compose.ui.text.input.ImeAction
import androidx.compose.ui.text.input.KeyboardCapitalization
import androidx.compose.ui.unit.dp
import androidx.compose.ui.unit.sp
import live.renoa.app.agents.AgentState
import live.renoa.app.badge.AgentHue
import live.renoa.app.badge.Badge
import live.renoa.app.badge.MascotFigure
import live.renoa.app.theme.LocalReducedMotion
import live.renoa.app.theme.Renoa
import live.renoa.app.theme.RenoaEasing
import live.renoa.app.theme.RenoaType
import live.renoa.app.ui.ButtonKind
import live.renoa.app.ui.ButtonSize
import live.renoa.app.ui.RenoaButton

/** Sign-up's three screens, after which [onDone] opens Home on Renoa. */
@Composable
fun SignUpScreen(viewModel: SignUpViewModel, onDone: () -> Unit) {
    val state by viewModel.state.collectAsState()
    val colors = Renoa.colors
    LaunchedEffect(state.done) { if (state.done) onDone() }
    BackHandler(enabled = state.step == SignUpStep.Day) { viewModel.back() }
    AnimatedContent(
        targetState = state.step,
        modifier = Modifier.fillMaxSize().background(colors.background).safeDrawingPadding().imePadding(),
        transitionSpec = { (slideInHorizontally(tween(300)) { it / 8 } + fadeIn(tween(300))) togetherWith fadeOut(tween(150)) },
        label = "sign-up step",
    ) { step ->
        Column(Modifier.fillMaxSize().padding(start = 22.dp, end = 22.dp, top = 28.dp, bottom = 16.dp)) {
            when (step) {
                SignUpStep.Checking -> Checking(state, viewModel)
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
private fun ColumnScope.Checking(state: SignUpUiState, viewModel: SignUpViewModel) {
    Spacer(Modifier.weight(1f))
    Box(Modifier.fillMaxWidth(), contentAlignment = Alignment.Center) { Tile(96) }
    Spacer(Modifier.weight(1f))
    state.notice?.let {
        Notice(it)
        Footer { RenoaButton("Try again", viewModel::resume, Modifier.weight(1f)) }
    }
}

@Composable
private fun ColumnScope.NameStep(state: SignUpUiState, viewModel: SignUpViewModel) {
    val colors = Renoa.colors
    val focus = remember { FocusRequester() }
    LaunchedEffect(Unit) { focus.requestFocus() }
    Scrolling {
        Tile(60)
        Step(1)
        Heading("Hi. What should I call you?")
        Box(
            Modifier
                .padding(top = 8.dp)
                .fillMaxWidth()
                .drawBehind { drawLine(colors.input, Offset(0f, size.height), Offset(size.width, size.height), 1.5.dp.toPx()) }
                .padding(bottom = 8.dp),
        ) {
            val style = RenoaType.Title.copy(fontSize = 24.sp, lineHeight = 30.sp, fontWeight = RenoaType.Body.fontWeight)
            if (state.name.isEmpty()) Text("Your name", style = style, color = colors.mutedForeground)
            BasicTextField(
                value = state.name,
                onValueChange = viewModel::setName,
                singleLine = true,
                enabled = !state.busy,
                textStyle = style.copy(color = colors.foreground),
                cursorBrush = SolidColor(colors.ring),
                keyboardOptions = KeyboardOptions(capitalization = KeyboardCapitalization.Words, imeAction = ImeAction.Next),
                keyboardActions = KeyboardActions(onNext = { viewModel.saveName() }),
                modifier = Modifier.fillMaxWidth().focusRequester(focus).semantics { contentDescription = "Your name" },
            )
        }
    }
    state.notice?.let { Notice(it) }
    Footer { RenoaButton("Next", viewModel::saveName, Modifier.weight(1f), enabled = state.canName) }
}

@OptIn(ExperimentalLayoutApi::class)
@Composable
private fun ColumnScope.DayStep(state: SignUpUiState, viewModel: SignUpViewModel) {
    val colors = Renoa.colors
    val shape = RoundedCornerShape(18.dp)
    Scrolling {
        Tile(60)
        Step(2)
        Heading("How does a usual day go for you, ${state.name}?")
        Text("A sentence or two is plenty. I'll learn the rest as we talk.", style = RenoaType.Body, color = colors.foregroundSoft)
        Box(
            Modifier
                .padding(top = 4.dp)
                .fillMaxWidth()
                .heightIn(min = 108.dp)
                .background(colors.card, shape)
                .border(1.dp, colors.border, shape)
                .padding(horizontal = 16.dp, vertical = 14.dp),
        ) {
            if (state.day.isEmpty()) {
                Text("Up around 7, out till 6. I cook most nights and I'm trying to eat more protein.", style = RenoaType.Body, color = colors.mutedForeground)
            }
            BasicTextField(
                value = state.day,
                onValueChange = viewModel::setDay,
                enabled = !state.busy,
                textStyle = RenoaType.Body.copy(color = colors.foreground),
                cursorBrush = SolidColor(colors.ring),
                keyboardOptions = KeyboardOptions(capitalization = KeyboardCapitalization.Sentences),
                modifier = Modifier.fillMaxWidth().semantics { contentDescription = "Your usual day" },
            )
        }
        // A small button keeps room above and below for its tap area, so wrapped rows need no gap of their own.
        FlowRow(horizontalArrangement = Arrangement.spacedBy(8.dp)) {
            val picked = state.phrases
            DAY_PHRASES.forEach { phrase ->
                RenoaButton(phrase, { viewModel.togglePhrase(phrase) }, kind = if (phrase in picked) ButtonKind.Primary else ButtonKind.Secondary, size = ButtonSize.Sm, enabled = !state.busy)
            }
        }
    }
    state.notice?.let { Notice(it) }
    Footer {
        RenoaButton("Skip", { viewModel.saveDay(skip = true) }, Modifier.weight(1f), kind = ButtonKind.Secondary, enabled = !state.busy)
        RenoaButton("Next", { viewModel.saveDay() }, Modifier.weight(1f), enabled = !state.busy)
    }
}

@Composable
private fun ColumnScope.MeetStep(state: SignUpUiState, viewModel: SignUpViewModel, onDone: () -> Unit) {
    val colors = Renoa.colors
    val reduced = LocalReducedMotion.current
    // She arrives once: dropping in from a little above, as tiles assemble on Home.
    val arrival = remember { Animatable(if (reduced) 1f else 0f) }
    LaunchedEffect(Unit) { arrival.animateTo(1f, tween(800, easing = RenoaEasing.Ease)) }
    Scrolling(Alignment.CenterHorizontally, Arrangement.spacedBy(14.dp, Alignment.CenterVertically)) {
        MascotFigure(
            Modifier
                .padding(bottom = 12.dp)
                .size(width = 210.dp, height = 336.dp)
                .graphicsLayer {
                    alpha = arrival.value
                    translationY = (1f - arrival.value) * -36.dp.toPx()
                },
        )
        Column(Modifier.fillMaxWidth(), verticalArrangement = Arrangement.spacedBy(14.dp)) {
            Step(3)
            Heading("I'm Renoa.")
            Text("I keep track of things, remind you and find things out. Whenever you want to talk, tap me on Home.", style = RenoaType.Body, color = colors.foregroundSoft)
        }
    }
    when {
        state.notice != null -> {
            Notice(state.notice)
            Footer { RenoaButton("Try again", viewModel::meet, Modifier.weight(1f)) }
        }
        else -> {
            if (state.busy) Text("Getting Renoa ready…", Modifier.padding(bottom = 10.dp), style = RenoaType.Caption, color = colors.mutedForeground)
            Footer { RenoaButton("Go to Home", onDone, Modifier.weight(1f), enabled = state.ready) }
        }
    }
}

/** The screen's words, scrolling on a short screen so the buttons below stay in reach. */
@Composable
private fun ColumnScope.Scrolling(
    align: Alignment.Horizontal = Alignment.Start,
    arrange: Arrangement.Vertical = Arrangement.spacedBy(14.dp),
    content: @Composable ColumnScope.() -> Unit,
) {
    Column(Modifier.weight(1f).fillMaxWidth().verticalScroll(rememberScrollState()), verticalArrangement = arrange, horizontalAlignment = align, content = content)
}

/** Renoa's own tile, as Home shows her. */
@Composable
private fun Tile(size: Int) = Badge(RENOA, AgentState.Resting, size.dp, primary = true, hue = AgentHue.Ink, label = "")

@Composable
private fun Step(n: Int) = Text("$n OF 3", Modifier.padding(top = 6.dp), style = RenoaType.Eyebrow, color = Renoa.colors.mutedForeground)

@Composable
private fun Heading(text: String) = Text(text, Modifier.semantics { heading() }, style = RenoaType.Title, color = Renoa.colors.foreground)

@Composable
private fun Notice(text: String) = Text(text, Modifier.padding(bottom = 10.dp), style = RenoaType.Caption, color = Renoa.colors.stateFailed)

@Composable
private fun Footer(buttons: @Composable RowScope.() -> Unit) {
    Row(Modifier.fillMaxWidth().padding(top = 8.dp), horizontalArrangement = Arrangement.spacedBy(10.dp), content = buttons)
}
