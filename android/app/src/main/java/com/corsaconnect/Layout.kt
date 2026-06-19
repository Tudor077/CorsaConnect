package com.corsaconnect

import android.content.Context
import org.json.JSONArray
import org.json.JSONObject
import java.util.UUID

/** Kinds of things that can sit on the HUD. */
enum class ControlType {
    STEERING_BAR,    // visual indicator of the gyro steering
    GAS,             // hold -> throttle (right trigger), on/off
    BRAKE,           // hold -> brake (left trigger), on/off
    THROTTLE_SLIDER, // vertical analog pedal -> throttle (right trigger)
    BRAKE_SLIDER,    // vertical analog pedal -> brake (left trigger)
    CLUTCH_SLIDER,   // vertical analog pedal -> clutch (right-stick Y)
    BUTTON,          // hold/toggle -> an XInput button (configurable)
    SPEEDOMETER,     // analog speed gauge (telemetry)
    TACHOMETER,      // analog rpm gauge (telemetry)
    GEAR_TEXT,       // current gear
    SPEED_TEXT,      // numeric km/h
}

/**
 * One element on the HUD. Position and size are stored as fractions of the
 * screen (0..1) so a layout looks the same regardless of device resolution.
 */
data class Element(
    val id: String,
    val type: ControlType,
    val x: Float,
    val y: Float,
    val w: Float,
    val h: Float,
    val label: String = "",
    val button: Int = 0,       // XInput mask, for BUTTON
    val momentary: Boolean = true, // BUTTON: hold vs toggle
) {
    fun toJson(): JSONObject = JSONObject().apply {
        put("id", id)
        put("type", type.name)
        put("x", x.toDouble())
        put("y", y.toDouble())
        put("w", w.toDouble())
        put("h", h.toDouble())
        put("label", label)
        put("button", button)
        put("momentary", momentary)
    }

    companion object {
        fun fromJson(o: JSONObject) = Element(
            id = o.optString("id", UUID.randomUUID().toString()),
            type = ControlType.valueOf(o.getString("type")),
            x = o.getDouble("x").toFloat(),
            y = o.getDouble("y").toFloat(),
            w = o.getDouble("w").toFloat(),
            h = o.getDouble("h").toFloat(),
            label = o.optString("label", ""),
            button = o.optInt("button", 0),
            momentary = o.optBoolean("momentary", true),
        )

        fun newOf(type: ControlType): Element {
            val (w, h) = when (type) {
                ControlType.SPEEDOMETER, ControlType.TACHOMETER -> 0.18f to 0.45f
                ControlType.STEERING_BAR -> 0.5f to 0.06f
                ControlType.GEAR_TEXT, ControlType.SPEED_TEXT -> 0.1f to 0.18f
                ControlType.THROTTLE_SLIDER, ControlType.BRAKE_SLIDER, ControlType.CLUTCH_SLIDER -> 0.12f to 0.6f
                else -> 0.16f to 0.3f
            }
            val label = when (type) {
                ControlType.GAS, ControlType.THROTTLE_SLIDER -> "GAS"
                ControlType.BRAKE, ControlType.BRAKE_SLIDER -> "BRAKE"
                ControlType.CLUTCH_SLIDER -> "CLUTCH"
                ControlType.BUTTON -> "A"
                else -> ""
            }
            val button = if (type == ControlType.BUTTON) XInput.A else 0
            return Element(UUID.randomUUID().toString(), type, 0.4f, 0.4f, w, h, label, button)
        }
    }
}

/** The whole HUD plus steering tuning and the saved server IP. */
data class Config(
    val serverIp: String = "192.168.1.141",
    val sensitivity: Float = 1f,
    val deadZone: Float = 0.04f,
    val maxAngleDeg: Float = 90f,
    val maxSpeed: Float = 260f,
    val maxRpm: Float = 8000f,
    val redlineRpm: Float = 6500f,      // where the rpm gauge turns red; match BeamNG per car
    val speedoDigital: Boolean = false, // false = analog gauge, true = digital number
    val tachoDigital: Boolean = false,
    val elements: List<Element>,
) {
    fun toJson(): JSONObject = JSONObject().apply {
        put("serverIp", serverIp)
        put("sensitivity", sensitivity.toDouble())
        put("deadZone", deadZone.toDouble())
        put("maxAngleDeg", maxAngleDeg.toDouble())
        put("maxSpeed", maxSpeed.toDouble())
        put("maxRpm", maxRpm.toDouble())
        put("redlineRpm", redlineRpm.toDouble())
        put("speedoDigital", speedoDigital)
        put("tachoDigital", tachoDigital)
        put("elements", JSONArray().apply { elements.forEach { put(it.toJson()) } })
    }

    companion object {
        fun fromJson(o: JSONObject): Config {
            val arr = o.getJSONArray("elements")
            val els = (0 until arr.length()).map { Element.fromJson(arr.getJSONObject(it)) }
            return Config(
                serverIp = o.optString("serverIp", "192.168.1.141"),
                sensitivity = o.optDouble("sensitivity", 1.0).toFloat(),
                deadZone = o.optDouble("deadZone", 0.04).toFloat(),
                maxAngleDeg = o.optDouble("maxAngleDeg", 90.0).toFloat(),
                maxSpeed = o.optDouble("maxSpeed", 260.0).toFloat(),
                maxRpm = o.optDouble("maxRpm", 8000.0).toFloat(),
                redlineRpm = o.optDouble("redlineRpm", 6500.0).toFloat(),
                speedoDigital = o.optBoolean("speedoDigital", false),
                tachoDigital = o.optBoolean("tachoDigital", false),
                elements = els,
            )
        }

        /** The stock layout, mirroring the original fixed HUD. */
        fun default(): Config = Config(
            elements = listOf(
                Element(id("steer"), ControlType.STEERING_BAR, 0.30f, 0.01f, 0.40f, 0.06f),
                Element(id("speedo"), ControlType.SPEEDOMETER, 0.31f, 0.10f, 0.18f, 0.45f),
                Element(id("tacho"), ControlType.TACHOMETER, 0.51f, 0.10f, 0.18f, 0.45f),
                Element(id("gear"), ControlType.GEAR_TEXT, 0.455f, 0.60f, 0.09f, 0.18f),
                // Three vertical pedal sliders: clutch + brake on the left, throttle on the right.
                Element(id("clutch"), ControlType.CLUTCH_SLIDER, 0.02f, 0.36f, 0.11f, 0.60f, "CLUTCH"),
                Element(id("brake"), ControlType.BRAKE_SLIDER, 0.15f, 0.36f, 0.11f, 0.60f, "BRAKE"),
                Element(id("gas"), ControlType.THROTTLE_SLIDER, 0.87f, 0.36f, 0.11f, 0.60f, "GAS"),
                Element(id("up"), ControlType.BUTTON, 0.83f, 0.06f, 0.15f, 0.13f, "SHIFT ↑", XInput.RB),
                Element(id("down"), ControlType.BUTTON, 0.83f, 0.21f, 0.15f, 0.13f, "SHIFT ↓", XInput.LB),
                Element(id("hand"), ControlType.BUTTON, 0.02f, 0.06f, 0.15f, 0.16f, "HAND", XInput.A),
            ),
        )

        private fun id(s: String) = "$s-${UUID.randomUUID().toString().take(4)}"
    }
}

/** Persists [Config] as a JSON blob in SharedPreferences. */
class ConfigStore(context: Context) {
    private val prefs = context.getSharedPreferences("corsaconnect", Context.MODE_PRIVATE)

    fun load(): Config = try {
        prefs.getString("config", null)?.let { Config.fromJson(JSONObject(it)) } ?: Config.default()
    } catch (e: Exception) {
        Config.default()
    }

    fun save(config: Config) {
        prefs.edit().putString("config", config.toJson().toString()).apply()
    }
}
