package com.corsaconnect

import androidx.compose.foundation.Canvas
import androidx.compose.foundation.gestures.awaitEachGesture
import androidx.compose.foundation.gestures.awaitFirstDown
import androidx.compose.foundation.gestures.detectDragGestures
import androidx.compose.foundation.gestures.detectTapGestures
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.runtime.Composable
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.setValue
import androidx.compose.ui.Modifier
import androidx.compose.ui.geometry.Offset
import androidx.compose.ui.hapticfeedback.HapticFeedbackType
import androidx.compose.ui.input.pointer.pointerInput
import androidx.compose.ui.platform.LocalHapticFeedback
import kotlin.math.atan2
import kotlin.math.cos
import kotlin.math.hypot
import kotlin.math.min
import kotlin.math.roundToInt
import kotlin.math.sin

/**
 * Every HUD element, drawn the way the panel would draw it.
 *
 * One skin, not a set of lookalikes: each widget here is built out of the same
 * primitives and the same font as the PicoPanel's screen, on a grid shared by
 * the whole HUD, so the phone reads as one 1-bit display rather than a set of
 * shapes that happen to be square. The background is pure black for the same
 * reason an OLED is: an unlit pixel emits nothing.
 *
 * Interaction is unchanged. These are the same gestures as the colour skin,
 * with a different painter behind them.
 */

/** How big one pixel is. The whole HUD shares it, so the grid lines up. */
fun pixelCell(screenHeightPx: Float) = (screenHeightPx / 96f).coerceAtLeast(3f)

@Composable
fun PixelElement(
    el: Element,
    t: Protocol.Telemetry,
    config: Config,
    steer: Float,
    enabled: Boolean,
    cell: Float,
    wheelReset: Int,
    onSteer: (Float) -> Unit,
    onPedal: (Int) -> Unit,
    onMask: (Int, Boolean) -> Unit,
    onJoy: (Float, Float) -> Unit,
) {
    when (el.type) {
        ControlType.THROTTLE_SLIDER, ControlType.BRAKE_SLIDER, ControlType.CLUTCH_SLIDER ->
            PixelPedal(el.label, enabled, cell, onPedal)

        ControlType.GAS -> PixelHold(el.label, enabled, cell) { onPedal(if (it) 255 else 0) }
        ControlType.BRAKE -> PixelHold(el.label, enabled, cell) { onPedal(if (it) 255 else 0) }

        ControlType.BUTTON -> PixelButton(el, enabled, cell, onMask)
        ControlType.JOYSTICK -> PixelStick(el.label, el.momentary, enabled, cell, onJoy)

        ControlType.STEERING_WHEEL ->
            PixelWheel(Math.toRadians(config.maxAngleDeg.toDouble()).toFloat(),
                wheelReset, enabled, cell, onSteer)

        else -> Canvas(Modifier.fillMaxSize()) {
            val (needCols, needRows) = readoutNeed(el, t, config)
            pixels(cellFor(cell, needCols, needRows)) { readout(el, t, config, steer) }
        }
    }
}

/**
 * The smallest grid each readout can be drawn in, in pixels of that grid.
 * Anything smaller and the label would not fit at text size 1, so the element
 * takes a finer grid instead of spilling over its own frame.
 */
private fun readoutNeed(el: Element, t: Protocol.Telemetry, config: Config): Pair<Int, Int> =
    when (el.type) {
        ControlType.SPEEDOMETER, ControlType.SPEED_TEXT ->
            colsFor(if (config.imperial) "MPH 000" else "KM/H 000") to 22
        ControlType.TACHOMETER -> colsFor("RPM 00000") to 22
        ControlType.TURBO -> colsFor("BAR 0.0") to 22
        ControlType.FUEL -> colsFor("FUEL 000%") to 22
        ControlType.ENGINE_TEMP -> colsFor("TEMP C 000") to 22
        ControlType.GEAR_TEXT -> colsFor("R") to 12
        // Six labels side by side, and they are the widest thing on the HUD:
        // four characters plus a pixel either side, with a gap between each.
        ControlType.DASH_LIGHTS -> 6 * (4 * 6 + 4) + 5 to 13
        ControlType.STEERING_BAR -> 24 to 11
        else -> 16 to 16
    }

// ---------------------------------------------------------------- readouts --

private fun Pix.readout(el: Element, t: Protocol.Telemetry, config: Config, steer: Float) {
    when (el.type) {
        ControlType.SPEEDOMETER -> {
            val v = if (config.imperial) t.speedKmh * 0.621371f else t.speedKmh
            val max = if (config.imperial) config.maxSpeed * 0.621371f else config.maxSpeed
            gauge(if (config.imperial) "MPH" else "KM/H", v.roundToInt().toString(),
                v / max.coerceAtLeast(1f), 0f)
        }
        ControlType.TACHOMETER -> {
            val auto = config.autoRpm && t.maxRpm > 100f
            val max = if (auto) t.maxRpm else config.maxRpm
            val red = if (auto && t.redline > 100f) t.redline else config.redlineRpm
            gauge("RPM", t.rpm.roundToInt().toString(), t.rpm / max.coerceAtLeast(1f),
                red / max.coerceAtLeast(1f))
        }
        ControlType.TURBO -> {
            val bar = (t.turbo * 10f).roundToInt()
            gauge(if (config.imperial) "PSI" else "BAR", "${bar / 10}.${kotlin.math.abs(bar % 10)}",
                t.turbo / 2f, 0f)
        }
        ControlType.FUEL -> gauge("FUEL", "${(t.fuel * 100f).roundToInt()}%", t.fuel, 0f)
        ControlType.ENGINE_TEMP -> {
            val c = t.engineTemp
            val shown = if (config.imperial) c * 9f / 5f + 32f else c
            gauge(if (config.imperial) "TEMP F" else "TEMP C", shown.roundToInt().toString(),
                c / 120f, 0f)
        }
        ControlType.GEAR_TEXT -> big(gearText(t.gear))
        ControlType.SPEED_TEXT -> {
            val v = if (config.imperial) t.speedKmh * 0.621371f else t.speedKmh
            big(v.roundToInt().toString(), if (config.imperial) "mph" else "km/h")
        }
        ControlType.DASH_LIGHTS -> dashLights(t.showLights)
        ControlType.STEERING_BAR -> steeringBar(steer)
        ControlType.PANEL_SCREEN -> Unit    // drawn by PanelScreen, its own grid
        else -> Unit
    }
}

/** Label, value and a dithered bar: the shape every gauge takes in this skin. */
private fun Pix.gauge(label: String, value: String, frac: Float, redFrac: Float) {
    frame(0, 0, cols, rows)
    text(label, 2, 2, 1)
    val barH = 6
    val valueTop = 10
    val room = rows - valueTop - barH - 3
    val size = fitBox(value, cols - 4, room, 4)
    text(value, (cols - textWidth(value, size)) / 2, valueTop, size)

    val y = rows - barH - 2
    frame(1, y, cols - 2, barH)
    val w = (frac.coerceIn(0f, 1f) * (cols - 4)).roundToInt()
    ditherRamp(2, y + 1, w, barH - 2, 3, 16)
    if (redFrac > 0f && redFrac < 1f) {
        val rx = 1 + (redFrac * (cols - 2)).roundToInt()
        rect(rx.coerceAtMost(cols - 2), y - 2, 1, barH + 2)
    }
}

/** One number, as large as it will go. The gear and the bare speed use this. */
private fun Pix.big(value: String, unit: String = "") {
    frame(0, 0, cols, rows)
    // Only the frame and a pixel of air: a gear that reads 'N' in a box this
    // size should fill it, not sit in the middle of it at text size 1.
    val room = rows - if (unit.isEmpty()) 2 else 10
    val size = fitBox(value, cols - 2, room, 6)
    val h = textHeight(size)
    val top = if (unit.isEmpty()) (rows - h) / 2 else (rows - h - 8) / 2
    text(value, (cols - textWidth(value, size)) / 2, top, size)
    if (unit.isNotEmpty()) text(unit, (cols - textWidth(unit, 1)) / 2, top + h + 2, 1)
}

private fun Pix.dashLights(showLights: Int) {
    val items = listOf("HBRK" to 0x4, "ABS" to 0x400, "TC" to 0x10,
        "OIL" to 0x100, "BATT" to 0x200, "BEAM" to 0x2)
    val gap = 1
    val each = (cols - gap * (items.size - 1)) / items.size
    val h = minOf(rows, 13)
    val top = (rows - h) / 2
    // Every label takes the size the narrowest of them can manage, so the row
    // reads as one instrument instead of six differently sized ones.
    val size = items.minOf { (name, _) -> fitBox(name, each - 2, h - 2, 2) }
    items.forEachIndexed { i, (name, bit) ->
        tag(name, i * (each + gap), top, each, h, size, (showLights and bit) != 0)
    }
}

/** The steering indicator: a centre tick and a block that slides along it. */
private fun Pix.steeringBar(steer: Float) {
    val y = (rows - 7) / 2
    frame(0, y, cols, 7)
    rect(cols / 2, y - 2, 1, 11)
    val knob = maxOf(3, cols / 16)
    val span = cols - 2 - knob
    val x = 1 + ((steer.coerceIn(-1f, 1f) + 1f) / 2f * span).roundToInt()
    rect(x, y + 1, knob, 5)
}

private fun gearText(gear: Int) = when {
    gear <= 0 -> "R"
    gear == 1 -> "N"
    else -> (gear - 1).toString()
}

// ------------------------------------------------------------ interactive --

/** A vertical pedal: drag up to apply, lift to spring back, like the real one. */
@Composable
private fun PixelPedal(label: String, enabled: Boolean, cell: Float, onValue: (Int) -> Unit) {
    var frac by remember { mutableStateOf(0f) }
    var heightPx by remember { mutableStateOf(1f) }
    val haptic = LocalHapticFeedback.current
    Canvas(
        Modifier.fillMaxSize().then(
            if (enabled) Modifier.pointerInput(Unit) {
                awaitEachGesture {
                    heightPx = size.height.toFloat().coerceAtLeast(1f)
                    fun apply(y: Float) {
                        frac = ((heightPx - y) / heightPx).coerceIn(0f, 1f)
                        onValue((frac * 255).roundToInt())
                    }
                    val down = awaitFirstDown()
                    haptic.performHapticFeedback(HapticFeedbackType.LongPress)
                    apply(down.position.y)
                    while (true) {
                        val change = awaitPointerEvent().changes.first()
                        if (!change.pressed) break
                        apply(change.position.y)
                    }
                    frac = 0f
                    onValue(0)
                }
            } else Modifier,
        ),
    ) {
        // A pedal is narrow by nature, so its label is what decides the grid.
        pixels(cellFor(cell, colsFor(label, 2), 20)) {
            frame(0, 0, cols, rows)
            val inner = rows - 2
            val fill = (frac * inner).roundToInt()
            // Density rises towards the top, so how hard it is pressed reads at
            // a glance even where the bar is narrow.
            for (j in 0 until fill) {
                val lv = 4 + 12 * (j + 1) / inner
                dither(1, rows - 1 - j, cols - 2, 1, lv)
            }
            text(label, (cols - textWidth(label, 1)) / 2, 2, 1)
            val pct = "${(frac * 100).roundToInt()}%"
            text(pct, (cols - textWidth(pct, 1)) / 2, rows / 2, 1,
                if (fill > rows / 2) PIXEL_BG else PIXEL_LIT)
        }
    }
}

/** Press and hold: the whole block lights up, the way an inverted label does. */
@Composable
private fun PixelHold(
    label: String, enabled: Boolean, cell: Float, onPressed: (Boolean) -> Unit,
) {
    var down by remember { mutableStateOf(false) }
    val haptic = LocalHapticFeedback.current
    Canvas(
        Modifier.fillMaxSize().then(
            if (enabled) Modifier.pointerInput(Unit) {
                detectTapGestures(onPress = {
                    haptic.performHapticFeedback(HapticFeedbackType.LongPress)
                    down = true
                    onPressed(true)
                    try { awaitRelease() } finally { down = false; onPressed(false) }
                })
            } else Modifier,
        ),
    ) {
        pixels(cellFor(cell, colsFor(label), 11)) {
            val size = fitBox(label, cols - 4, rows - 4, 3)
            tag(label, 0, 0, cols, rows, size, down)
        }
    }
}

@Composable
private fun PixelButton(
    el: Element, enabled: Boolean, cell: Float, onMask: (Int, Boolean) -> Unit,
) {
    var toggled by remember(el.id) { mutableStateOf(false) }
    var down by remember { mutableStateOf(false) }
    val mask = el.button or el.button2
    val label = el.label.ifBlank { XInput.comboName(el.button, el.button2) }
    val haptic = LocalHapticFeedback.current
    Canvas(
        Modifier.fillMaxSize().then(
            if (enabled) Modifier.pointerInput(el.id, el.momentary) {
                detectTapGestures(onPress = {
                    haptic.performHapticFeedback(HapticFeedbackType.LongPress)
                    if (el.momentary) {
                        down = true
                        onMask(mask, true)
                        try { awaitRelease() } finally { down = false; onMask(mask, false) }
                    } else {
                        toggled = !toggled
                        onMask(mask, toggled)
                        awaitRelease()
                    }
                })
            } else Modifier,
        ),
    ) {
        pixels(cellFor(cell, colsFor(label), 11)) {
            val size = fitBox(label, cols - 4, rows - 4, 3)
            tag(label, 0, 0, cols, rows, size, down || toggled)
        }
    }
}

/** The free stick: a ring, a crosshair and a knob that follows your thumb. */
@Composable
private fun PixelStick(
    label: String, springBack: Boolean, enabled: Boolean, cell: Float,
    onValue: (Float, Float) -> Unit,
) {
    var knob by remember { mutableStateOf(Offset.Zero) }
    val haptic = LocalHapticFeedback.current
    Canvas(
        Modifier.fillMaxSize().then(
            if (enabled) Modifier.pointerInput(springBack) {
                awaitEachGesture {
                    val radius = min(size.width, size.height) / 2f * 0.72f
                    val centre = Offset(size.width / 2f, size.height / 2f)
                    fun apply(p: Offset) {
                        var d = p - centre
                        val len = hypot(d.x, d.y)
                        if (len > radius) d *= radius / len
                        knob = d / radius
                        onValue(knob.x, -knob.y)
                    }
                    val first = awaitFirstDown()
                    haptic.performHapticFeedback(HapticFeedbackType.LongPress)
                    apply(first.position)
                    while (true) {
                        val change = awaitPointerEvent().changes.first()
                        if (!change.pressed) break
                        apply(change.position)
                    }
                    if (springBack) {
                        knob = Offset.Zero
                        onValue(0f, 0f)
                    }
                }
            } else Modifier,
        ),
    ) {
        pixels(cellFor(cell, 16, 16)) {
            val cx = cols / 2
            val cy = rows / 2
            val r = (min(cols, rows) / 2f * 0.72f).toInt()
            circle(cx, cy, r)
            rect(cx - 1, cy, 3, 1)
            rect(cx, cy - 1, 1, 3)
            val kr = maxOf(2, r * 4 / 10)
            fillCircle(cx + (knob.x * r).roundToInt(), cy + (knob.y * r).roundToInt(), kr)
            if (label.isNotBlank()) text(label, (cols - textWidth(label, 1)) / 2, 1, 1)
        }
    }
}

/** The on-screen wheel. Same drag maths as the colour skin, drawn on the grid. */
@Composable
private fun PixelWheel(
    maxAngleRad: Float, resetKey: Int, enabled: Boolean, cell: Float, onSteer: (Float) -> Unit,
) {
    var rotation by remember(resetKey) { mutableStateOf(0f) }
    var lastTouch by remember { mutableStateOf(0f) }
    val haptic = LocalHapticFeedback.current
    Canvas(
        Modifier.fillMaxSize().then(
            if (enabled) Modifier.pointerInput(maxAngleRad) {
                val centre = Offset(size.width / 2f, size.height / 2f)
                detectDragGestures(
                    onDragStart = { pos ->
                        lastTouch = atan2(pos.y - centre.y, pos.x - centre.x)
                        haptic.performHapticFeedback(HapticFeedbackType.LongPress)
                    },
                    onDrag = { change, _ ->
                        val a = atan2(change.position.y - centre.y, change.position.x - centre.x)
                        var d = a - lastTouch
                        while (d > Math.PI) d -= (2 * Math.PI).toFloat()
                        while (d < -Math.PI) d += (2 * Math.PI).toFloat()
                        lastTouch = a
                        rotation = (rotation + d).coerceIn(-maxAngleRad, maxAngleRad)
                        onSteer(rotation / maxAngleRad)
                        change.consume()
                    },
                )
            } else Modifier,
        ),
    ) {
        pixels(cellFor(cell, 20, 20)) {
            val cx = cols / 2
            val cy = rows / 2
            val r = min(cols, rows) / 2 - 1
            // The rim is two rings rather than a thick stroke: on a grid, that
            // is what thickness looks like.
            circle(cx, cy, r)
            circle(cx, cy, r - 1)
            fillCircle(cx, cy, maxOf(2, r / 5))
            // Three spokes, drawn at the rotated angle so the pixels stay
            // square. Rotating the canvas instead would tilt the whole grid.
            for (base in listOf(Math.PI, 0.0, Math.PI / 2)) {
                val a = base + rotation
                line(cx, cy,
                    cx + (r * cos(a)).roundToInt(), cy + (r * sin(a)).roundToInt())
            }
            val top = -Math.PI / 2 + rotation
            fillCircle(
                cx + (r * 0.86 * cos(top)).roundToInt(),
                cy + (r * 0.86 * sin(top)).roundToInt(),
                maxOf(1, r / 8),
            )
        }
    }
}
