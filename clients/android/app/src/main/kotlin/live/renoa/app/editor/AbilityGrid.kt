package live.renoa.app.editor

import androidx.compose.foundation.Canvas
import androidx.compose.foundation.clickable
import androidx.compose.foundation.horizontalScroll
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.height
import androidx.compose.foundation.layout.offset
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.layout.widthIn
import androidx.compose.foundation.layout.width
import androidx.compose.foundation.rememberScrollState
import androidx.compose.material3.Text
import androidx.compose.runtime.Composable
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.geometry.Offset
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.graphics.Path
import androidx.compose.ui.graphics.StrokeCap
import androidx.compose.ui.graphics.StrokeJoin
import androidx.compose.ui.graphics.drawscope.DrawScope
import androidx.compose.ui.graphics.drawscope.Stroke
import androidx.compose.ui.graphics.drawscope.withTransform
import androidx.compose.ui.graphics.luminance
import androidx.compose.ui.semantics.Role
import androidx.compose.ui.semantics.role
import androidx.compose.ui.semantics.semantics
import androidx.compose.ui.semantics.stateDescription
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.text.style.TextAlign
import androidx.compose.ui.unit.dp
import androidx.compose.ui.unit.sp
import live.renoa.app.agents.AbilityKind
import live.renoa.app.agents.AbilityMarks
import live.renoa.app.agents.AbilitySwitch
import live.renoa.app.badge.hexPath
import live.renoa.app.theme.LocalMotionClock
import live.renoa.app.theme.LocalReducedMotion
import live.renoa.app.theme.Renoa
import live.renoa.app.theme.RenoaType
import live.renoa.app.ui.rememberSvgPath
import kotlin.math.max
import kotlin.math.min

/** Where tile [index] sits in the grid, in dp from the grid's top-left. Tiles fill columns of three; every other column hangs half a tile lower. */
data class TileSpot(val column: Int, val row: Int, val x: Float, val y: Float)

object AbilityLayout {
    const val ROWS = 3
    const val COLUMN_STEP = 58f
    const val ROW_STEP = 66f
    const val TILE = 72f

    fun spot(index: Int): TileSpot {
        val column = index / ROWS
        val row = index % ROWS
        return TileSpot(column, row, column * COLUMN_STEP, row * ROW_STEP + if (column % 2 == 1) ROW_STEP / 2 else 0f)
    }

    fun columns(count: Int): Int = (count + ROWS - 1) / ROWS

    fun width(count: Int): Float = (max(columns(count), 1) - 1) * COLUMN_STEP + TILE

    const val HEIGHT = ROWS * ROW_STEP + ROW_STEP / 2 + 6
}

/** Ink that reads on [fill]: white on a deep tone, near-black on a pale one. */
fun inkOn(fill: Color): Color = if (fill.luminance() > 0.5f) Color(0xFF1A1204) else Color.White

@Composable
fun AbilityGrid(
    abilities: List<AbilitySwitch>,
    creating: Boolean,
    tone: Color,
    onToggle: (String) -> Unit,
    modifier: Modifier = Modifier,
) {
    Box(modifier.horizontalScroll(rememberScrollState())) {
        Box(Modifier.width(AbilityLayout.width(abilities.size).dp).height(AbilityLayout.HEIGHT.dp)) {
            abilities.forEachIndexed { i, ability ->
                val spot = AbilityLayout.spot(i)
                Tile(ability, creating, tone, { onToggle(ability.id) }, Modifier.offset(spot.x.dp, spot.y.dp))
            }
        }
    }
}

@Composable
private fun Tile(ability: AbilitySwitch, creating: Boolean, tone: Color, onClick: () -> Unit, modifier: Modifier) {
    val colors = Renoa.colors
    val lit = ability.lit
    val fill = if (lit) tone else colors.card
    val ink = if (lit) inkOn(tone) else colors.foreground
    val dim =
        when {
            ability.kind == AbilityKind.Soon -> 0.4f
            !lit && !ability.canSwitch(creating) -> 0.85f
            else -> 1f
        }
    val icon = rememberSvgPath(ability.icon.path)
    val spark = rememberSvgPath(AbilityMarks.SPARK)
    val lock = rememberSvgPath(AbilityMarks.LOCK)
    val clock = LocalMotionClock.current
    val reduced = LocalReducedMotion.current
    val showsLock = ability.kind != AbilityKind.Everyday

    Box(
        modifier
            .size(AbilityLayout.TILE.dp)
            .clickable(role = Role.Switch, onClickLabel = if (lit) "Switch ${ability.title} off" else "Switch ${ability.title} on", onClick = onClick)
            .semantics { role = Role.Switch; stateDescription = if (lit) "On" else "Off" },
        contentAlignment = Alignment.Center,
    ) {
        Canvas(Modifier.size(AbilityLayout.TILE.dp)) {
            val c = Offset(size.width / 2, size.height / 2)
            if (ability.addedByAgent && !reduced) drawRipples(c, tone, clock.value)
            drawPath(hexPath(c.x, c.y, 33.dp.toPx(), 5.dp.toPx()), fill.copy(alpha = fill.alpha * dim))
            drawPath(hexPath(c.x, c.y, 33.dp.toPx(), 5.dp.toPx()), (if (lit) tone else colors.border).copy(alpha = dim), style = Stroke(1.2.dp.toPx()))
            val unit = 1.dp.toPx()
            val s = 0.8f * unit
            withTransform({ translate(c.x - 9.6f * unit, c.y - 22 * unit); scale(s, s, Offset.Zero) }) {
                drawPath(icon, ink.copy(alpha = dim), style = Stroke(1.8f, cap = StrokeCap.Round, join = StrokeJoin.Round))
            }
            if (ability.addedByAgent) drawMark(Offset(c.x + 22.dp.toPx(), c.y - 22.dp.toPx()), tone, Color.White, spark, colors.background)
            if (showsLock) drawMark(Offset(c.x + 22.dp.toPx(), c.y - 22.dp.toPx()), colors.foreground, colors.background, lock, colors.background)
        }
        Text(
            ability.title,
            Modifier.padding(top = 30.dp).widthIn(max = 56.dp),
            style = RenoaType.Caption.copy(fontSize = 10.sp, lineHeight = 12.sp, fontWeight = FontWeight.SemiBold),
            color = ink.copy(alpha = dim),
            textAlign = TextAlign.Center,
            maxLines = 1,
        )
    }
}

private fun DrawScope.drawMark(at: Offset, fill: Color, ink: Color, glyph: Path, ring: Color) {
    val unit = 1.dp.toPx()
    drawCircle(fill, 10 * unit, at)
    drawCircle(ring, 10 * unit, at, style = Stroke(2 * unit))
    val s = 0.5f * unit
    withTransform({ translate(at.x - 6 * unit, at.y - 6 * unit); scale(s, s, Offset.Zero) }) {
        drawPath(glyph, ink, style = Stroke(2.6f, cap = StrokeCap.Round, join = StrokeJoin.Round))
    }
}

/** Two rings that spread out from a tile the agent added itself, then rest. */
private fun DrawScope.drawRipples(centre: Offset, tone: Color, seconds: Float) {
    val period = 4.2f
    for (delay in listOf(0f, 0.18f)) {
        val u = (((seconds - delay) % period + period) % period) / period
        val alpha = when {
            u < 0.36f -> 0f
            u < 0.40f -> 0.8f * (u - 0.36f) / 0.04f
            u < 0.78f -> 0.8f * (1f - (u - 0.40f) / 0.38f)
            else -> 0f
        }
        val grow = min(1f, max(0f, u / 0.78f))
        if (alpha > 0f) drawCircle(tone.copy(alpha = alpha), 34.dp.toPx() * (0.8f + 0.9f * grow), centre, style = Stroke(1.5.dp.toPx()))
    }
}
