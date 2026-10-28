package live.renoa.app.ui

import androidx.compose.foundation.background
import androidx.compose.foundation.border
import androidx.compose.foundation.clickable
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.height
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.material3.Text
import androidx.compose.runtime.Composable
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.draw.alpha
import androidx.compose.ui.draw.clip
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.semantics.Role
import androidx.compose.ui.unit.dp
import live.renoa.app.theme.Renoa
import live.renoa.app.theme.RenoaShapes
import live.renoa.app.theme.RenoaType

/** The design system's button kinds (components/Button). */
enum class ButtonKind {
    /** Solid ink: the one main action in a view. */
    Primary,

    /** Card fill with an `input` edge, beside a primary. */
    Secondary,

    /** Text only: cancel, dismiss, and actions inside dense rows. */
    Ghost,

    /** `state-failed` text and edge: remove, delete, disconnect. */
    Destructive,
}

/** `md` is 40 tall and the default; `sm` is 32, for rows and toolbars. */
enum class ButtonSize(val height: Int, val padding: Int) { Md(40, 16), Sm(32, 12) }

/**
 * A Renoa button. Its label says exactly what happens. Disabled drops to 45% and stops taking
 * taps, as the design system sets it.
 */
@Composable
fun RenoaButton(
    label: String,
    onClick: () -> Unit,
    modifier: Modifier = Modifier,
    kind: ButtonKind = ButtonKind.Primary,
    size: ButtonSize = ButtonSize.Md,
    enabled: Boolean = true,
    icon: String? = null,
) {
    val colors = Renoa.colors
    val shape = RoundedCornerShape(RenoaShapes.Control)
    val (fill, ink, edge) =
        when (kind) {
            ButtonKind.Primary -> Triple(colors.primary, colors.primaryForeground, Color.Transparent)
            ButtonKind.Secondary -> Triple(colors.card, colors.foreground, colors.input)
            ButtonKind.Ghost -> Triple(Color.Transparent, colors.foreground, Color.Transparent)
            ButtonKind.Destructive -> Triple(Color.Transparent, colors.stateFailed, colors.stateFailed)
        }
    Row(
        modifier
            .height(size.height.dp)
            .alpha(if (enabled) 1f else 0.45f)
            .clip(shape)
            .background(fill)
            .border(1.dp, edge, shape)
            .clickable(enabled = enabled, role = Role.Button, onClick = onClick)
            .padding(horizontal = size.padding.dp),
        horizontalArrangement = Arrangement.spacedBy(8.dp, Alignment.CenterHorizontally),
        verticalAlignment = Alignment.CenterVertically,
    ) {
        if (icon != null) PathIcon(icon, ink, size = 16.dp)
        Text(label, style = RenoaType.Label, color = ink)
    }
}
