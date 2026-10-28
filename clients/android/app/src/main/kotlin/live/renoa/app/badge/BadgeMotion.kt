package live.renoa.app.badge

import androidx.compose.animation.core.Easing
import live.renoa.app.agents.AgentState
import live.renoa.app.theme.RenoaEasing
import kotlin.math.abs

/** The dot that rides on a badge. */
enum class Dot {
    /** Something wants the owner. */
    Calling,

    /** Something broke. */
    Failed,

    /** A small badge is busy. */
    Running,
}

data class OrbFrame(val alpha: Float, val scale: Float)

/**
 * One instant of a badge, in the badge's own units (the hex core has radius 36). Everything the
 * canvas needs to draw a state comes from here, so the motion can be checked without a screen.
 */
data class BadgeFrame(
    val coreScale: Float = 1f,
    val orbs: List<OrbFrame> = List(ORBS) { OrbFrame(1f, 1f) },
    val glowAlpha: Float = 0f,
    /** Vertical shift of the whole badge; negative is up. */
    val lift: Float = 0f,
    val shadowAlpha: Float = 0f,
    val dot: Dot? = null,
    val dotScale: Float = 1f,
    val grayscale: Boolean = false,
    val brightness: Float = 1f,
    val alpha: Float = 1f,
) {
    companion object {
        const val ORBS = 6
    }
}

object BadgeMotion {
    private const val BREATHE_SECONDS = 6f
    private const val BLINK_SECONDS = 1.8f
    private const val BLINK_STAGGER = 0.3f
    private const val HOVER_SECONDS = 3.2f
    private const val DOT_SECONDS = 1.4f
    private const val ASLEEP_ALPHA = 0.38f
    private const val STUCK_BRIGHTNESS = 0.8f
    private const val STILL_GLOW = 0.45f
    private const val STILL_LIFT = -11f

    /** Whether a badge in [state] changes over time at all. A still badge needs no frame loop. */
    fun animates(state: AgentState, reducedMotion: Boolean): Boolean =
        !reducedMotion &&
            when (state) {
                AgentState.Resting, AgentState.Working, AgentState.Wants -> true
                AgentState.Stuck, AgentState.Asleep -> false
            }

    /**
     * The badge in [state] at [seconds] on the shared clock. A small ([mini]) badge has no orbs and
     * says "working" with one pulsing dot. With [reducedMotion] nothing loops: the frame is the
     * same at every time, and working and wants-you keep a still cue so the states stay apart.
     */
    fun frame(state: AgentState, mini: Boolean, seconds: Float, reducedMotion: Boolean): BadgeFrame =
        (if (reducedMotion) still(state) else moving(state, mini, seconds)).let { if (mini) it.copy(orbs = emptyList()) else it }

    private fun moving(state: AgentState, mini: Boolean, t: Float): BadgeFrame =
        when (state) {
            AgentState.Resting -> BadgeFrame(coreScale = swing(t, BREATHE_SECONDS, 1f, 1.03f))
            AgentState.Working ->
                BadgeFrame(
                    orbs = List(BadgeFrame.ORBS) { blink(t - it * BLINK_STAGGER) },
                    glowAlpha = swing(t, BLINK_SECONDS, 0.25f, 0.6f),
                    dot = if (mini) Dot.Running else null,
                    dotScale = swing(t, DOT_SECONDS, 1f, 1.35f),
                )
            AgentState.Wants ->
                BadgeFrame(lift = swing(t, HOVER_SECONDS, -9f, -13f), shadowAlpha = 0.5f, dot = Dot.Calling)
            AgentState.Stuck, AgentState.Asleep -> still(state)
        }

    private fun still(state: AgentState): BadgeFrame =
        when (state) {
            AgentState.Resting -> BadgeFrame()
            AgentState.Working -> BadgeFrame(glowAlpha = STILL_GLOW, dot = Dot.Running)
            AgentState.Wants -> BadgeFrame(lift = STILL_LIFT, shadowAlpha = 0.5f, dot = Dot.Calling)
            AgentState.Stuck -> BadgeFrame(grayscale = true, brightness = STUCK_BRIGHTNESS, dot = Dot.Failed)
            AgentState.Asleep -> BadgeFrame(grayscale = true, alpha = ASLEEP_ALPHA)
        }

    /** One tile lighting up and settling. Before its delay has passed a tile rests at full size. */
    private fun blink(local: Float): OrbFrame {
        if (local < 0f) return OrbFrame(1f, 1f)
        val u = (local % BLINK_SECONDS) / BLINK_SECONDS
        val ease = RenoaEasing.Ease
        return when {
            u < 0.15f -> {
                val e = ease.transform(u / 0.15f)
                OrbFrame(lerp(0.35f, 1f, e), lerp(0.88f, 1.14f, e))
            }
            u < 0.45f -> {
                val e = ease.transform((u - 0.15f) / 0.30f)
                OrbFrame(lerp(1f, 0.35f, e), lerp(1.14f, 0.88f, e))
            }
            else -> OrbFrame(0.35f, 0.88f)
        }
    }

    /** Eases from [low] to [high] and back once per [period]. */
    private fun swing(t: Float, period: Float, low: Float, high: Float, easing: Easing = RenoaEasing.EaseInOut): Float {
        val u = (t % period) / period
        val half = 1f - abs(2f * u - 1f)
        return lerp(low, high, easing.transform(half))
    }

    private fun lerp(a: Float, b: Float, f: Float) = a + (b - a) * f
}
