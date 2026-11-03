package live.renoa.app.badge

import androidx.compose.animation.core.Easing
import live.renoa.app.agents.AgentState
import live.renoa.app.theme.RenoaEasing
import kotlin.math.abs

/**
 * One instant of an agent's tile, in tile units (the face has radius [BadgeFrame.RADIUS]).
 * Everything the canvas needs comes from here, so the motion can be checked without a screen.
 */
data class BadgeFrame(
    /** How far the tile has lifted off its socket; 0 sits on it, 1 is one full lift. */
    val lift: Float = 0f,
    /** A ring leaving the tile, 0 to 1 through its life, or null when none is out. */
    val ring: Float? = null,
    /** How big the question bubble of an agent that wants the owner is, or null when it asks nothing. */
    val bubble: Float? = null,
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
    private const val BUBBLE_SECONDS = 1.4f
    private const val ASLEEP_ALPHA = 0.55f

    /** Whether a tile in [state] changes over time at all. A still tile needs no frame loop. */
    fun animates(state: AgentState, reducedMotion: Boolean): Boolean =
        !reducedMotion && (state == AgentState.Working || state == AgentState.Wants)

    /** The tile in [state] at [seconds] on the shared clock. With [reducedMotion] nothing loops, and each state keeps a still cue. */
    fun frame(state: AgentState, seconds: Float, reducedMotion: Boolean): BadgeFrame =
        when (state) {
            AgentState.Resting -> BadgeFrame()
            // Held still, a ring stays partway out.
            AgentState.Working -> BadgeFrame(ring = if (reducedMotion) 0.35f else (seconds % RING_SECONDS) / RING_SECONDS)
            AgentState.Wants ->
                if (reducedMotion) {
                    BadgeFrame(lift = 0.8f, bubble = 1f)
                } else {
                    BadgeFrame(lift = swing(seconds, HOVER_SECONDS, 0.65f, 0.95f), bubble = swing(seconds, BUBBLE_SECONDS, 1f, 1.15f))
                }
            AgentState.Stuck -> BadgeFrame(failed = true)
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
