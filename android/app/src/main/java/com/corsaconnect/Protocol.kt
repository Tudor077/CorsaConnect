package com.corsaconnect

import java.nio.ByteBuffer
import java.nio.ByteOrder

/**
 * Wire formats shared with the Rust server. Keep this in sync with
 * `server/src/protocol.rs`. Everything is little-endian.
 *
 * v2: `buttons` carries the raw 16-bit XInput button mask so the phone fully
 * controls which controller buttons each on-screen control triggers. The server
 * just forwards the mask to the virtual pad.
 * v3: adds `clutch` as a third analog pedal (maps to the right-stick Y axis).
 * v4: telemetry adds `slip` and `impact` (slide + crash) from BeamNG MotionSim.
 * v5: telemetry adds learned `maxRpm` and `redline` so the tach auto-fits the car.
 * v6: telemetry adds the rest of OutGauge: `flags`, `showLights`, two text displays.
 */
object Protocol {
    const val INPUT_PORT = 5000
    const val TELEMETRY_PORT = 5001
    const val VERSION: Byte = 6

    /** Live controller state the phone streams to the server. */
    data class Input(
        val steer: Short = 0,   // full i16 range, left thumbstick X
        val throttle: Int = 0,  // 0..255, right trigger
        val brake: Int = 0,     // 0..255, left trigger
        val clutch: Int = 0,    // 0..255, right-stick Y axis
        val buttons: Int = 0,   // raw 16-bit XInput mask, see [XInput]
    )

    /** Encode an [Input] into the 10-byte packet the server expects. */
    fun encodeInput(input: Input): ByteArray {
        val buf = ByteBuffer.allocate(10).order(ByteOrder.LITTLE_ENDIAN)
        buf.put('C'.code.toByte())
        buf.put('C'.code.toByte())
        buf.put(VERSION)
        buf.putShort((input.buttons and 0xFFFF).toShort())
        buf.putShort(input.steer)
        buf.put(input.throttle.coerceIn(0, 255).toByte())
        buf.put(input.brake.coerceIn(0, 255).toByte())
        buf.put(input.clutch.coerceIn(0, 255).toByte())
        return buf.array()
    }

    /** Telemetry pushed back from the server (parsed from BeamNG OutGauge). */
    data class Telemetry(
        val gear: Int = 1,
        val speedKmh: Float = 0f,
        val rpm: Float = 0f,
        val fuel: Float = 0f,
        val turbo: Float = 0f,
        val engineTemp: Float = 0f,
        val throttle: Float = 0f,
        val brake: Float = 0f,
        val slip: Float = 0f,   // 0..1 sideways slide (0 if MotionSim is off)
        val impact: Float = 0f, // 0..1 crash strength, decaying
        val maxRpm: Float = 0f, // learned rev range (0 until learned)
        val redline: Float = 0f,
        val flags: Int = 0,        // OutGauge status flags bitmask
        val showLights: Int = 0,   // OutGauge dash lights lit (bitmask)
        val display1: String = "", // OutGauge text display 1
        val display2: String = "", // OutGauge text display 2
    )

    /** Decode a telemetry packet, or null if it isn't one. */
    fun decodeTelemetry(data: ByteArray, len: Int): Telemetry? {
        // "CT" + version + i8 gear + 11 f32 + u16 + u32 + 2x16 bytes = 86 bytes
        if (len < 86 || data[0] != 'C'.code.toByte() || data[1] != 'T'.code.toByte()) return null
        if (data[2] != VERSION) return null
        val buf = ByteBuffer.wrap(data, 0, len).order(ByteOrder.LITTLE_ENDIAN)
        buf.position(3)
        val gear = buf.get().toInt()
        val speed = buf.float
        val rpm = buf.float
        val fuel = buf.float
        val turbo = buf.float
        val engTemp = buf.float
        val throttle = buf.float
        val brake = buf.float
        val slip = buf.float
        val impact = buf.float
        val maxRpm = buf.float
        val redline = buf.float
        val flags = buf.short.toInt() and 0xFFFF
        val showLights = buf.int
        val display1 = readText(buf, 16)
        val display2 = readText(buf, 16)
        return Telemetry(
            gear, speed, rpm, fuel, turbo, engTemp, throttle, brake, slip, impact, maxRpm, redline,
            flags, showLights, display1, display2,
        )
    }

    /** Read [n] bytes as ASCII text, trimmed at the first null. */
    private fun readText(buf: ByteBuffer, n: Int): String {
        val raw = ByteArray(n)
        buf.get(raw)
        val end = raw.indexOf(0).let { if (it < 0) n else it }
        return String(raw, 0, end, Charsets.US_ASCII).trim()
    }
}

/** Standard XInput (Xbox controller) button masks that a button can be bound to. */
object XInput {
    const val DPAD_UP = 0x0001
    const val DPAD_DOWN = 0x0002
    const val DPAD_LEFT = 0x0004
    const val DPAD_RIGHT = 0x0008
    const val START = 0x0010
    const val BACK = 0x0020
    const val LEFT_THUMB = 0x0040
    const val RIGHT_THUMB = 0x0080
    const val LB = 0x0100
    const val RB = 0x0200
    const val A = 0x1000
    const val B = 0x2000
    const val X = 0x4000
    const val Y = 0x8000

    /** Human-readable name -> mask, in the order shown in the binding picker. */
    val named: List<Pair<String, Int>> = listOf(
        "A" to A, "B" to B, "X" to X, "Y" to Y,
        "LB" to LB, "RB" to RB,
        "Start" to START, "Back" to BACK,
        "L-Stick" to LEFT_THUMB, "R-Stick" to RIGHT_THUMB,
        "D-Up" to DPAD_UP, "D-Down" to DPAD_DOWN,
        "D-Left" to DPAD_LEFT, "D-Right" to DPAD_RIGHT,
    )

    fun nameOf(mask: Int): String = named.firstOrNull { it.second == mask }?.first ?: "—"

    /** Display name for a button, plus a second one if it's a combo (a+b). */
    fun comboName(a: Int, b: Int): String =
        if (b != 0) "${nameOf(a)}+${nameOf(b)}" else nameOf(a)
}
