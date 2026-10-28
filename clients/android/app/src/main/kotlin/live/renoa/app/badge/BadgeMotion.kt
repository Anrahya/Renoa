package live.renoa.app.badge

import androidx.compose.animation.core.Easing
import live.renoa.app.agents.AgentState
import live.renoa.app.theme.RenoaEasing
import kotlin.math.abs

/** The round status dot. Hex means structure and a circle means live state, so state is only ever a dot. */
enum class Dot {
    /** The agent needs the owner: ink, the design system's `state-needs-you`. */
    Calling,

    /** Something broke: `state-failed`. */
    Failed,

    /** Work in progress: `state-running`. */
    Running,
}

/**
 * One instant of an agent's tile, in tile units (the face has radius [BadgeFrame.RADIUS]).
 * Everything the canvas needs comes from here, so the motion can be checked without a screen.
 */
data class BadgeFrame(
    /** How far the tile has lifted off its socket; 0 sits on it, 1 is one full lift. */
    val lift: Float = 0f,
    /** A ring leaving the tile, 0 to 1 through its life, or null when none is out. */
    val ring: Float? = null,
    val dot: Dot? = null,
    val dotScale: Float = 1f,
    /** A retiring tile: a muted face with no body, as the landing page draws an old part. */
    val retiring: Boolean = false,
    /** A broken tile: its edge turns `state-failed`. */
    val failed: Boolean = false,
    val alpha: Float = 1f,
) {
    companion object {
        const val RADIUS = 36f

        /** `tile-depth` (4px at a 60px tile), the body showing below the face. */
        const val DEPTH = 3.2f

        /** `tile-lift` (18px at a 60px tile). */
        const val LIFT = 10.8f
    }
}

/**
 * How a tile moves in each state, after the landing page: resting sits still on its socket,
 * working sends out a ring the way a tile pings when a packet reaches it, wants-you lifts off its
 * socket the way a tile waiting for approval does, stuck turns its edge red, and asleep retires.
 */
object BadgeMotion {
    const val RING_SECONDS = 1.6f
    private const val HOVER_SECONDS = 3.2f
    private const val DOT_SECONDS = 1.4f
    private const val ASLEEP_ALPHA = 0.55f

    /** Whether a tile in [state] changes over time at all. A still tile needs no frame loop. */
    fun animates(state: AgentState, reducedMotion: Boolean): Boolean =
        !reducedMotion && (state == AgentState.Working || state == AgentState.Wants)

    /** The tile in [state] at [seconds] on the shared clock. With [reducedMotion] nothing loops, and each state keeps a still cue. */
    fun frame(state: AgentState, seconds: Float, reducedMotion: Boolean): BadgeFrame =
        when (state) {
            AgentState.Resting -> BadgeFrame()
            AgentState.Working ->
                if (reducedMotion) BadgeFrame(dot = Dot.Running) else BadgeFrame(ring = (seconds % RING_SECONDS) / RING_SECONDS, dot = Dot.Running)
            AgentState.Wants ->
                if (reducedMotion) {
                    BadgeFrame(lift = 0.8f, dot = Dot.Calling)
                } else {
                    BadgeFrame(lift = swing(seconds, HOVER_SECONDS, 0.65f, 0.95f), dot = Dot.Calling, dotScale = swing(seconds, DOT_SECONDS, 1f, 1.3f))
                }
            AgentState.Stuck -> BadgeFrame(dot = Dot.Failed, failed = true)
            AgentState.Asleep -> BadgeFrame(retiring = true, alpha = ASLEEP_ALPHA)
        }

    /** Eases from [low] to [high] and back once per [period]. */
    private fun swing(t: Float, period: Float, low: Float, high: Float, easing: Easing = RenoaEasing.EaseInOut): Float {
        val u = (t % period) / period
        val half = 1f - abs(2f * u - 1f)
        return low + (high - low) * easing.transform(half)
    }
}

/** Where the primary agent's light is on its way round the tile, 0 to 1, at [seconds]. */
fun primarySweep(seconds: Float, reducedMotion: Boolean): Float? = if (reducedMotion) null else (seconds % PRIMARY_SWEEP_SECONDS) / PRIMARY_SWEEP_SECONDS

private const val PRIMARY_SWEEP_SECONDS = 4f

/** The letter on an agent's tile: the first letter or digit of its name, upper-cased, or "?" for none. */
fun tileLetter(name: String): Char = name.firstOrNull { it.isLetterOrDigit() }?.uppercaseChar() ?: '?'
