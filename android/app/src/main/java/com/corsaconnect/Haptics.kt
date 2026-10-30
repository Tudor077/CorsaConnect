package com.corsaconnect

import android.content.Context
import android.media.AudioAttributes
import android.os.Build
import android.os.SystemClock
import android.os.VibrationAttributes
import android.os.VibrationEffect
import android.os.Vibrator
import android.os.VibratorManager

/*
 * Force feedback on the phone's one vibration motor.
 *
 * What it plays, and where it comes from:
 *   * In BeamNG with DotDash 1.3+, the game's own gamepad vibration
 *     (lua/vehicle/extensions/input/haptics.lua, sent by the CorsaConnect
 *     protocol): the road - bumps, kerbs, hits and tyre slip - plus the two
 *     trigger channels, wheelspin and lock-up, and ABS working.
 *   * Without it, the phone's own estimate of a slide (from MotionSim).
 *   * Events everywhere: a crash, a gear change, a crunch when shifting
 *     without the clutch, the starter when the engine catches.
 *
 * How it plays them, from Android's haptics guidance:
 *   * Continuous textures are repeating ON/OFF waveforms, started once and
 *     replaced only when the texture actually changes. Calling vibrate() every
 *     frame cancels the motor mid-spin over and over ("cancelled_superseded" in
 *     dumpsys), which feels like a weak flat buzz.
 *   * Many phones (the Nothing Phone included) have no amplitude control: any
 *     level plays at 100%. Strength is then how long the motor is on per beat,
 *     so every texture is a duty cycle, and the intensity slider scales it.
 *   * Pulses must be >= ~50 ms apart to be felt as pulses; 5-10 ms gaps blur.
 *   * Games are media, not touch feedback: USAGE_MEDIA / USAGE_GAME.
 *
 * [HapticMixer] decides what should be playing (plain Kotlin, unit-tested);
 * [HapticsEngine] runs it on a thread and drives the motor.
 */

/** On/off for each channel, mirrored from [Config]. */
data class HapticSettings(
    val enabled: Boolean = true,
    val intensity: Float = 1f,     // master 0..1
    val ignition: Boolean = true,  // starter rattle when the engine catches
    val shift: Boolean = true,     // knock on gear change
    val grind: Boolean = true,     // crunch shifting without the clutch
    val drift: Boolean = true,     // the phone's own slide estimate (MotionSim), when BeamNG sends nothing
    val collision: Boolean = true, // jolt on impact
    val road: Boolean = true,      // BeamNG: bumps, kerbs, hits, tyre slip
    val wheelspin: Boolean = true, // BeamNG: driven wheels spinning
    val brakes: Boolean = true,    // BeamNG: wheels locking, ABS working
)

/** Everything the mixer looks at, sampled each tick. */
data class HapticState(
    val active: Boolean,    // connected; telemetry expected to flow
    val rpm: Float,
    val gear: Int,
    val clutch01: Float,    // clutch pedal 0..1 (phone-local); 1 = fully pressed
    val throttle01: Float,  // throttle pedal 0..1 (phone-local)
    val shiftHeld: Boolean, // any shift-flagged button currently pressed
    val slip: Float,        // 0..1 sideways slide, from MotionSim
    val impact: Float,      // 0..1 crash strength, decaying, from MotionSim
    /** BeamNG's own gamepad vibration, while it's arriving; null otherwise. */
    val beam: Protocol.BeamHaptics? = null,
)

/**
 * A vibration to play: OFF/ON durations in ms (Android's order, starting with
 * an OFF gap), the level of each ON step 0..1, and where to loop from (-1 for
 * a one-off). Levels are what a motor with amplitude control plays; a motor
 * without it plays every ON step at full, so durations carry the strength too.
 */
data class Pattern(val timings: LongArray, val levels: FloatArray, val repeat: Int) {
    override fun equals(other: Any?) = other is Pattern && repeat == other.repeat &&
        timings.contentEquals(other.timings) && levels.contentEquals(other.levels)
    override fun hashCode() = timings.contentHashCode() * 31 + repeat
    val durationMs: Long get() = timings.sum()
}

/** What the motor should do now. */
sealed interface HapticCommand {
    /** Start this pattern, replacing whatever plays. */
    data class Play(val pattern: Pattern) : HapticCommand
    /** Stop the motor. */
    data object Stop : HapticCommand
}

/**
 * Decides what should be playing. Feed it a [HapticState] each tick with the
 * time; it answers with a command only when the motor has to change.
 */
class HapticMixer(private val amplitudeControl: Boolean) {

    /** The continuous textures, strongest wins. */
    enum class Texture { ABS, LOCKUP, ROAD, WHEELSPIN, SLIDE }

    private var settings = HapticSettings()

    // Peak-held levels: a kerb is one or two frames of BeamNG's jerk, too short
    // to feel, so each channel jumps up at once and fades at DECAY per second.
    private var road = 0f
    private var spin = 0f
    private var lock = 0f
    private var slide = 0f

    private var prevGear = Int.MIN_VALUE
    private var prevShift = false
    private var prevImpact = 0f
    private var prevRpm = 0f
    private var lastMs = -1L

    /** The texture loop now playing (or due once an event ends), as (texture, step). */
    private var playing: Pair<Texture, Int>? = null
    private var eventUntilMs = 0L
    /** When the wanted texture last differed from [playing]; changes must hold briefly. */
    private var pendingSinceMs = -1L

    fun update(s: HapticState, cfg: HapticSettings, nowMs: Long): HapticCommand? {
        settings = cfg
        val dt = if (lastMs < 0) 0f else ((nowMs - lastMs) / 1000f).coerceIn(0f, 0.1f)
        lastMs = nowMs
        if (!cfg.enabled || !s.active) {
            remember(s)
            road = 0f; spin = 0f; lock = 0f; slide = 0f
            val wasPlaying = playing != null || nowMs < eventUntilMs
            playing = null; eventUntilMs = 0L
            return if (wasPlaying) HapticCommand.Stop else null
        }

        // --- Events: they cut in, and the texture comes back after. ---
        event(s, cfg, eventPlaying = nowMs < eventUntilMs)?.let { p ->
            remember(s)
            eventUntilMs = nowMs + p.durationMs
            playing = null // re-issue the texture when the event is over
            return HapticCommand.Play(p)
        }
        remember(s)

        // --- Continuous textures. ---
        val beam = s.beam
        val decay = DECAY * dt
        road = hold(road, if (beam != null && cfg.road) beam.rumble else 0f, decay)
        spin = hold(spin, if (beam != null && cfg.wheelspin) (beam.wheelspin * WHEELSPIN_GAIN).coerceAtMost(1f) else 0f, decay)
        lock = hold(lock, if (beam != null && cfg.brakes) (beam.lockup * LOCKUP_GAIN).coerceAtMost(1f) else 0f, decay)
        // The phone's own slide estimate only stands in when the game says nothing.
        val slideIn = if (beam == null && cfg.drift) ((s.slip - SLIP_MIN) / (SLIP_FULL - SLIP_MIN)).coerceIn(0f, 1f) else 0f
        slide = hold(slide, slideIn, decay)
        val abs = beam != null && cfg.brakes && beam.abs

        if (nowMs < eventUntilMs) return null
        val want = wanted(abs)
        if (want == playing) {
            pendingSinceMs = -1L
            return null
        }
        // Starting, stopping or going stronger is immediate; easing off waits
        // a moment, so a level hovering on a step boundary doesn't restart the
        // loop every frame.
        val stronger = want != null && (playing == null || want.first.ordinal < playing!!.first.ordinal ||
            (want.first == playing!!.first && want.second > playing!!.second))
        if (!stronger && want != null) {
            if (pendingSinceMs < 0) pendingSinceMs = nowMs
            if (nowMs - pendingSinceMs < SETTLE_MS) return null
        }
        pendingSinceMs = -1L
        playing = want
        return if (want == null) HapticCommand.Stop else HapticCommand.Play(texture(want.first, want.second))
    }

    private fun remember(s: HapticState) {
        prevGear = s.gear; prevShift = s.shiftHeld; prevImpact = s.impact; prevRpm = s.rpm
    }

    private fun hold(current: Float, input: Float, decay: Float): Float =
        maxOf(input.coerceIn(0f, 1f), current - decay).coerceAtLeast(0f)

    /** The texture to play, and its strength step 1..STEPS, or null for none. */
    private fun wanted(abs: Boolean): Pair<Texture, Int>? {
        if (abs) return Texture.ABS to STEPS
        // Lock-up and the road are what you'd feel first; the spin is a fine
        // grain under them. Ties go to the earlier one in [Texture].
        val candidates = listOf(
            Texture.LOCKUP to lock, Texture.ROAD to road, Texture.WHEELSPIN to spin * 0.8f, Texture.SLIDE to slide,
        )
        val (t, level) = candidates.maxByOrNull { it.second } ?: return null
        if (level < FLOOR) return null
        return t to step(if (t == Texture.WHEELSPIN) spin else level)
    }

    private fun step(level: Float) = (1 + (level * STEPS).toInt()).coerceIn(1, STEPS)

    /** A repeating OFF/ON loop for a texture at a step. Strength is duty cycle
     *  (and amplitude where the motor has it), all scaled by the intensity. */
    fun texture(t: Texture, step: Int): Pattern {
        val x = (step - 1f) / (STEPS - 1f) // 0..1
        val (on, period, level) = when (t) {
            // ~15 Hz, even and hard: ABS hammering the brakes.
            Texture.ABS -> Triple(26f, 66f, 0.9f)
            // Near-continuous: the tyre dragging.
            Texture.LOCKUP -> Triple(lerp(45f, 85f, x), 95f, lerp(0.5f, 1f, x))
            // The road: sparse taps when it's light, a pounding when it's rough.
            Texture.ROAD -> Triple(lerp(14f, 42f, x), lerp(120f, 60f, x), lerp(0.3f, 1f, x))
            // A fine quick grain, never too heavy.
            Texture.WHEELSPIN -> Triple(lerp(9f, 16f, x), lerp(55f, 45f, x), lerp(0.25f, 0.6f, x))
            Texture.SLIDE -> Triple(lerp(12f, 34f, x), lerp(110f, 65f, x), lerp(0.25f, 0.9f, x))
        }
        val scaledOn = scaleOn(on)
        val off = (period - scaledOn).coerceAtLeast(MIN_GAP_MS.toFloat())
        // [off, on] looping from index 0.
        return Pattern(longArrayOf(off.toLong(), scaledOn.toLong()), floatArrayOf(0f, level), 0)
    }

    /** The intensity slider: on a motor without amplitude control it can only
     *  shorten the pulse; with control, amplitude does the work instead. */
    private fun scaleOn(on: Float): Float =
        if (amplitudeControl) on else (on * (0.35f + 0.65f * settings.intensity)).coerceAtLeast(MIN_ON_MS.toFloat())

    /** An event to play now, if one just happened. While one is still
     *  playing only a crash may cut in: shifting without the clutch fires the
     *  crunch and then the gear change a frame later, and the knock used to
     *  cancel the crunch 20-40 ms in, so it was never felt. */
    private fun event(s: HapticState, cfg: HapticSettings, eventPlaying: Boolean): Pattern? {
        // A crash: a hard slam and a rough tail, longer the bigger the hit.
        if (cfg.collision && s.impact >= IMPACT_FIRE && prevImpact < IMPACT_FIRE) {
            val mag = ((s.impact - IMPACT_FIRE) / (1f - IMPACT_FIRE)).coerceIn(0f, 1f)
            val slam = lerp(120f, 320f, mag).toLong()
            return Pattern(longArrayOf(0, slam, 60, slam / 3, 60, slam / 4), floatArrayOf(0f, 1f, 0f, 0.7f, 0f, 0.5f), -1)
        }
        if (eventPlaying) return null
        // Shifting with the clutch up: a long crunch.
        if (cfg.grind && s.shiftHeld && !prevShift && s.clutch01 < CLUTCH_DISENGAGE) {
            return Pattern(longArrayOf(0, 70, 25, 70, 25, 90), floatArrayOf(0f, 1f, 0f, 1f, 0f, 1f), -1)
        }
        // A gear going in: two firm knocks.
        if (cfg.shift && prevGear != Int.MIN_VALUE && s.gear != prevGear) {
            return Pattern(longArrayOf(0, scaleOn(45f).toLong(), 55, scaleOn(35f).toLong()), floatArrayOf(0f, 1f, 0f, 0.8f), -1)
        }
        // The engine catching: the starter's rattle. BeamNG reports 0 rpm
        // until it fires, so it's the 0 -> running edge.
        if (cfg.ignition && prevRpm < ENGINE_MIN_RPM && s.rpm >= ENGINE_MIN_RPM && prevGear != Int.MIN_VALUE) {
            val t = mutableListOf(0L)
            val l = mutableListOf(0f)
            repeat(4) { t += 70; l += 0.9f; t += 60; l += 0f }
            return Pattern(t.toLongArray(), l.toFloatArray(), -1)
        }
        return null
    }

    private fun lerp(a: Float, b: Float, t: Float) = a + (b - a) * t

    companion object {
        const val STEPS = 5
        const val FLOOR = 0.04f         // below this, nothing to feel
        const val DECAY = 6f            // per second: a spike to 1 lasts ~150 ms
        const val SETTLE_MS = 90L       // easing off has to hold this long
        const val MIN_ON_MS = 8L        // shorter, and the motor barely spins up
        const val MIN_GAP_MS = 18L
        const val LOCKUP_GAIN = 7f      // BeamNG's brake trigger: a full lock-up is ~0.14
        const val WHEELSPIN_GAIN = 4f   // BeamNG's throttle trigger: a hard spin is ~0.2
        const val ENGINE_MIN_RPM = 60f
        const val CLUTCH_DISENGAGE = 0.45f
        const val SLIP_MIN = 0.035f     // ~3 degrees sideways
        const val SLIP_FULL = 0.4f      // ~36 degrees, fully sideways
        const val IMPACT_FIRE = 0.12f
    }
}

/** Runs the [HapticMixer] ~60 times a second and drives the motor with it. */
class HapticsEngine(context: Context, private val provider: () -> HapticState) {

    private val vibrator: Vibrator? = resolveVibrator(context)?.takeIf { it.hasVibrator() }
    private val amplitudeControl = vibrator?.hasAmplitudeControl() == true
    private val mixer = HapticMixer(amplitudeControl)

    @Volatile var settings = HapticSettings()

    @Volatile private var running = false
    private var thread: Thread? = null

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
        while (running) {
            try {
                when (val c = mixer.update(provider(), settings, SystemClock.elapsedRealtime())) {
                    is HapticCommand.Play -> play(c.pattern)
                    HapticCommand.Stop -> vibrator?.cancel()
                    null -> {}
                }
            } catch (_: Exception) {
            }
            try {
                Thread.sleep(16)
            } catch (_: InterruptedException) {
                break
            }
        }
        vibrator?.cancel()
    }

    private fun play(p: Pattern) {
        val v = vibrator ?: return
        val effect = if (amplitudeControl) {
            val master = settings.intensity.coerceIn(0f, 1f)
            val amps = IntArray(p.levels.size) { i ->
                if (p.levels[i] <= 0f) 0 else (p.levels[i] * master * 255f).toInt().coerceIn(1, 255)
            }
            VibrationEffect.createWaveform(p.timings, amps, p.repeat)
        } else {
            VibrationEffect.createWaveform(p.timings, p.repeat)
        }
        // A game is media, not touch feedback: it follows the media vibration
        // setting and isn't muted with keyboard / touch haptics.
        if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.TIRAMISU) {
            v.vibrate(effect, VibrationAttributes.createForUsage(VibrationAttributes.USAGE_MEDIA))
        } else {
            @Suppress("DEPRECATION")
            v.vibrate(effect, AudioAttributes.Builder().setUsage(AudioAttributes.USAGE_GAME).build())
        }
    }

    private companion object {
        fun resolveVibrator(context: Context): Vibrator? =
            if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.S) {
                (context.getSystemService(Context.VIBRATOR_MANAGER_SERVICE) as? VibratorManager)?.defaultVibrator
            } else {
                @Suppress("DEPRECATION")
                context.getSystemService(Context.VIBRATOR_SERVICE) as? Vibrator
            }
    }
}
