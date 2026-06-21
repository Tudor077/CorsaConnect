package com.corsaconnect

import androidx.compose.foundation.Canvas
import androidx.compose.foundation.background
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Spacer
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.height
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.runtime.Composable
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.draw.clip
import androidx.compose.ui.geometry.Offset
import androidx.compose.ui.geometry.Size
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.graphics.StrokeCap
import androidx.compose.ui.graphics.drawscope.Stroke
import androidx.compose.ui.text.font.FontFamily
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.unit.dp
import androidx.compose.ui.unit.sp
import androidx.compose.material3.Text
import kotlin.math.cos
import kotlin.math.min
import kotlin.math.roundToInt
import kotlin.math.sin

private const val START_ANGLE = 135f   // down-left
private const val SWEEP = 270f          // clockwise to down-right

// --- Vapor LCD skin (monochrome green-grey) ---
val LCD_BG = Color(0xFFB3BCA6)   // light panel
val LCD_DARK = Color(0xFF1B2316) // lit segments / text
val LCD_DIM = Color(0xFF8C9783)  // unlit / ghost segments
val LCD_FILL = Color(0xFF6E7A5E) // pedal / bar fill

/** Seven-segment patterns (segments a,b,c,d,e,f,g) for the chars we display. */
private val SEGMENTS = mapOf(
    '0' to "abcdef", '1' to "bc", '2' to "abged", '3' to "abgcd", '4' to "fgbc",
    '5' to "afgcd", '6' to "afgecd", '7' to "abc", '8' to "abcdefg", '9' to "abcdfg",
    '-' to "g", 'R' to "eg", 'N' to "ceg", ' ' to "",
)

/** Draws [text] as a monochrome 7-segment LCD display, faint ghost segments and
 *  all, scaled to fit. Supports digits, '.', '-', and R / N (for the gear). */
@Composable
fun SegmentDisplay(text: String, on: Color, off: Color, modifier: Modifier = Modifier, cells: Int = 0) {
    Canvas(modifier) {
        val raw = text.uppercase().toCharArray()
        // Fixed-width: reserve `cells` digits, right-aligned, blanks shown as
        // faint ghost segments so digits keep a constant size and don't jump.
        val n = maxOf(cells, raw.size).coerceAtLeast(1)
        val pad = n - raw.size
        val cellH = size.height
        val cellW = (cellH * 0.58f).coerceAtMost(size.width / n)
        val startX = (size.width - cellW * n) / 2f
        val t = cellW * 0.15f
        val m = cellW * 0.2f
        for (i in 0 until n) {
            val ci = i - pad
            val ghost = ci < 0
            val ch = if (ghost) '8' else raw[ci]
            val ox = startX + i * cellW
            if (ch == '.') {
                drawCircle(on, radius = t * 0.7f, center = Offset(ox + cellW * 0.5f, cellH - m))
                continue
            }
            val segs = SEGMENTS[ch] ?: ""
            val x0 = ox + m
            val x1 = ox + cellW - m
            val y0 = m
            val ymid = cellH / 2f
            val y1 = cellH - m
            fun seg(id: Char, ax: Float, ay: Float, bx: Float, by: Float) {
                drawLine(
                    if (!ghost && segs.contains(id)) on else off,
                    Offset(ax, ay), Offset(bx, by),
                    strokeWidth = t, cap = StrokeCap.Round,
                )
            }
            seg('a', x0, y0, x1, y0)
            seg('b', x1, y0, x1, ymid)
            seg('c', x1, ymid, x1, y1)
            seg('d', x0, y1, x1, y1)
            seg('e', x0, ymid, x0, y1)
            seg('f', x0, y0, x0, ymid)
            seg('g', x0, ymid, x1, ymid)
        }
    }
}

private val LcdLabelFont = FontFamily.Monospace

/** Tach-style LCD gauge: a row of ticks that curves up on the left then
 *  straightens across the top (Trail Tech Vapor style). Ticks are evenly spaced
 *  along the path and sized off the height, so they scale uniformly and keep
 *  equal gaps no matter the widget's size or aspect. */
@Composable
fun LcdArcGauge(
    value: Float,
    maxValue: Float,
    unit: String,
    modifier: Modifier = Modifier,
    redlineFrac: Float = 1f,
) {
    val frac = (value / maxValue).coerceIn(0f, 1f)
    Box(modifier.background(LCD_BG), contentAlignment = Alignment.Center) {
        Column(Modifier.fillMaxSize().padding(6.dp), horizontalAlignment = Alignment.CenterHorizontally) {
            Canvas(Modifier.fillMaxWidth().weight(1f)) {
                val rOut = size.height * 0.82f
                val rIn = rOut * 0.78f // short ticks, trimmed from the bottom
                val rMid = (rIn + rOut) / 2f
                val cy0 = size.height * 0.96f
                val startA = Math.toRadians(186.0).toFloat() // lower-left
                val endA = Math.toRadians(90.0).toFloat()    // straight up
                val sweep = startA - endA
                val cx0 = 4f + (-cos(startA)) * rOut          // curve centre x
                val rightEnd = size.width - 4f
                val curveLen = rMid * sweep
                val straightLen = (rightEnd - cx0).coerceAtLeast(0f)
                val totalLen = curveLen + straightLen
                // Fixed spacing (off the height) -> equal gaps; tick count adapts.
                val step = (rOut * 0.2f).coerceAtLeast(3f)
                val w = step * 0.45f
                val n = (totalLen / step).toInt().coerceAtLeast(1)
                for (j in 0 until n) {
                    val pos = (j + 0.5f) * step
                    val lf = pos / totalLen
                    val col = when {
                        lf <= frac -> LCD_DARK
                        redlineFrac < 1f && lf > redlineFrac -> LCD_DARK.copy(alpha = 0.4f)
                        else -> LCD_DIM
                    }
                    val inner: Offset
                    val outer: Offset
                    if (pos <= curveLen) {
                        val a = startA - (pos / curveLen) * sweep
                        inner = Offset(cx0 + cos(a) * rIn, cy0 - sin(a) * rIn)
                        outer = Offset(cx0 + cos(a) * rOut, cy0 - sin(a) * rOut)
                    } else {
                        val sx = cx0 + (pos - curveLen)
                        inner = Offset(sx, cy0 - rIn)
                        outer = Offset(sx, cy0 - rOut)
                    }
                    drawLine(col, inner, outer, strokeWidth = w, cap = StrokeCap.Butt)
                }
            }
            Text(unit, color = LCD_DARK, fontSize = 10.sp, fontFamily = LcdLabelFont)
        }
    }
}

/** An LCD-panel gauge: a segment bar + a big 7-segment number, all monochrome. */
@Composable
fun LcdGauge(
    value: Float,
    maxValue: Float,
    bigText: String,
    unit: String,
    modifier: Modifier = Modifier,
    redlineFrac: Float = 1f,
    bar: Boolean = true,
    cells: Int = 0,
) {
    val frac = (value / maxValue).coerceIn(0f, 1f)
    Box(modifier.background(LCD_BG), contentAlignment = Alignment.Center) {
        Column(Modifier.fillMaxSize().padding(8.dp), horizontalAlignment = Alignment.CenterHorizontally) {
            if (bar) {
                Canvas(Modifier.fillMaxWidth().height(12.dp)) {
                    val n = 24
                    val gap = size.width * 0.012f
                    val bw = (size.width - gap * (n - 1)) / n
                    for (i in 0 until n) {
                        val f = (i + 1).toFloat() / n
                        val lit = f <= frac
                        val redzone = redlineFrac < 1f && f > redlineFrac
                        val c = when {
                            lit -> LCD_DARK
                            redzone -> LCD_DARK.copy(alpha = 0.4f)
                            else -> LCD_DIM
                        }
                        drawRect(c, topLeft = Offset(i * (bw + gap), 0f), size = Size(bw, size.height))
                    }
                }
            }
            SegmentDisplay(bigText, LCD_DARK, LCD_DIM.copy(alpha = 0.4f), Modifier.fillMaxWidth().weight(1f), cells)
            Text(unit, color = LCD_DARK, fontSize = 10.sp, fontFamily = LcdLabelFont)
        }
    }
}

/** An LCD-panel text readout (gear, speed text, fuel, temp). */
@Composable
fun LcdReadout(value: String, label: String, big: Boolean = false, modifier: Modifier = Modifier, cells: Int = 0) {
    Box(modifier.background(LCD_BG), contentAlignment = Alignment.Center) {
        Column(Modifier.fillMaxSize().padding(6.dp), horizontalAlignment = Alignment.CenterHorizontally) {
            SegmentDisplay(value, LCD_DARK, LCD_DIM.copy(alpha = 0.4f), Modifier.fillMaxWidth().weight(1f), cells)
            Text(label, color = LCD_DARK, fontSize = 10.sp, fontFamily = LcdLabelFont)
        }
    }
}

/**
 * A circular analog gauge. [redlineFrac] paints the tail of the arc red
 * (e.g. an rpm redline). The big number in the middle is the live value.
 */
@Composable
fun Gauge(
    value: Float,
    maxValue: Float,
    bigText: String,
    unit: String,
    accent: Color,
    modifier: Modifier = Modifier,
    redlineFrac: Float = 1f,
) {
    val frac = (value / maxValue).coerceIn(0f, 1f)
    val overRedline = redlineFrac < 1f && frac > redlineFrac
    Box(modifier, contentAlignment = Alignment.Center) {
        Canvas(Modifier.fillMaxSize()) {
            val d = min(size.width, size.height)
            val stroke = d * 0.09f
            val pad = stroke / 2 + d * 0.04f
            val arcSize = Size(d - pad * 2, d - pad * 2)
            val topLeft = Offset((size.width - arcSize.width) / 2, (size.height - arcSize.height) / 2)
            val center = Offset(size.width / 2, size.height / 2)
            val radius = arcSize.width / 2

            // Track.
            drawArc(
                color = Color(0xFF2A2A33),
                startAngle = START_ANGLE,
                sweepAngle = SWEEP,
                useCenter = false,
                topLeft = topLeft,
                size = arcSize,
                style = Stroke(width = stroke, cap = StrokeCap.Round),
            )
            // Progress fill (the rpm bar) runs the whole way, under the redline.
            drawArc(
                color = accent,
                startAngle = START_ANGLE,
                sweepAngle = SWEEP * frac,
                useCenter = false,
                topLeft = topLeft,
                size = arcSize,
                style = Stroke(width = stroke, cap = StrokeCap.Round),
            )
            // Redline zone: a translucent red overlay drawn on top, so the rpm bar
            // shows through it.
            if (redlineFrac < 1f) {
                drawArc(
                    color = Color(0x99E53935),
                    startAngle = START_ANGLE + SWEEP * redlineFrac,
                    sweepAngle = SWEEP * (1f - redlineFrac),
                    useCenter = false,
                    topLeft = topLeft,
                    size = arcSize,
                    style = Stroke(width = stroke, cap = StrokeCap.Round),
                )
            }
            // Tick marks.
            val ticks = 9
            for (i in 0..ticks) {
                val a = Math.toRadians((START_ANGLE + SWEEP * i / ticks).toDouble())
                val outer = radius - stroke
                val inner = outer - d * 0.05f
                drawLine(
                    color = Color(0xFF55555F),
                    start = center + Offset((cos(a) * outer).toFloat(), (sin(a) * outer).toFloat()),
                    end = center + Offset((cos(a) * inner).toFloat(), (sin(a) * inner).toFloat()),
                    strokeWidth = d * 0.01f,
                )
            }
            // Needle.
            val na = Math.toRadians((START_ANGLE + SWEEP * frac).toDouble())
            val nl = radius - stroke * 1.5f
            drawLine(
                color = Color.White,
                start = center,
                end = center + Offset((cos(na) * nl).toFloat(), (sin(na) * nl).toFloat()),
                strokeWidth = d * 0.02f,
                cap = StrokeCap.Round,
            )
        }
        Column(horizontalAlignment = Alignment.CenterHorizontally) {
            Text(
                bigText,
                color = if (overRedline) Color(0xFFE53935) else Color.White,
                fontSize = 30.sp,
                fontWeight = FontWeight.Bold,
            )
            Text(unit, color = Color(0xFF8A8A95), fontSize = 12.sp)
        }
    }
}

/**
 * A digital readout that fills the same footprint as [Gauge]: a big number with
 * a thin bar underneath. The bar shows the redline zone in red, and the number
 * turns red once [value] passes the redline.
 */
@Composable
fun DigitalGauge(
    value: Float,
    maxValue: Float,
    bigText: String,
    unit: String,
    accent: Color,
    modifier: Modifier = Modifier,
    redlineFrac: Float = 1f,
) {
    val frac = (value / maxValue).coerceIn(0f, 1f)
    val overRedline = redlineFrac < 1f && frac > redlineFrac
    Box(modifier, contentAlignment = Alignment.Center) {
        Column(horizontalAlignment = Alignment.CenterHorizontally) {
            Text(
                bigText,
                color = if (overRedline) Color(0xFFE53935) else Color.White,
                fontSize = 48.sp,
                fontWeight = FontWeight.Bold,
            )
            Text(unit, color = Color(0xFF8A8A95), fontSize = 13.sp)
            Box(
                Modifier
                    .padding(top = 8.dp)
                    .fillMaxWidth(0.8f)
                    .height(6.dp),
            ) {
                Canvas(Modifier.fillMaxSize()) {
                    val h = size.height
                    // Track.
                    drawLine(
                        color = Color(0xFF2A2A33),
                        start = Offset(0f, h / 2),
                        end = Offset(size.width, h / 2),
                        strokeWidth = h,
                        cap = StrokeCap.Round,
                    )
                    // Progress fill (rpm) runs the whole way, under the redline.
                    if (frac > 0f) {
                        drawLine(
                            color = accent,
                            start = Offset(0f, h / 2),
                            end = Offset(size.width * frac, h / 2),
                            strokeWidth = h,
                            cap = StrokeCap.Round,
                        )
                    }
                    // Redline zone: translucent red overlay on top.
                    if (redlineFrac < 1f) {
                        drawLine(
                            color = Color(0x99E53935),
                            start = Offset(size.width * redlineFrac, h / 2),
                            end = Offset(size.width, h / 2),
                            strokeWidth = h,
                            cap = StrokeCap.Round,
                        )
                    }
                }
            }
        }
    }
}

@Composable
fun Speedometer(
    speedKmh: Float,
    maxKmh: Float,
    digital: Boolean = false,
    imperial: Boolean = false,
    lcd: Boolean = false,
    modifier: Modifier = Modifier,
) {
    val value = if (imperial) speedKmh * 0.621371f else speedKmh
    val max = if (imperial) maxKmh * 0.621371f else maxKmh
    val unit = if (imperial) "mph" else "km/h"
    val bigText = value.roundToInt().toString()
    val accent = Color(0xFF4C8DFF)
    when {
        lcd -> LcdGauge(value, max, bigText, unit, modifier, bar = false, cells = 3)
        digital -> DigitalGauge(value, max, bigText, unit, accent, modifier)
        else -> Gauge(value, max, bigText, unit, accent, modifier)
    }
}

@Composable
fun BoostGauge(
    turboBar: Float,
    digital: Boolean = false,
    imperial: Boolean = false,
    lcd: Boolean = false,
    modifier: Modifier = Modifier,
) {
    val value = (if (imperial) turboBar * 14.5038f else turboBar).coerceAtLeast(0f)
    val max = if (imperial) 30f else 2f
    val unit = if (imperial) "psi" else "bar"
    val bigText = "%.1f".format(value)
    val accent = Color(0xFF2EC4B6)
    when {
        lcd -> LcdGauge(value, max, bigText, unit, modifier)
        digital -> DigitalGauge(value, max, bigText, unit, accent, modifier)
        else -> Gauge(value, max, bigText, unit, accent, modifier)
    }
}

@Composable
fun Tachometer(
    rpm: Float,
    maxRpm: Float,
    redlineRpm: Float,
    digital: Boolean = false,
    lcd: Boolean = false,
    modifier: Modifier = Modifier,
) {
    val redlineFrac = if (maxRpm > 0f) (redlineRpm / maxRpm).coerceIn(0f, 1f) else 1f
    val bigText = (rpm / 1000f).let { String.format("%.1f", it) }
    val accent = Color(0xFFFF9F1C)
    when {
        lcd -> LcdArcGauge(rpm, maxRpm, "rpm", modifier, redlineFrac)
        digital -> DigitalGauge(rpm, maxRpm, bigText, "x1000 rpm", accent, modifier, redlineFrac)
        else -> Gauge(rpm, maxRpm, bigText, "x1000 rpm", accent, modifier, redlineFrac)
    }
}
