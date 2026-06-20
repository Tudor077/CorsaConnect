package com.corsaconnect

import android.content.Context
import android.os.Build
import android.os.VibrationEffect
import android.os.Vibrator
import android.os.VibratorManager
import kotlin.random.Random

/**
 * Turns the phone into a force-feedback wheel. A single background thread reads a
 * [HapticState] snapshot ~60Hz and drives the vibrator so the wheel "feels" the
 * car:
 *   * a repetitive engine pulse that speeds up with RPM (covers idle and revs),
 *   * heavy slow thuds while the starter cranks, until the engine catches,
 *   * a crisp tick on every gear change,
 *   * a long harsh buzz when you shift without holding the clutch (grinding),
 *   * a ragged rumble while sliding/drifting.
 *
 * Everything is driven by short [VibrationEffect.createOneShot] pulses on our own
 * timer, which is what gives the deliberately "pulsing" idle the user asked for.
 */
class HapticsEngine(context: Context, private val provider: () -> HapticState) {

    private val vibrator: Vibrator? = resolveVibrator(context)?.takeIf { it.hasVibrator() }
    private val hasAmplitude = vibrator?.hasAmplitudeControl() == true

    /** Live settings, mirrored from [Config]; safe to swap from the UI thread. */
    @Volatile var settings = HapticSettings()

    @Volatile private var running = false
    private var thread: Thread? = null

    // State carried between ticks.
    private var prevGear = Int.MIN_VALUE
    private var prevShift = false
    private var prevImpact = 0f
    private var prevRpm = 0f
    private var pulseTimer = 0f
    private var grindUntilMs = 0L

    fun start() {
        if (running || vibrator == null) return
        running = true
        thread = Thread(::loop, "cc-haptics").apply { start() }
    }

    fun stop() {
        running = false
        thread?.interrupt()
        thread = null
        vibrator?.cancel()
    }

    private fun loop() {
        var last = System.nanoTime()
        while (running) {
            val now = System.nanoTime()
            val dt = ((now - last) / 1e9f).coerceAtMost(0.1f)
            last = now
            try {
                tick(dt)
            } catch (_: Exception) {
            }
            try {
                Thread.sleep(16)
            } catch (_: InterruptedException) {
                break
            }
        }
    }

    private fun tick(dt: Float) {
        val v = vibrator ?: return
        val s = provider()
        val cfg = settings

        if (!cfg.enabled || !s.active) {
            prevGear = s.gear; prevShift = s.shiftHeld; prevImpact = s.impact
            prevRpm = s.rpm; pulseTimer = 0f
            return
        }
        val master = cfg.intensity.coerceIn(0f, 1f)
        val nowMs = System.currentTimeMillis()
        var playedDiscrete = false

        // --- Crash: a big jolt the moment an impact spike lands. ---
        if (cfg.collision && s.impact >= IMPACT_FIRE && prevImpact < IMPACT_FIRE) {
            val mag = ((s.impact - IMPACT_FIRE) / (1f - IMPACT_FIRE)).coerceIn(0f, 1f)
            val dur = lerp(110f, 360f, mag).toLong()
            // Hard slam plus a short rough tail, scaled by how big the hit was.
            waveform(
                v,
                longArrayOf(0, dur, 20, (dur / 3)),
                intArrayOf(0, amp(1f, master), 0, amp(0.6f + 0.4f * mag, master)),
            )
            pulseTimer = (dur + dur / 3 + 20) / 1000f // let the crash ring out
            prevGear = s.gear; prevShift = s.shiftHeld; prevImpact = s.impact
            prevRpm = s.rpm
            return
        }

        // --- Grind: a shift action begins while the clutch pedal is up. ---
        val shiftEdge = s.shiftHeld && !prevShift
        if (cfg.grind && shiftEdge && s.clutch01 < CLUTCH_DISENGAGE) {
            oneShot(v, 200, amp(1.0f, master)) // long crunch
            grindUntilMs = nowMs + 280
            playedDiscrete = true
        }

        // --- Gear change: a hard double-knock, like a gear slamming home. ---
        val gearChanged = prevGear != Int.MIN_VALUE && s.gear != prevGear
        if (!playedDiscrete && cfg.shift && gearChanged && nowMs > grindUntilMs) {
            waveform(v, longArrayOf(0, 60, 30, 55), intArrayOf(0, amp(1f, master), 0, amp(0.85f, master)))
            playedDiscrete = true
        }

        // --- Ignition: a starter rattle the moment the engine comes to life.
        // BeamNG often reports rpm=0 until it catches, then jumps to idle, so we
        // fire on that 0 -> alive edge instead of relying on a cranking rpm band.
        if (!playedDiscrete && cfg.ignition && prevRpm < ENGINE_MIN_RPM && s.rpm >= ENGINE_MIN_RPM) {
            waveform(
                v,
                longArrayOf(0, 125, 75, 125, 75, 125, 75, 150),
                intArrayOf(
                    0, amp(0.85f, master), 0, amp(0.9f, master),
                    0, amp(0.85f, master), 0, amp(1f, master),
                ),
            )
            pulseTimer = 0.9f
            playedDiscrete = true
        }

        // --- Drift pulse: ragged rumble while sliding, scaled by how sideways. ---
        pulseTimer -= dt
        val drifting = cfg.drift && s.slip > SLIP_MIN
        if (!playedDiscrete && drifting && pulseTimer <= 0f) {
            val driftAmount = ((s.slip - SLIP_MIN) / (SLIP_FULL - SLIP_MIN)).coerceIn(0f, 1f)
            // Light slide -> barely there; fully sideways -> hard and ragged.
            val amplitude = lerp(0.18f, 1.0f, driftAmount) * (0.85f + Random.nextFloat() * 0.3f)
            oneShot(v, 18, amp(amplitude, master))
            pulseTimer = lerp(0.10f, 0.035f, driftAmount)
        } else if (!drifting) {
            pulseTimer = 0f // ready to pulse the instant a slide starts
        }

        prevGear = s.gear
        prevShift = s.shiftHeld
        prevImpact = s.impact
        prevRpm = s.rpm
    }

    private fun oneShot(v: Vibrator, ms: Long, amplitude: Int) {
        val a = if (hasAmplitude) amplitude else VibrationEffect.DEFAULT_AMPLITUDE
        v.vibrate(VibrationEffect.createOneShot(ms, a))
    }

    /** A multi-step pattern. `timings` start with an off gap; `amplitudes` are
     *  honored only on devices with amplitude control, else it's on/off. */
    private fun waveform(v: Vibrator, timings: LongArray, amplitudes: IntArray) {
        val effect = if (hasAmplitude) {
            VibrationEffect.createWaveform(timings, amplitudes, -1)
        } else {
            VibrationEffect.createWaveform(timings, -1)
        }
        v.vibrate(effect)
    }

    /** level 0..1 scaled by the master intensity, clamped to a usable 1..255. */
    private fun amp(level: Float, master: Float): Int =
        (level.coerceIn(0f, 1f) * master * 255f).toInt().coerceIn(1, 255)

    private fun lerp(a: Float, b: Float, t: Float) = a + (b - a) * t

    private companion object {
        const val ENGINE_MIN_RPM = 60f   // engine considered alive above this
        const val CLUTCH_DISENGAGE = 0.45f // pedal must be this pressed to shift clean
        const val SLIP_MIN = 0.035f  // slip fraction where a slide starts to register (~3 deg)
        const val SLIP_FULL = 0.4f   // slip fraction felt as "fully sideways" (~36 deg)
        const val IMPACT_FIRE = 0.12f // crash strength that triggers the jolt

        fun resolveVibrator(context: Context): Vibrator? =
            if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.S) {
                (context.getSystemService(Context.VIBRATOR_MANAGER_SERVICE) as? VibratorManager)
                    ?.defaultVibrator
            } else {
                @Suppress("DEPRECATION")
                context.getSystemService(Context.VIBRATOR_SERVICE) as? Vibrator
            }
    }
}

/** On/off + tuning for each feedback channel, mirrored from [Config]. */
data class HapticSettings(
    val enabled: Boolean = true,
    val intensity: Float = 1f,     // master 0..1
    val ignition: Boolean = true,  // starter rattle when the engine catches
    val shift: Boolean = true,     // hard knock on gear change
    val grind: Boolean = true,     // harsh buzz shifting without clutch
    val drift: Boolean = true,     // hard ragged rumble while sliding (real slip)
    val collision: Boolean = true, // big jolt on impact
)

/** Everything the engine needs to decide what to play, sampled each tick. */
data class HapticState(
    val active: Boolean,    // connected; telemetry expected to flow
    val rpm: Float,
    val gear: Int,
    val clutch01: Float,    // clutch pedal 0..1 (phone-local); 1 = fully pressed
    val throttle01: Float,  // throttle pedal 0..1 (phone-local)
    val shiftHeld: Boolean, // any shift-flagged button currently pressed
    val slip: Float,        // 0..1 real sideways slide, from MotionSim
    val impact: Float,      // 0..1 crash strength, decaying, from MotionSim
)
