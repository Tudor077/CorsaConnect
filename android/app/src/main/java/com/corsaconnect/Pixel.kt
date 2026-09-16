package com.corsaconnect

import androidx.compose.ui.geometry.Offset
import androidx.compose.ui.geometry.Size
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.graphics.drawscope.DrawScope
import kotlin.math.abs
import kotlin.math.roundToInt
import kotlin.math.sqrt

/**
 * The 1-bit drawing kit behind the PIXEL design.
 *
 * Everything in that skin is drawn the way the PicoPanel draws it: whole pixels
 * on a grid, lit or unlit, nothing in between. The grid is deliberately coarse.
 * The point is that a phone and a 128x32 OLED show the same dashboard, and the
 * only honest way to do that is to give the phone the same pixels, just bigger.
 *
 * The font is not a lookalike. These are the bytes out of Adafruit GFX's
 * glcdfont.c, the font the panel itself draws with: five columns per glyph, one
 * bit per pixel, least significant bit at the top. A word rendered here and the
 * same word on the panel are the same shape.
 */

/** Pure black, because an OLED pixel that is off emits nothing at all. */
val PIXEL_BG = Color(0xFF000000)
/** A lit pixel, with the faint blue an OLED really has. */
val PIXEL_LIT = Color(0xFFE6F2FF)
/** Drawn but unlit: gauge ticks, the empty part of a bar, a button at rest. */
val PIXEL_DIM = Color(0xFF1B2733)

private const val FONT_FIRST = 0x20

/** GFX's classic 5x7 font, ASCII 0x20..0x7E, one byte per column. */
private val FONT = intArrayOf(
    0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x5F, 0x00, 0x00, 0x00, 0x07, 0x00, 0x07,
    0x00, 0x14, 0x7F, 0x14, 0x7F, 0x14, 0x24, 0x2A, 0x7F, 0x2A, 0x12, 0x23, 0x13, 0x08,
    0x64, 0x62, 0x36, 0x49, 0x56, 0x20, 0x50, 0x00, 0x08, 0x07, 0x03, 0x00, 0x00, 0x1C,
    0x22, 0x41, 0x00, 0x00, 0x41, 0x22, 0x1C, 0x00, 0x2A, 0x1C, 0x7F, 0x1C, 0x2A, 0x08,
    0x08, 0x3E, 0x08, 0x08, 0x00, 0x80, 0x70, 0x30, 0x00, 0x08, 0x08, 0x08, 0x08, 0x08,
    0x00, 0x00, 0x60, 0x60, 0x00, 0x20, 0x10, 0x08, 0x04, 0x02, 0x3E, 0x51, 0x49, 0x45,
    0x3E, 0x00, 0x42, 0x7F, 0x40, 0x00, 0x72, 0x49, 0x49, 0x49, 0x46, 0x21, 0x41, 0x49,
    0x4D, 0x33, 0x18, 0x14, 0x12, 0x7F, 0x10, 0x27, 0x45, 0x45, 0x45, 0x39, 0x3C, 0x4A,
    0x49, 0x49, 0x31, 0x41, 0x21, 0x11, 0x09, 0x07, 0x36, 0x49, 0x49, 0x49, 0x36, 0x46,
    0x49, 0x49, 0x29, 0x1E, 0x00, 0x00, 0x14, 0x00, 0x00, 0x00, 0x40, 0x34, 0x00, 0x00,
    0x00, 0x08, 0x14, 0x22, 0x41, 0x14, 0x14, 0x14, 0x14, 0x14, 0x00, 0x41, 0x22, 0x14,
    0x08, 0x02, 0x01, 0x59, 0x09, 0x06, 0x3E, 0x41, 0x5D, 0x59, 0x4E, 0x7C, 0x12, 0x11,
    0x12, 0x7C, 0x7F, 0x49, 0x49, 0x49, 0x36, 0x3E, 0x41, 0x41, 0x41, 0x22, 0x7F, 0x41,
    0x41, 0x41, 0x3E, 0x7F, 0x49, 0x49, 0x49, 0x41, 0x7F, 0x09, 0x09, 0x09, 0x01, 0x3E,
    0x41, 0x41, 0x51, 0x73, 0x7F, 0x08, 0x08, 0x08, 0x7F, 0x00, 0x41, 0x7F, 0x41, 0x00,
    0x20, 0x40, 0x41, 0x3F, 0x01, 0x7F, 0x08, 0x14, 0x22, 0x41, 0x7F, 0x40, 0x40, 0x40,
    0x40, 0x7F, 0x02, 0x1C, 0x02, 0x7F, 0x7F, 0x04, 0x08, 0x10, 0x7F, 0x3E, 0x41, 0x41,
    0x41, 0x3E, 0x7F, 0x09, 0x09, 0x09, 0x06, 0x3E, 0x41, 0x51, 0x21, 0x5E, 0x7F, 0x09,
    0x19, 0x29, 0x46, 0x26, 0x49, 0x49, 0x49, 0x32, 0x03, 0x01, 0x7F, 0x01, 0x03, 0x3F,
    0x40, 0x40, 0x40, 0x3F, 0x1F, 0x20, 0x40, 0x20, 0x1F, 0x3F, 0x40, 0x38, 0x40, 0x3F,
    0x63, 0x14, 0x08, 0x14, 0x63, 0x03, 0x04, 0x78, 0x04, 0x03, 0x61, 0x59, 0x49, 0x4D,
    0x43, 0x00, 0x7F, 0x41, 0x41, 0x41, 0x02, 0x04, 0x08, 0x10, 0x20, 0x00, 0x41, 0x41,
    0x41, 0x7F, 0x04, 0x02, 0x01, 0x02, 0x04, 0x40, 0x40, 0x40, 0x40, 0x40, 0x00, 0x03,
    0x07, 0x08, 0x00, 0x20, 0x54, 0x54, 0x78, 0x40, 0x7F, 0x28, 0x44, 0x44, 0x38, 0x38,
    0x44, 0x44, 0x44, 0x28, 0x38, 0x44, 0x44, 0x28, 0x7F, 0x38, 0x54, 0x54, 0x54, 0x18,
    0x00, 0x08, 0x7E, 0x09, 0x02, 0x18, 0xA4, 0xA4, 0x9C, 0x78, 0x7F, 0x08, 0x04, 0x04,
    0x78, 0x00, 0x44, 0x7D, 0x40, 0x00, 0x20, 0x40, 0x40, 0x3D, 0x00, 0x7F, 0x10, 0x28,
    0x44, 0x00, 0x00, 0x41, 0x7F, 0x40, 0x00, 0x7C, 0x04, 0x78, 0x04, 0x78, 0x7C, 0x08,
    0x04, 0x04, 0x78, 0x38, 0x44, 0x44, 0x44, 0x38, 0xFC, 0x18, 0x24, 0x24, 0x18, 0x18,
    0x24, 0x24, 0x18, 0xFC, 0x7C, 0x08, 0x04, 0x04, 0x08, 0x48, 0x54, 0x54, 0x54, 0x24,
    0x04, 0x04, 0x3F, 0x44, 0x24, 0x3C, 0x40, 0x40, 0x20, 0x7C, 0x1C, 0x20, 0x40, 0x20,
    0x1C, 0x3C, 0x40, 0x30, 0x40, 0x3C, 0x44, 0x28, 0x10, 0x28, 0x44, 0x4C, 0x90, 0x90,
    0x90, 0x7C, 0x44, 0x64, 0x54, 0x4C, 0x44, 0x00, 0x08, 0x36, 0x41, 0x00, 0x00, 0x00,
    0x77, 0x00, 0x00, 0x00, 0x41, 0x36, 0x08, 0x00, 0x02, 0x01, 0x02, 0x04, 0x02
)

private val BAYER4 = intArrayOf(
    0, 8, 2, 10,
    12, 4, 14, 6,
    3, 11, 1, 9,
    15, 7, 13, 5,
)

/** Grid painter. Every coordinate below is a pixel of the virtual display. */
class Pix(private val ds: DrawScope, val cell: Float, val cols: Int, val rows: Int) {

    fun rect(x: Int, y: Int, w: Int, h: Int, color: Color = PIXEL_LIT) {
        if (w <= 0 || h <= 0) return
        ds.drawRect(color, Offset(x * cell, y * cell), Size(w * cell, h * cell))
    }

    fun dot(x: Int, y: Int, color: Color = PIXEL_LIT) = rect(x, y, 1, 1, color)

    /** A hollow rectangle one pixel thick, like GFX's drawRect. */
    fun frame(x: Int, y: Int, w: Int, h: Int, color: Color = PIXEL_LIT) {
        if (w <= 0 || h <= 0) return
        rect(x, y, w, 1, color)
        rect(x, y + h - 1, w, 1, color)
        rect(x, y, 1, h, color)
        rect(x + w - 1, y, 1, h, color)
    }

    /** Bresenham, so a diagonal lands on whole pixels instead of a smear. */
    fun line(x0: Int, y0: Int, x1: Int, y1: Int, color: Color = PIXEL_LIT) {
        var x = x0
        var y = y0
        val dx = abs(x1 - x0)
        val dy = -abs(y1 - y0)
        val sx = if (x0 < x1) 1 else -1
        val sy = if (y0 < y1) 1 else -1
        var err = dx + dy
        while (true) {
            dot(x, y, color)
            if (x == x1 && y == y1) break
            val e2 = 2 * err
            if (e2 >= dy) { err += dy; x += sx }
            if (e2 <= dx) { err += dx; y += sy }
        }
    }

    /** Midpoint circle, outline only. */
    fun circle(cx: Int, cy: Int, r: Int, color: Color = PIXEL_LIT) {
        var x = r
        var y = 0
        var err = 1 - r
        while (x >= y) {
            dot(cx + x, cy + y, color); dot(cx + y, cy + x, color)
            dot(cx - x, cy + y, color); dot(cx - y, cy + x, color)
            dot(cx - x, cy - y, color); dot(cx - y, cy - x, color)
            dot(cx + x, cy - y, color); dot(cx + y, cy - x, color)
            y++
            if (err < 0) {
                err += 2 * y + 1
            } else {
                x--
                err += 2 * (y - x) + 1
            }
        }
    }

    fun fillCircle(cx: Int, cy: Int, r: Int, color: Color = PIXEL_LIT) {
        for (dy in -r..r) {
            val w = sqrt((r * r - dy * dy).toDouble()).toInt()
            rect(cx - w, cy + dy, 2 * w + 1, 1, color)
        }
    }

    /**
     * Filled triangle, ported from Adafruit_GFX::fillTriangle.
     *
     * Not my own scanline fill: a triangle's edge pixels depend entirely on how
     * the rasteriser rounds, and mine disagreed with the panel on four pixels of
     * the blinker arrow. Since the whole point is that the two screens draw the
     * same picture, this follows GFX's arithmetic exactly, truncating division
     * included.
     */
    fun triangle(
        x0: Int, y0: Int, x1: Int, y1: Int, x2: Int, y2: Int, color: Color = PIXEL_LIT,
    ) {
        var ax = x0; var ay = y0
        var bx = x1; var by = y1
        var cx = x2; var cy = y2
        // Sort so that ay <= by <= cy.
        if (ay > by) { val tx = ax; ax = bx; bx = tx; val ty = ay; ay = by; by = ty }
        if (by > cy) { val tx = bx; bx = cx; cx = tx; val ty = by; by = cy; cy = ty }
        if (ay > by) { val tx = ax; ax = bx; bx = tx; val ty = ay; ay = by; by = ty }

        if (ay == cy) {                       // all on one line
            val lo = minOf(ax, bx, cx)
            val hi = maxOf(ax, bx, cx)
            rect(lo, ay, hi - lo + 1, 1, color)
            return
        }

        val dx01 = bx - ax; val dy01 = by - ay
        val dx02 = cx - ax; val dy02 = cy - ay
        val dx12 = cx - bx; val dy12 = cy - by
        var sa = 0
        var sb = 0

        // The middle scanline belongs to the second loop unless the triangle is
        // flat-bottomed, exactly as in GFX.
        val last = if (by == cy) by else by - 1
        var y = ay
        while (y <= last) {
            var a = ax + sa / dy01
            var b = ax + sb / dy02
            sa += dx01
            sb += dx02
            if (a > b) { val t = a; a = b; b = t }
            rect(a, y, b - a + 1, 1, color)
            y++
        }
        sa = dx12 * (y - by)
        sb = dx02 * (y - ay)
        while (y <= cy) {
            var a = bx + sa / dy12
            var b = ax + sb / dy02
            sa += dx12
            sb += dx02
            if (a > b) { val t = a; a = b; b = t }
            rect(a, y, b - a + 1, 1, color)
            y++
        }
    }

    /**
     * GFX's 4x4 ordered dither, level 0..16. A screen with no grey fakes one
     * with density, and reusing the same threshold matrix is what makes a bar
     * here look like the same bar there.
     */
    fun dither(x: Int, y: Int, w: Int, h: Int, level: Int, color: Color = PIXEL_LIT) {
        if (level <= 0 || w <= 0 || h <= 0) return
        if (level >= 16) { rect(x, y, w, h, color); return }
        for (i in 0 until w) for (j in 0 until h) {
            if (BAYER4[((j and 3) shl 2) or (i and 3)] < level) dot(x + i, y + j, color)
        }
    }

    /** Density rising from left to right. */
    fun ditherRamp(
        x: Int, y: Int, w: Int, h: Int, from: Int, to: Int, color: Color = PIXEL_LIT,
    ) {
        if (w <= 0) return
        for (i in 0 until w) {
            val lv = from + (to - from) * i / (if (w > 1) w - 1 else 1)
            for (j in 0 until h) {
                if (BAYER4[((j and 3) shl 2) or (i and 3)] < lv) dot(x + i, y + j, color)
            }
        }
    }

    /** Text, GFX style: (x, y) is the top-left of the glyph box, 6*size wide. */
    fun text(s: String, x: Int, y: Int, size: Int = 1, color: Color = PIXEL_LIT) {
        var cx = x
        for (ch in s) {
            val idx = (ch.code - FONT_FIRST) * 5
            if (idx >= 0 && idx + 5 <= FONT.size) {
                for (col in 0 until 5) {
                    val bits = FONT[idx + col]
                    for (row in 0 until 7) {
                        if ((bits shr row) and 1 == 1) {
                            rect(cx + col * size, y + row * size, size, size, color)
                        }
                    }
                }
            }
            cx += 6 * size
        }
    }

    fun textWidth(s: String, size: Int = 1) = s.length * 6 * size
    fun textHeight(size: Int = 1) = 7 * size

    fun textCentred(s: String, y: Int, size: Int = 1, color: Color = PIXEL_LIT) =
        text(s, (cols - textWidth(s, size)) / 2, y, size, color)

    /** The biggest text size that fits `s` into `w` pixels, at least 1. */
    fun fit(s: String, w: Int, max: Int = 4): Int {
        for (size in max downTo 1) if (textWidth(s, size) <= w) return size
        return 1
    }

    /** A label: lit on black, or inverted into a filled block when it is on. */
    fun tag(s: String, x: Int, y: Int, w: Int, h: Int, size: Int, on: Boolean) {
        if (on) {
            rect(x, y, w, h)
            text(s, x + (w - textWidth(s, size)) / 2, y + (h - textHeight(size)) / 2, size, PIXEL_BG)
        } else {
            frame(x, y, w, h)
            text(s, x + (w - textWidth(s, size)) / 2, y + (h - textHeight(size)) / 2, size)
        }
    }
}

/** Run grid drawing inside a Compose canvas. */
fun DrawScope.pixels(cell: Float, block: Pix.() -> Unit) {
    val cols = (size.width / cell).toInt()
    val rows = (size.height / cell).toInt()
    if (cols > 0 && rows > 0) Pix(this, cell, cols, rows).block()
}
