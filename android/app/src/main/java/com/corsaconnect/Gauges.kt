package com.corsaconnect

import androidx.compose.foundation.Canvas
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.height
import androidx.compose.foundation.layout.padding
import androidx.compose.runtime.Composable
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.geometry.Offset
import androidx.compose.ui.geometry.Size
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.graphics.StrokeCap
import androidx.compose.ui.graphics.drawscope.Stroke
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
fun Speedometer(speedKmh: Float, maxKmh: Float, digital: Boolean = false, modifier: Modifier = Modifier) {
    val bigText = speedKmh.roundToInt().toString()
    val accent = Color(0xFF4C8DFF)
    if (digital) {
        DigitalGauge(speedKmh, maxKmh, bigText, "km/h", accent, modifier)
    } else {
        Gauge(speedKmh, maxKmh, bigText, "km/h", accent, modifier)
    }
}

@Composable
fun Tachometer(
    rpm: Float,
    maxRpm: Float,
    redlineRpm: Float,
    digital: Boolean = false,
    modifier: Modifier = Modifier,
) {
    val redlineFrac = if (maxRpm > 0f) (redlineRpm / maxRpm).coerceIn(0f, 1f) else 1f
    val bigText = (rpm / 1000f).let { String.format("%.1f", it) }
    val accent = Color(0xFFFF9F1C)
    if (digital) {
        DigitalGauge(rpm, maxRpm, bigText, "x1000 rpm", accent, modifier, redlineFrac)
    } else {
        Gauge(rpm, maxRpm, bigText, "x1000 rpm", accent, modifier, redlineFrac)
    }
}
