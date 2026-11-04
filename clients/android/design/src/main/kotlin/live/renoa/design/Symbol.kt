// Copyright 2026 Po Kai Lee and the Codync contributors.
// Licensed under the Apache License, Version 2.0; see licenses/Apache-2.0-Codync.txt.
// Adapted from Codync apps/android/design at ed8daf11f89398e598ef1f791abc3014ded68496.
package live.renoa.design

import androidx.compose.foundation.Image
import androidx.compose.foundation.layout.size
import androidx.compose.material3.LocalContentColor
import androidx.compose.runtime.Composable
import androidx.compose.ui.Modifier
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.graphics.ColorFilter
import androidx.compose.ui.res.painterResource
import androidx.compose.ui.semantics.clearAndSetSemantics
import androidx.compose.ui.semantics.contentDescription
import androidx.compose.ui.semantics.semantics
import androidx.compose.ui.unit.Dp
import androidx.compose.ui.unit.dp

/**
 * `Image(systemName:)`: the stand-in for an SF Symbol, tinted with the content color.
 * `size` is the glyph's box; a symbol set at font size F looks right at about F × 1.2.
 */
@Composable fun Symbol(name: String, modifier: Modifier = Modifier, size: Dp = 20.dp, tint: Color = LocalContentColor.current,
    contentDescription: String? = null, height: Dp = size) {
    val painter = when (name) {
        "bubble.left.and.bubble.right.fill" -> Bubbles
        "rectangle.stack.fill" -> CardStack
        "hexagon.fill" -> Hexagon
        else -> painterResource(symbolRes(name) ?: R.drawable.sf_circle_dotted)
    }
    val described = if (contentDescription == null) modifier.clearAndSetSemantics {}
        else modifier.semantics { this.contentDescription = contentDescription }
    Image(painter, null, described.size(size, height), colorFilter = ColorFilter.tint(tint))
}
