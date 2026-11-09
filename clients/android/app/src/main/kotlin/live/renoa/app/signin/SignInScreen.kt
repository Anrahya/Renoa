// Portions copyright 2026 Po Kai Lee and the Codync contributors.
// Licensed under the Apache License, Version 2.0; see licenses/Apache-2.0-Codync.txt.
// Adapted from Codync apps/android `WelcomeView.kt` and `PairingView.kt` at ed8daf11f89398e598ef1f791abc3014ded68496.
package live.renoa.app.signin

import androidx.activity.compose.BackHandler
import androidx.compose.animation.AnimatedContent
import androidx.compose.animation.AnimatedVisibility
import androidx.compose.animation.core.Animatable
import androidx.compose.animation.core.animateFloatAsState
import androidx.compose.animation.fadeIn
import androidx.compose.animation.fadeOut
import androidx.compose.animation.scaleIn
import androidx.compose.animation.slideInHorizontally
import androidx.compose.animation.slideOutHorizontally
import androidx.compose.animation.togetherWith
import androidx.compose.foundation.background
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.BoxWithConstraints
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.Spacer
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.height
import androidx.compose.foundation.layout.heightIn
import androidx.compose.foundation.layout.imePadding
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.safeDrawingPadding
import androidx.compose.foundation.rememberScrollState
import androidx.compose.foundation.text.KeyboardActions
import androidx.compose.foundation.text.KeyboardOptions
import androidx.compose.foundation.verticalScroll
import androidx.compose.material3.Text
import androidx.compose.runtime.Composable
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableIntStateOf
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.saveable.rememberSaveable
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.draw.clip
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.graphics.TransformOrigin
import androidx.compose.ui.graphics.graphicsLayer
import androidx.compose.ui.platform.LocalDensity
import androidx.compose.ui.semantics.contentDescription
import androidx.compose.ui.semantics.semantics
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.text.input.ImeAction
import androidx.compose.ui.text.input.KeyboardCapitalization
import androidx.compose.ui.text.input.KeyboardType
import androidx.compose.ui.unit.dp
import androidx.compose.ui.unit.sp
import com.kyant.shapes.RoundedRectangle
import kotlinx.coroutines.delay
import live.renoa.app.agents.AgentState
import live.renoa.app.agents.DEMO_LOOKS
import live.renoa.app.badge.Badge
import live.renoa.app.theme.LocalMotionClock
import live.renoa.app.theme.LocalReducedMotion
import live.renoa.design.BackButton
import live.renoa.design.Motion
import live.renoa.design.Palette
import live.renoa.design.PlainTextField
import live.renoa.design.ScreenHeader
import live.renoa.design.Symbol
import live.renoa.design.Type
import live.renoa.design.WideButton
import live.renoa.design.monospaced
import kotlin.math.sin

private enum class SignInStep { Welcome, Code }

/**
 * Onboarding and sign-in: Codync's four-beat welcome pitch followed by the passkey code screen.
 * When [state] already carries an error or work in progress, or [code] is already typed, the code
 * screen opens right away.
 */
@Composable
internal fun SignInScreen(state: SignInState, code: String, onCodeChange: (String) -> Unit, onSubmit: () -> Unit) {
    val startsOnCode = state != SignInState.Ready || code.isNotEmpty()
    var step by rememberSaveable { mutableStateOf(if (startsOnCode) SignInStep.Code else SignInStep.Welcome) }
    val working = state == SignInState.Working
    LaunchedEffect(state) {
        if (state != SignInState.Ready) step = SignInStep.Code
    }
    BackHandler(enabled = step == SignInStep.Code && !working) { step = SignInStep.Welcome }
    AnimatedContent(
        targetState = step,
        modifier = Modifier.fillMaxSize().background(Palette.background).safeDrawingPadding().imePadding(),
        transitionSpec = {
            val forward = targetState == SignInStep.Code
            (slideInHorizontally(Motion.spring(0.45f)) { if (forward) it else -it } + fadeIn(Motion.spring(0.45f))) togetherWith
                (slideOutHorizontally(Motion.spring(0.45f)) { if (forward) -it else it } + fadeOut(Motion.spring(0.45f)))
        },
        label = "Sign-in step",
    ) { shown ->
        when (shown) {
            SignInStep.Welcome -> WelcomePage(onStart = { step = SignInStep.Code })
            SignInStep.Code -> CodePage(state, code, onCodeChange, onBack = { step = SignInStep.Welcome }, onSubmit = onSubmit)
        }
    }
}

@Composable
private fun WelcomePage(onStart: () -> Unit) {
    val still = LocalReducedMotion.current
    var beat by remember { mutableIntStateOf(if (still) 4 else 0) }
    LaunchedEffect(Unit) {
        for (next in beat + 1..4) {
            delay(if (next == 1) 150 else 280)
            beat = next
        }
    }
    BoxWithConstraints(Modifier.fillMaxSize()) {
        val height = maxHeight
        Column(Modifier.fillMaxSize().verticalScroll(rememberScrollState())) {
            Column(
                Modifier.fillMaxWidth().heightIn(min = height).padding(horizontal = 24.dp).padding(bottom = 12.dp),
                verticalArrangement = Arrangement.SpaceBetween,
            ) {
                Spacer(Modifier.height(0.dp))
                AgentCluster(beat >= 1, Modifier.padding(top = 24.dp))
                Column(Modifier.padding(top = 24.dp)) {
                    ChatGlimpse(beat >= 2, Modifier.padding(bottom = 24.dp))
                    Column(Modifier.rise(beat >= 3), verticalArrangement = Arrangement.spacedBy(10.dp)) {
                        Text(
                            "Your agents,\nin your pocket.",
                            style = Type.system(34f, FontWeight.SemiBold).copy(letterSpacing = (-0.6).sp),
                            color = Palette.text,
                        )
                        Text(
                            "Give each one a name and a job. They keep track of things, remind you, and work while you're away.",
                            style = Type.body,
                            color = Palette.secondary,
                        )
                    }
                }
                Column(Modifier.padding(top = 32.dp).rise(beat >= 4), verticalArrangement = Arrangement.spacedBy(10.dp)) {
                    WideButton("Get started", action = onStart)
                }
            }
        }
    }
}

private class ClusterAgent(val name: String, val primary: Boolean, val size: Int, val x: Float, val y: Float, val state: AgentState)

private val CLUSTER = listOf(
    ClusterAgent("Renoa", true, 92, 0f, 0f, AgentState.Working),
    ClusterAgent("Tifa", false, 64, -104f, -34f, AgentState.Resting),
    ClusterAgent("Optimus", false, 58, 100f, -46f, AgentState.Working),
    ClusterAgent("Echo", false, 48, -76f, 64f, AgentState.Resting),
    ClusterAgent("Mira", false, 52, 84f, 58f, AgentState.Wants),
)

/** Five Renoa hex badges bobbing out of step on the welcome stage, driven by [LocalMotionClock]. */
@Composable
private fun AgentCluster(shown: Boolean, modifier: Modifier = Modifier) {
    val still = LocalReducedMotion.current
    val clock = LocalMotionClock.current
    val density = LocalDensity.current
    Box(modifier.fillMaxWidth().height(170.dp), contentAlignment = Alignment.Center) {
        for ((index, member) in CLUSTER.withIndex()) {
            val entry = remember { Animatable(if (still) 1f else 0f) }
            LaunchedEffect(shown) {
                if (shown) {
                    delay(index * 70L)
                    entry.animateTo(1f, Motion.spring(0.7f, 0.4f))
                } else {
                    entry.snapTo(0f)
                }
            }
            Box(
                Modifier.graphicsLayer {
                    val bob = if (still) 0f else sin(clock.value * 1.3f + index * 1.7f) * 5f
                    translationX = with(density) { member.x.dp.toPx() }
                    translationY = with(density) { (member.y + bob).dp.toPx() }
                    val scale = 0.3f + 0.7f * entry.value
                    scaleX = scale
                    scaleY = scale
                    alpha = entry.value.coerceIn(0f, 1f)
                },
            ) {
                Badge(member.name, member.state, member.size.dp, primary = member.primary, look = DEMO_LOOKS[member.name], label = "")
            }
        }
    }
}

/** One short exchange, played once: you ask, Renoa works, Renoa answers. */
@Composable
private fun ChatGlimpse(shown: Boolean, modifier: Modifier = Modifier) {
    val still = LocalReducedMotion.current
    var replied by remember { mutableStateOf(still) }
    LaunchedEffect(shown) {
        if (shown && !replied) {
            delay(1600)
            replied = true
        }
    }
    Column(modifier.fillMaxWidth().semantics(mergeDescendants = true) {}, verticalArrangement = Arrangement.spacedBy(10.dp)) {
        Box(Modifier.fillMaxWidth().rise(shown), contentAlignment = Alignment.CenterEnd) {
            GlimpseBubble("What's the plan for dinner?", Palette.bubbleUser)
        }
        Row(Modifier.rise(shown), verticalAlignment = Alignment.Bottom, horizontalArrangement = Arrangement.spacedBy(8.dp)) {
            Badge("Renoa", if (replied) AgentState.Resting else AgentState.Working, 28.dp, primary = true, look = null, label = "")
            AnimatedContent(
                replied,
                transitionSpec = {
                    (fadeIn(Motion.spring(0.45f, 0.2f)) + scaleIn(Motion.spring(0.45f, 0.2f), 0.92f, TransformOrigin(0f, 1f))) togetherWith
                        fadeOut(Motion.fade())
                },
                label = "Reply",
            ) { done ->
                if (done) GlimpseBubble("Dal with brown rice, and a quick salad on the side.", Palette.bubbleAgent)
                else Text("Working…", style = Type.subheadline, color = Palette.tertiary)
            }
        }
    }
}

@Composable
private fun GlimpseBubble(text: String, fill: Color) =
    Text(
        text,
        Modifier.clip(RoundedRectangle(18.dp)).background(fill).padding(horizontal = 14.dp, vertical = 9.dp),
        style = Type.subheadline,
        color = Palette.text,
    )

@Composable
private fun CodePage(state: SignInState, code: String, onCodeChange: (String) -> Unit, onBack: () -> Unit, onSubmit: () -> Unit) {
    val working = state == SignInState.Working
    val ready = !working && SignInCode.parse(code) != null
    val failure = (state as? SignInState.Failed)?.message

    Column(Modifier.fillMaxSize()) {
        ScreenHeader(leading = { if (!working) BackButton(onBack) })
        Column(
            Modifier.weight(1f).verticalScroll(rememberScrollState()).padding(horizontal = 24.dp).padding(bottom = 16.dp),
            verticalArrangement = Arrangement.spacedBy(24.dp),
        ) {
            Box(Modifier.fillMaxWidth().padding(top = 8.dp), contentAlignment = Alignment.Center) {
                Badge("Renoa", if (working) AgentState.Working else AgentState.Resting, 72.dp, primary = true, look = null, label = "")
            }
            Column(verticalArrangement = Arrangement.spacedBy(10.dp)) {
                Text("Sign in to Renoa", style = PAGE_TITLE, color = Palette.text)
                Text("Enter the code from your Renoa. Your passkey does the rest.", style = Type.body, color = Palette.secondary)
            }
            Box(
                Modifier
                    .fillMaxWidth()
                    .clip(RoundedRectangle(12.dp))
                    .background(Palette.surface)
                    .padding(horizontal = 14.dp, vertical = 12.dp),
            ) {
                PlainTextField(
                    text = code,
                    onChange = onCodeChange,
                    modifier = Modifier.fillMaxWidth().semantics { contentDescription = "Sign-in code" },
                    prompt = "Sign-in code",
                    singleLine = true,
                    style = Type.callout.monospaced,
                    enabled = !working,
                    keyboard = KeyboardOptions(
                        capitalization = KeyboardCapitalization.None,
                        autoCorrectEnabled = false,
                        keyboardType = KeyboardType.Ascii,
                        imeAction = ImeAction.Go,
                    ),
                    actions = KeyboardActions(onGo = { if (ready) onSubmit() }),
                )
            }
            AnimatedVisibility(failure != null, enter = fadeIn(Motion.fade()), exit = fadeOut(Motion.fade())) {
                Text(failure.orEmpty(), style = Type.footnote, color = Palette.danger)
            }
            Row(horizontalArrangement = Arrangement.spacedBy(6.dp)) {
                Symbol("lock.fill", Modifier.padding(top = 1.dp), size = 16.dp, tint = Palette.tertiary)
                Text("A setup code works once, for 30 minutes, with your passkey.", style = Type.footnote, color = Palette.tertiary)
            }
        }
        Box(Modifier.padding(horizontal = 24.dp).padding(bottom = 12.dp)) {
            WideButton("Continue with passkey", enabled = ready, busy = working, action = onSubmit)
        }
    }
}

private val PAGE_TITLE = Type.system(30f, FontWeight.SemiBold).copy(letterSpacing = (-0.5).sp)

@Composable
private fun Modifier.rise(shown: Boolean): Modifier {
    val progress by animateFloatAsState(if (shown) 1f else 0f, Motion.spring(0.6f, 0.25f), label = "Rise")
    val lift = with(LocalDensity.current) { 14.dp.toPx() }
    return graphicsLayer {
        alpha = progress.coerceIn(0f, 1f)
        translationY = (1f - progress) * lift
    }
}
