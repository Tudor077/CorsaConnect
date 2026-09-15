package com.corsaconnect

import android.content.Context
import android.hardware.Sensor
import android.hardware.SensorEvent
import android.hardware.SensorEventListener
import android.hardware.SensorManager
import kotlin.math.abs
import kotlin.math.atan2
import kotlin.math.hypot
import kotlin.math.min
import kotlin.math.sign
import kotlin.math.sqrt

/**
 * Turns the phone into a steering wheel.
 *
 * **Gravity is the wheel angle**; the gyroscope only fills the gaps. The
 * in-plane angle of the gravity vector is an absolute, drift-free reading of
 * how far the phone is rotated, so the wheel is pinned to it with a short
 * (~80 ms) time constant. The gyroscope is integrated on top for two
 * things only: the high-frequency detail between gravity samples, and keeping
 * count of which turn we're on so the wheel can go past 180 degrees even though
 * gravity repeats every 360.
 *
 * That ordering is what keeps it accurate. Integrating the gyro and letting
 * gravity nudge it back - the other way round - means every bit of gyro bias
 * shows up as the wheel wandering off and creeping back over a few seconds.
 * Here the steady-state error from a gyro bias is bias / [PULL_GAIN]: a bad
 * 6 deg/s bias parks the wheel half a degree off instead of ten.
 *
 * Gravity isn't always meaningful, though - the flatter the phone lies, the
 * more its in-plane angle is just noise, and linear acceleration can pollute
 * the reading on devices that derive TYPE_GRAVITY from a plain low-pass. Both
 * are folded into a 0..1 [gravTrust] that scales the pull, so the filter fades
 * to pure gyro at extreme angles and fades back in as soon as gravity is worth
 * listening to again. The gyro's turn count then decides which 360-degree
 * branch to snap back to, so nothing jumps.
 *
 * The user picks center with [calibrate]; [steerNormalized] is the result in -1..1.
 */
class SteeringSensor(context: Context) : SensorEventListener {
    private val sensorManager =
        context.getSystemService(Context.SENSOR_SERVICE) as SensorManager
    private val gravity: Sensor? = sensorManager.getDefaultSensor(Sensor.TYPE_GRAVITY)
    private val gyro: Sensor? = sensorManager.getDefaultSensor(Sensor.TYPE_GYROSCOPE)

    /** Raw in-plane gravity angle (radians) from the latest sample. */
    @Volatile private var gravAngle = 0f
    /** How much that angle is worth believing right now, 0..1. */
    @Volatile private var gravTrust = 0f
    /** Gravity angle captured as "wheel centered". */
    @Volatile private var centerOffset = 0f

    /** Fused wheel angle (radians) relative to center, unbounded. */
    @Volatile private var wheelAngle = 0f
    /** Timestamp (ns) of the previous gyro sample, for integration. */
    private var lastGyroTs = 0L

    /** Max physical rotation (radians) that maps to full lock. ~90 degrees. */
    var maxAngleRad = Math.toRadians(90.0).toFloat()
    /** Multiplies steering response; >1 = twitchier, <1 = calmer. */
    var sensitivity = 1.0f
    /** Fraction of travel near center that is ignored. */
    var deadZone = 0.04f

    /** True only if this device actually has a gyroscope. */
    val hasGyro: Boolean get() = gyro != null

    fun start() {
        // Both sensors at ~200 Hz: gravity is the reference, so sampling it
        // often is what makes the wheel feel immediate rather than filtered.
        gravity?.let { sensorManager.registerListener(this, it, SAMPLE_US) }
        gyro?.let { sensorManager.registerListener(this, it, SAMPLE_US) }
    }

    fun stop() {
        sensorManager.unregisterListener(this)
        lastGyroTs = 0L
    }

    /** Capture the current pose as the wheel's center. */
    fun calibrate() {
        centerOffset = gravAngle
        wheelAngle = 0f
    }

    /** Steering in -1..1 after calibration, dead zone and sensitivity. */
    fun steerNormalized(): Float {
        val a = (wheelAngle / maxAngleRad * sensitivity).coerceIn(-1f, 1f)
        // Rescale what's left of the travel instead of just zeroing the middle,
        // so the wheel doesn't jump by a dead zone's worth as it comes off center.
        if (abs(a) <= deadZone) return 0f
        return sign(a) * (abs(a) - deadZone) / (1f - deadZone)
    }

    /** Steering as the full i16 range the wire protocol uses. */
    fun steerShort(): Short = (steerNormalized() * Short.MAX_VALUE).toInt().toShort()

    override fun onSensorChanged(event: SensorEvent) {
        when (event.sensor.type) {
            Sensor.TYPE_GRAVITY -> onGravity(event)
            Sensor.TYPE_GYROSCOPE -> onGyro(event)
        }
    }

    /**
     * New absolute reading. x,y are gravity's components in the device plane;
     * their angle is the wheel rotation (negated so tilting right steers right).
     */
    private fun onGravity(event: SensorEvent) {
        val (x, y, z) = Triple(event.values[0], event.values[1], event.values[2])
        gravAngle = -atan2(x, y)

        // Trust falls off as the phone lies flatter: the in-plane part of
        // gravity is what carries the angle, and once it's small the angle is
        // mostly noise (its error scales as 1 / inPlane).
        val inPlane = hypot(x, y) / EARTH_G
        var trust = smoothStep(inPlane, PLANE_LOW, PLANE_HIGH)
        // A real gravity vector is always ~9.81 long. If it isn't, this device
        // is low-passing raw acceleration and hand movement is leaking in.
        val magErr = abs(sqrt(x * x + y * y + z * z) - EARTH_G)
        trust *= 1f - smoothStep(magErr, MAG_TOL, MAG_TOL + MAG_SPAN)
        gravTrust = trust

        // Without a gyro there's nothing to fuse: follow gravity directly. The
        // wheel then wraps at +/-180 degrees, which is all such a device can do.
        if (gyro == null) wheelAngle = wrap(gravAngle - centerOffset)
    }

    /**
     * values[2] is the angular rate (rad/s) around the screen axis. Integrate
     * it for the between-samples detail and the turn count, then pull the
     * result back onto gravity. Negated to match the gravity sign (right turn
     * -> positive steer); flip it here if a device steers the wrong way.
     */
    private fun onGyro(event: SensorEvent) {
        val prev = lastGyroTs
        lastGyroTs = event.timestamp
        if (prev == 0L) return
        val dt = (event.timestamp - prev) * 1e-9f
        if (dt <= 0f || dt > MAX_DT) return

        wheelAngle -= event.values[2] * dt
        pullToGravity(dt)
    }

    /**
     * Pull the wheel angle onto the absolute gravity reading, at a rate set by
     * how much gravity is worth believing. [wrap] picks the nearest 360-degree
     * branch, so which turn we're on comes from the gyro and only the
     * within-turn angle comes from gravity.
     */
    private fun pullToGravity(dt: Float) {
        val trust = gravTrust
        if (trust <= 0f) return
        val err = wrap(gravAngle - centerOffset - wheelAngle)
        wheelAngle += err * min(1f, PULL_GAIN * trust * dt)
    }

    override fun onAccuracyChanged(sensor: Sensor?, accuracy: Int) {}

    /** Wrap an angle into -PI..PI so crossing the seam doesn't jump. */
    private fun wrap(a: Float): Float {
        var x = a
        while (x > Math.PI) x -= (2 * Math.PI).toFloat()
        while (x < -Math.PI) x += (2 * Math.PI).toFloat()
        return x
    }

    /** 0 below [lo], 1 above [hi], smooth in between (no kink at either end). */
    private fun smoothStep(v: Float, lo: Float, hi: Float): Float {
        val t = ((v - lo) / (hi - lo)).coerceIn(0f, 1f)
        return t * t * (3f - 2f * t)
    }

    private companion object {
        /** Sensor period in microseconds (~200 Hz). */
        const val SAMPLE_US = 5_000
        const val EARTH_G = 9.80665f

        /** In-plane gravity (as a fraction of g) where trust starts / is full. */
        const val PLANE_LOW = 0.34f  // ~20 degrees off flat
        const val PLANE_HIGH = 0.64f // ~40 degrees off flat

        /** Gravity magnitude error (m/s^2) tolerated before trust starts falling. */
        const val MAG_TOL = 0.5f
        /** Further error over which trust fades to nothing. */
        const val MAG_SPAN = 2.0f

        /**
         * How hard gravity pulls the wheel, per second, at full trust. 12 is an
         * ~83 ms time constant: fast enough that gyro bias never shows, slow
         * enough that gravity's own noise stays out of the wheel.
         */
        const val PULL_GAIN = 12f

        /** Ignore gyro gaps longer than this (app was paused, sensor hiccup). */
        const val MAX_DT = 0.1f
    }
}
