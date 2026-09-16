package com.corsaconnect

import android.graphics.Paint
import android.graphics.Typeface
import androidx.compose.foundation.Canvas
import androidx.compose.foundation.gestures.detectTapGestures
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.runtime.Composable
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.setValue
import androidx.compose.ui.Modifier
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.graphics.drawscope.DrawScope
import androidx.compose.ui.graphics.nativeCanvas
import androidx.compose.ui.input.pointer.pointerInput
import kotlin.math.roundToInt

/**
 * The PicoPanel's screen, redrawn here.
 *
 * Same layout, same pages, same order - the panel's dashboard is already shaped
 * for driving, so there was no reason to invent a second one for the phone. It
 * is drawn rather than mirrored because the phone has no link to the panel: it
 * has the same telemetry, so it can produce the same picture at its own
 * resolution instead of magnifying 128x32 pixels.
 *
 * Everything below is in the panel's own pixel grid and scaled on the way out,
 * so the coordinates match `drawCarPage()` in PicoPanel.ino line for line. The
 * header is the one thing left out, as asked - the page name and the counter
 * are the panel's way of saying which of its nine pages you're on, and here
 * there are only these.
 *
 * Tap to change page, the way the USER button does on the panel.
 */

private const val VW = 128f      // the panel's width, in its own pixels
private const val VH = 32f
/** Where the content starts once the header band is gone. */
private const val TOP = 9f
/** The visible grid: what's left of the panel once the header is cut off. */
const val PANEL_ASPECT = VW / (VH - TOP)

private val LIT = Color(0xFFE6F2FF)   // an OLED's slightly blue white
private val OFF = Color(0xFF07090C)

/** Adafruit GFX threshold matrix, so a gradient dithers exactly as it does there. */
private val BAYER = intArrayOf(
    0, 8, 2, 10,
    12, 4, 14, 6,
    3, 11, 1, 9,
    15, 7, 13, 5,
)

@Composable
fun PanelScreen(t: Protocol.Telemetry, enabled: Boolean) {
    var page by remember { mutableStateOf(0) }
    Canvas(
        Modifier
            .fillMaxSize()
            .then(
                if (enabled) Modifier.pointerInput(Unit) {
                    detectTapGestures { page = (page + 1) % 3 }
                } else Modifier
            )
    ) {
        drawRect(OFF)
        // One panel pixel, in real ones. Whichever side runs out first decides,
        // so dragging the element out of proportion shrinks the picture instead
        // of cutting the rev bar off the bottom.
        val s = minOf(size.width / VW, size.height / (VH - TOP))
        drawCarPage(page, t, s)
    }
}

/** Text the way Adafruit GFX places it: (x, y) is the top-left of the glyph box. */
private class PanelText(private val s: Float) {
    private val paint = Paint().apply {
        isAntiAlias = true
        typeface = Typeface.create(Typeface.MONOSPACE, Typeface.BOLD)
        color = android.graphics.Color.rgb(230, 242, 255)
    }

    /** `size` is GFX's text size: 1 is a 6x8 cell, 2 is 12x16. */
    fun draw(canvas: android.graphics.Canvas, str: String, x: Float, y: Float, size: Int) {
        // GFX advances 6 pixels per character per size step. Monospace scales
        // linearly, so one measurement gives the size that matches exactly.
        paint.textSize = 100f
        val advance = paint.measureText("0")
        paint.textSize = 100f * (6f * size * s) / advance
        // The glyph box is 7 pixels tall per size step and sits on its bottom.
        canvas.drawText(str, x * s, (y + 7f * size - TOP) * s, paint)
    }
}

private fun DrawScope.px(x: Float, y: Float, w: Float, h: Float, s: Float) =
    drawRect(LIT, topLeft = androidx.compose.ui.geometry.Offset(x * s, (y - TOP) * s),
        size = androidx.compose.ui.geometry.Size(w * s, h * s))

/** A hollow rectangle, one panel pixel thick, like GFX's drawRect. */
private fun DrawScope.frame(x: Float, y: Float, w: Float, h: Float, s: Float) {
    px(x, y, w, 1f, s)
    px(x, y + h - 1, w, 1f, s)
    px(x, y, 1f, h, s)
    px(x + w - 1, y, 1f, h, s)
}

/**
 * Density rising left to right, one panel pixel at a time. The dither is the
 * point: it is what the bar looks like on a screen that has no grey, and
 * smoothing it into a gradient here would make the two displays disagree.
 */
private fun DrawScope.ditherRamp(
    x: Int, y: Int, w: Int, h: Int, from: Int, to: Int, s: Float,
) {
    if (w <= 0) return
    for (i in 0 until w) {
        val lv = from + (to - from) * i / (if (w > 1) w - 1 else 1)
        for (j in 0 until h) {
            if (BAYER[((j and 3) shl 2) or (i and 3)] < lv) {
                px((x + i).toFloat(), (y + j).toFloat(), 1f, 1f, s)
            }
        }
    }
}

/** The turn-signal arrow, drawn only while lit - it blinks in the game anyway. */
private fun DrawScope.blinkArrow(left: Boolean, on: Boolean, s: Float) {
    if (!on) return
    val path = androidx.compose.ui.graphics.Path()
    if (left) {
        path.moveTo(0f, (17f - TOP) * s)
        path.lineTo(8f * s, (11f - TOP) * s)
        path.lineTo(8f * s, (23f - TOP) * s)
    } else {
        path.moveTo(127f * s, (17f - TOP) * s)
        path.lineTo(119f * s, (11f - TOP) * s)
        path.lineTo(119f * s, (23f - TOP) * s)
    }
    path.close()
    drawPath(path, LIT)
}

private fun DrawScope.drawRpmBar(y: Int, rpm: Int, rpmMax: Int, redline: Int, s: Float) {
    val bh = 6
    if (rpmMax <= 0) return
    val over = redline > 0 && rpm >= redline
    if (over && (System.currentTimeMillis() % 300) < 150) {
        px(0f, y.toFloat(), VW, bh.toFloat(), s)      // full bar: shift now
        return
    }
    val w = (rpm.toLong() * VW.toInt() / rpmMax).toInt().coerceIn(0, VW.toInt())
    frame(0f, y.toFloat(), VW, bh.toFloat(), s)
    ditherRamp(1, y + 1, if (w > 2) w - 2 else 0, bh - 2, 3, 16, s)
    if (redline in 1 until rpmMax) {
        val rx = (redline.toLong() * VW.toInt() / rpmMax).toInt().coerceAtMost(VW.toInt() - 1)
        px(rx.toFloat(), (y - 3).toFloat(), 1f, (bh + 3).toFloat(), s)
    }
}

private fun DrawScope.drawCarPage(page: Int, t: Protocol.Telemetry, s: Float) {
    val text = PanelText(s)
    // Our gear is 0 = reverse, 1 = neutral, 2 = first. The panel counts from
    // neutral, with reverse below it.
    val gear = t.gear - 1
    val rpmMax = if (t.maxRpm > 100f) t.maxRpm.roundToInt() else 0
    val redline = if (t.redline > 100f) t.redline.roundToInt() else 0

    drawContext.canvas.nativeCanvas.let { c ->
        when (page) {
            0 -> {
                // Big speed, flanked by the turn-signal arrows.
                blinkArrow(true, (t.showLights and 32) != 0, s)
                blinkArrow(false, (t.showLights and 64) != 0, s)

                text.draw(c, t.speedKmh.roundToInt().toString(), 11f, 11f, 2)
                text.draw(c, "km/h", 11f + 36f, 18f, 1)

                when {
                    gear < 0 -> text.draw(c, "R", 98f, 11f, 2)
                    gear == 0 -> text.draw(c, "N", 98f, 11f, 2)
                    gear < 10 -> text.draw(c, gear.toString(), 98f, 11f, 2)
                    else -> text.draw(c, gear.toString(), 98f, 15f, 1)
                }
                drawRpmBar(26, t.rpm.roundToInt(), rpmMax, redline, s)
            }
            1 -> {
                val fuel = (t.fuel * 100f).roundToInt()
                val temp = t.engineTemp.roundToInt()
                val turbo = (t.turbo * 10f).roundToInt()
                text.draw(c, "FUEL " + if (t.fuel >= 0f) "$fuel%" else "?", 0f, 11f, 1)
                text.draw(c, "TEMP " + if (temp != 0) "${temp}C" else "?", 68f, 11f, 1)
                text.draw(c, "TURBO ${turbo / 10}.${turbo % 10}b", 0f, 21f, 1)
                text.draw(c, "RPM ${t.rpm.roundToInt()}", 68f, 21f, 1)
            }
            else -> {
                text.draw(c, "THR ${(t.throttle * 100f).roundToInt()}%", 0f, 11f, 1)
                text.draw(c, "BRAKE ${(t.brake * 100f).roundToInt()}%", 56f, 11f, 1)
                if (redline > 0) {
                    text.draw(c, "REDLINE $redline", 0f, 21f, 1)
                } else {
                    text.draw(c, "redline not learned", 0f, 21f, 1)
                }
            }
        }
    }
}
