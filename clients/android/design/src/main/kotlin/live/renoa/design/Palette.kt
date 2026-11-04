// Copyright 2026 Po Kai Lee and the Codync contributors.
// Licensed under the Apache License, Version 2.0; see licenses/Apache-2.0-Codync.txt.
// Adapted from Codync apps/android/design at ed8daf11f89398e598ef1f791abc3014ded68496.
package live.renoa.design

import androidx.compose.runtime.Composable
import androidx.compose.runtime.ReadOnlyComposable
import androidx.compose.runtime.staticCompositionLocalOf
import androidx.compose.ui.graphics.Color

/** Whether the dark half of each color pair applies; `DesignTheme` sets it from the system. */
val LocalDark = staticCompositionLocalOf { false }

private fun hex(value: Long) = Color(0xFF000000 or value)

/** One mode's colors, readable outside composition (a WebView's stylesheet, a test). */
class Tones private constructor(private val dark: Boolean) {
    private fun pair(light: Long, night: Long) = hex(if (dark) night else light)

    val background: Color = pair(0xFFFFFF, 0x0A0A0A)
    val surface: Color = pair(0xF4F4F4, 0x141414)
    val bubbleAgent: Color = pair(0xF0F0F0, 0x1C1C1C)
    val bubbleUser: Color = pair(0xE2E2E2, 0x363636)
    val border: Color = pair(0xE6E6E6, 0x262626)
    val text: Color = pair(0x141414, 0xF2F2F2)
    val secondary: Color = pair(0x6B6B6B, 0x9A9A9A)
    val tertiary: Color = pair(0x767676, 0x929292)
    /** Ink used for fills (primary buttons, the send button, unread badges). */
    val accentFill: Color = pair(0x000000, 0xFFFFFF)
    /** Ink readable as text and tint on the background. */
    val accent: Color = pair(0x000000, 0xFFFFFF)
    val onAccent: Color = pair(0xFFFFFF, 0x000000)
    val accentDim: Color = pair(0xD9D9D9, 0x333333)
    val danger: Color = pair(0xC23A2B, 0xF0A7A7)
    /** A bot that needs you, in widgets and task cards: readable amber in both modes. */
    val attention: Color = pair(0x936000, 0xECAF52)
    val codeBackground: Color = pair(0xF4F4F4, 0x111111)
    val added: Color = pair(0x2E7D32, 0x8FD18B)
    val removed: Color = pair(0xC62828, 0xF0A7A7)

    companion object {
        val Light = Tones(false)
        val Dark = Tones(true)
    }
}

/**
 * Black and white: a pure black (or white) ground, grey bubbles, and color that comes only
 * from the bots; amber marks "needs you" and red marks errors.
 */
object Palette {
    private val tones: Tones @Composable @ReadOnlyComposable get() = if (LocalDark.current) Tones.Dark else Tones.Light

    val background: Color @Composable @ReadOnlyComposable get() = tones.background
    val surface: Color @Composable @ReadOnlyComposable get() = tones.surface
    val bubbleAgent: Color @Composable @ReadOnlyComposable get() = tones.bubbleAgent
    val bubbleUser: Color @Composable @ReadOnlyComposable get() = tones.bubbleUser
    val border: Color @Composable @ReadOnlyComposable get() = tones.border
    val text: Color @Composable @ReadOnlyComposable get() = tones.text
    val secondary: Color @Composable @ReadOnlyComposable get() = tones.secondary
    val tertiary: Color @Composable @ReadOnlyComposable get() = tones.tertiary
    /** Ink used for fills (primary buttons, the send button, unread badges). */
    val accentFill: Color @Composable @ReadOnlyComposable get() = tones.accentFill
    /** Ink readable as text and tint on the background. */
    val accent: Color @Composable @ReadOnlyComposable get() = tones.accent
    val onAccent: Color @Composable @ReadOnlyComposable get() = tones.onAccent
    val accentDim: Color @Composable @ReadOnlyComposable get() = tones.accentDim
    val danger: Color @Composable @ReadOnlyComposable get() = tones.danger
    /** A bot that needs you, in widgets and task cards: readable amber in both modes. */
    val attention: Color @Composable @ReadOnlyComposable get() = tones.attention
    val codeBackground: Color @Composable @ReadOnlyComposable get() = tones.codeBackground
    val added: Color @Composable @ReadOnlyComposable get() = tones.added
    val removed: Color @Composable @ReadOnlyComposable get() = tones.removed
    val warning: Color get() = hex(0xF0A030)
}
