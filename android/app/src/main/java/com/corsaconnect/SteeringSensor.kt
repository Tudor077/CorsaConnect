package com.corsaconnect

import android.content.Context
import android.hardware.Sensor
import android.hardware.SensorEvent
import android.hardware.SensorEventListener
import android.hardware.SensorManager
import kotlin.math.atan2
import kotlin.math.abs

/**
 * Turns the phone into a steering wheel using the gravity vector.
 *
 * Hold the phone upright facing you (landscape) and rotate it in its own plane
 * like a real wheel. The in-plane angle of gravity tracks that rotation and,
 * unlike a raw gyroscope, it never drifts. The user picks the center with
 * [calibrate]; [steerNormalized] is the result in -1..1.
 */
class SteeringSensor(context: Context) : SensorEventListener {
    private val sensorManager =
        context.getSystemService(Context.SENSOR_SERVICE) as SensorManager
    private val gravity: Sensor? = sensorManager.getDefaultSensor(Sensor.TYPE_GRAVITY)

    /** Raw in-plane angle (radians) from the latest sensor sample. */
    @Volatile private var rawAngle = 0f
    /** Angle captured as "wheel centered". */
    @Volatile private var centerOffset = 0f

    /** Max physical rotation (radians) that maps to full lock. ~90 degrees. */
    var maxAngleRad = Math.toRadians(90.0).toFloat()
    /** Multiplies steering response; >1 = twitchier, <1 = calmer. */
    var sensitivity = 1.0f
    /** Fraction of travel near center that is ignored. */
    var deadZone = 0.04f

    fun start() {
        gravity?.let {
            sensorManager.registerListener(this, it, SensorManager.SENSOR_DELAY_GAME)
        }
    }

    fun stop() = sensorManager.unregisterListener(this)

    /** Capture the current pose as the wheel's center. */
    fun calibrate() {
        centerOffset = rawAngle
    }

    /** Steering in -1..1 after calibration, dead zone and sensitivity. */
    fun steerNormalized(): Float {
        var a = wrap(rawAngle - centerOffset) / maxAngleRad * sensitivity
        a = a.coerceIn(-1f, 1f)
        if (abs(a) < deadZone) return 0f
        return a
    }

    /** Steering as the full i16 range the wire protocol uses. */
    fun steerShort(): Short = (steerNormalized() * Short.MAX_VALUE).toInt().toShort()

    override fun onSensorChanged(event: SensorEvent) {
        if (event.sensor.type != Sensor.TYPE_GRAVITY) return
        // x,y are gravity's components in the device plane; their angle is the
        // wheel rotation. Negate so tilting right steers right.
        rawAngle = -atan2(event.values[0], event.values[1])
    }

    override fun onAccuracyChanged(sensor: Sensor?, accuracy: Int) {}

    /** Wrap an angle into -PI..PI so crossing the seam doesn't jump. */
    private fun wrap(a: Float): Float {
        var x = a
        while (x > Math.PI) x -= (2 * Math.PI).toFloat()
        while (x < -Math.PI) x += (2 * Math.PI).toFloat()
        return x
    }
}
