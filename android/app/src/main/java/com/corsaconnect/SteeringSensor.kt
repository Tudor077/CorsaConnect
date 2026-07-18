package com.corsaconnect

import android.content.Context
import android.hardware.Sensor
import android.hardware.SensorEvent
import android.hardware.SensorEventListener
import android.hardware.SensorManager
import kotlin.math.atan2
import kotlin.math.abs
import kotlin.math.hypot

/**
 * Turns the phone into a steering wheel. Two modes:
 *
 * - **Gravity** (default): the in-plane angle of the gravity vector tracks the
 *   phone's rotation. It never drifts, but it wraps at +/-180 degrees, so it
 *   can only model up to a ~180-degree wheel.
 *
 * - **Gyro + accel fusion**: integrates the gyroscope's angular rate for an
 *   unbounded wheel angle (spin it like a real 900/1080-degree wheel), while the
 *   gravity vector slowly pulls that angle back to its drift-free absolute
 *   reading so it doesn't wander over time. Gravity is ambiguous every 360
 *   degrees, but the gyro's own running estimate resolves which turn we're on
 *   (shortest-path correction). The correction only runs while the phone is held
 *   upright enough for the in-plane gravity angle to be meaningful.
 *
 * The user picks center with [calibrate]; [steerNormalized] is the result in -1..1.
 */
class SteeringSensor(context: Context) : SensorEventListener {
    private val sensorManager =
        context.getSystemService(Context.SENSOR_SERVICE) as SensorManager
    private val gravity: Sensor? = sensorManager.getDefaultSensor(Sensor.TYPE_GRAVITY)
    private val gyro: Sensor? = sensorManager.getDefaultSensor(Sensor.TYPE_GYROSCOPE)

    /** Raw in-plane gravity angle (radians) from the latest sample. */
    @Volatile private var rawAngle = 0f
    /** In-plane gravity magnitude (m/s^2); low when the phone lies flat. */
    @Volatile private var planeG = 0f
    /** Gravity angle captured as "wheel centered". */
    @Volatile private var centerOffset = 0f

    /** Accumulated gyro angle (radians) since the last [calibrate], unbounded. */
    @Volatile private var gyroAngle = 0f
    /** Timestamp (ns) of the previous gyro sample, for integration. */
    private var lastGyroTs = 0L

    /** Use fused gyro+accel instead of gravity (allows > 180-degree wheels). */
    @Volatile var useGyro = false

    /** Max physical rotation (radians) that maps to full lock. ~90 degrees. */
    var maxAngleRad = Math.toRadians(90.0).toFloat()
    /** Multiplies steering response; >1 = twitchier, <1 = calmer. */
    var sensitivity = 1.0f
    /** Fraction of travel near center that is ignored. */
    var deadZone = 0.04f

    /** True only if this device actually has a gyroscope. */
    val hasGyro: Boolean get() = gyro != null

    fun start() {
        gravity?.let {
            sensorManager.registerListener(this, it, SensorManager.SENSOR_DELAY_GAME)
        }
        gyro?.let {
            sensorManager.registerListener(this, it, SensorManager.SENSOR_DELAY_GAME)
        }
    }

    fun stop() = sensorManager.unregisterListener(this)

    /** Capture the current pose as the wheel's center. */
    fun calibrate() {
        centerOffset = rawAngle
        gyroAngle = 0f
    }

    /** Steering in -1..1 after calibration, dead zone and sensitivity. */
    fun steerNormalized(): Float {
        val raw = if (useGyro) gyroAngle else wrap(rawAngle - centerOffset)
        var a = raw / maxAngleRad * sensitivity
        a = a.coerceIn(-1f, 1f)
        if (abs(a) < deadZone) return 0f
        return a
    }

    /** Steering as the full i16 range the wire protocol uses. */
    fun steerShort(): Short = (steerNormalized() * Short.MAX_VALUE).toInt().toShort()

    override fun onSensorChanged(event: SensorEvent) {
        when (event.sensor.type) {
            Sensor.TYPE_GRAVITY -> {
                // x,y are gravity's components in the device plane; their angle is
                // the wheel rotation. Negate so tilting right steers right.
                rawAngle = -atan2(event.values[0], event.values[1])
                planeG = hypot(event.values[0], event.values[1])
            }
            Sensor.TYPE_GYROSCOPE -> {
                // values[2] is the angular rate (rad/s) around the screen axis.
                // Integrate it into an unbounded wheel angle. Negated to match the
                // gravity mode's sign (right turn -> positive steer); flip the sign
                // here if a device steers the wrong way in gyro mode.
                if (lastGyroTs != 0L) {
                    val dt = (event.timestamp - lastGyroTs) * 1e-9f
                    if (dt in 0f..0.1f) {
                        gyroAngle -= event.values[2] * dt
                        fuseWithGravity(dt)
                    }
                }
                lastGyroTs = event.timestamp
            }
        }
    }

    /**
     * Nudge the integrated gyro angle toward the drift-free gravity reading to
     * cancel long-term drift. Only runs when the phone is upright enough
     * ([planeG] high); when it lies flat the in-plane angle is just noise. The
     * [wrap] picks the shortest path, so the 360-degree ambiguity is resolved by
     * whichever turn the gyro currently thinks we're on.
     */
    private fun fuseWithGravity(dt: Float) {
        if (planeG < PLANE_MIN) return
        val gravRel = wrap(rawAngle - centerOffset)
        val err = wrap(gravRel - gyroAngle)
        gyroAngle += DRIFT_GAIN * err * dt
    }

    override fun onAccuracyChanged(sensor: Sensor?, accuracy: Int) {}

    /** Wrap an angle into -PI..PI so crossing the seam doesn't jump. */
    private fun wrap(a: Float): Float {
        var x = a
        while (x > Math.PI) x -= (2 * Math.PI).toFloat()
        while (x < -Math.PI) x += (2 * Math.PI).toFloat()
        return x
    }

    private companion object {
        /** In-plane gravity (m/s^2) below which the phone is too flat to trust. ~40 degrees up. */
        const val PLANE_MIN = 6.0f
        /** Drift-correction strength (per second). Small so it never fights real steering. */
        const val DRIFT_GAIN = 0.6f
    }
}
