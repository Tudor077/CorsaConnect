package com.corsaconnect

import androidx.compose.foundation.Canvas
import androidx.compose.foundation.gestures.detectTapGestures
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.runtime.Composable
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.setValue
import androidx.compose.ui.Modifier
import androidx.compose.ui.input.pointer.pointerInput
import kotlin.math.abs
import kotlin.math.roundToInt

/**
 * The PicoPanel's own screen, on the phone.
 *
 * Same pages, same order, same coordinates: this is `drawCarPage()` from
 * PicoPanel.ino, line for line, in the panel's 128x32 grid, with the panel's
 * own font. It is drawn rather than mirrored because the phone has no link to
 * the panel - it has the same telemetry, so it can produce the same picture
 * itself at whatever size the element is dragged to.
 *
 * The header is the one thing left out, as asked: a page name and a counter are
 * how the panel says which of its nine pages you are on, and here there are
 * three. The content simply starts at the row where the header ended.
 *
 * Tap to change page, the way the USER button does on the panel.
 */

private const val PW = 128       // the panel, in its own pixels
private const val PH = 32
/** The first row below the header. Everything is drawn from here down. */
private const val HEADER = 9
const val PANEL_ROWS = PH - HEADER

@Composable
fun PanelScreen(t: Protocol.Telemetry, enabled: Boolean) {
    var page by remember { mutableStateOf(0) }
    Canvas(
        Modifier
            .fillMaxSize()
            .then(
                if (enabled) Modifier.pointerInput(Unit) {
                    detectTapGestures { page = (page + 1) % 3 }
                } else Modifier,
            ),
    ) {
        drawRect(PIXEL_BG)
        // Whichever side runs out first decides, so dragging the element out of
        // proportion shrinks the picture instead of cutting the rev bar off.
        val cell = minOf(size.width / PW, size.height / PANEL_ROWS)
        Pix(this, cell, PW, PANEL_ROWS).carPage(page, t)
    }
}

/** Draw at the panel's own y, with the header band taken out. */
private fun Pix.at(s: String, x: Int, y: Int, size: Int) = text(s, x, y - HEADER, size)

private fun Pix.carPage(page: Int, t: Protocol.Telemetry) {
    // Our gear is 0 = reverse, 1 = neutral, 2 = first. The panel counts from
    // neutral, with reverse below it.
    val gear = t.gear - 1
    val rpmMax = if (t.maxRpm > 100f) t.maxRpm.roundToInt() else 0
    val redline = if (t.redline > 100f) t.redline.roundToInt() else 0

    when (page) {
        0 -> {
            blinkArrow(true, (t.showLights and 32) != 0)
            blinkArrow(false, (t.showLights and 64) != 0)

            at(t.speedKmh.roundToInt().toString(), 11, 11, 2)
            at("km/h", 11 + 36, 18, 1)

            when {
                gear < 0 -> at("R", 98, 11, 2)
                gear == 0 -> at("N", 98, 11, 2)
                gear < 10 -> at(gear.toString(), 98, 11, 2)
                else -> at(gear.toString(), 98, 15, 1)
            }
            rpmBar(26, t.rpm.roundToInt(), rpmMax, redline)
        }
        1 -> {
            val fuel = (t.fuel * 100f).roundToInt()
            val temp = t.engineTemp.roundToInt()
            val turbo = (t.turbo * 10f).roundToInt()
            at("FUEL " + if (t.fuel >= 0f) "$fuel%" else "?", 0, 11, 1)
            at("TEMP " + if (temp != 0) "${temp}C" else "?", 68, 11, 1)
            at("TURBO ${turbo / 10}.${abs(turbo % 10)}b", 0, 21, 1)
            at("RPM ${t.rpm.roundToInt()}", 68, 21, 1)
        }
        else -> {
            at("THR ${(t.throttle * 100f).roundToInt()}%", 0, 11, 1)
            at("BRAKE ${(t.brake * 100f).roundToInt()}%", 56, 11, 1)
            if (redline > 0) at("REDLINE $redline", 0, 21, 1)
            else at("redline not learned", 0, 21, 1)
        }
    }
}

/** Only drawn while lit: it blinks in the game, so it blinks here by itself. */
private fun Pix.blinkArrow(left: Boolean, on: Boolean) {
    if (!on) return
    if (left) {
        triangle(0, 17 - HEADER, 8, 11 - HEADER, 8, 23 - HEADER)
    } else {
        triangle(PW - 1, 17 - HEADER, PW - 9, 11 - HEADER, PW - 9, 23 - HEADER)
    }
}

private fun Pix.rpmBar(y: Int, rpm: Int, rpmMax: Int, redline: Int) {
    val bh = 6
    if (rpmMax <= 0) return
    val top = y - HEADER
    if (redline > 0 && rpm >= redline && (System.currentTimeMillis() % 300) < 150) {
        rect(0, top, PW, bh)              // solid bar: shift now
        return
    }
    val w = (rpm.toLong() * PW / rpmMax).toInt().coerceIn(0, PW)
    frame(0, top, PW, bh)
    ditherRamp(1, top + 1, if (w > 2) w - 2 else 0, bh - 2, 3, 16)
    if (redline in 1 until rpmMax) {
        val rx = (redline.toLong() * PW / rpmMax).toInt().coerceAtMost(PW - 1)
        rect(rx, top - 3, 1, bh + 3)
    }
}
