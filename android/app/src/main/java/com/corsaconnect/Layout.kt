package com.corsaconnect

import android.content.Context
import org.json.JSONArray
import org.json.JSONObject
import java.util.UUID

/** Kinds of things that can sit on the HUD. */
enum class ControlType {
    STEERING_BAR,    // visual indicator of the gyro steering
    STEERING_WHEEL,  // on-screen wheel: drag to rotate, no motion sensors
    GAS,             // hold -> throttle (right trigger), on/off
    BRAKE,           // hold -> brake (left trigger), on/off
    THROTTLE_SLIDER, // vertical analog pedal -> throttle (right trigger)
    BRAKE_SLIDER,    // vertical analog pedal -> brake (left trigger)
    CLUTCH_SLIDER,   // vertical analog pedal -> clutch (right-stick Y)
    JOYSTICK,        // two-axis thumbstick -> the free stick axes (vJoy RX/RY)
    PANEL_SCREEN,    // the PicoPanel dashboard, redrawn at the phone's resolution
    BUTTON,          // hold/toggle -> an XInput button (configurable)
    SPEEDOMETER,     // analog speed gauge (telemetry)
    TACHOMETER,      // analog rpm gauge (telemetry)
    GEAR_TEXT,       // current gear
    SPEED_TEXT,      // numeric km/h
    TURBO,           // turbo boost gauge (bar/psi)
    FUEL,            // fuel level %
    ENGINE_TEMP,     // engine temperature (C/F)
    DASH_LIGHTS,     // lit dash indicators (ABS, TC, handbrake, ...)
    TURN_SIGNALS,    // the two blinker arrows, flashing with the car's own
}

/** Visual skin for the gauges/readouts. */
enum class Design {
    MODERN,  // the dark, colourful default
    VAPOR,   // monochrome green-grey LCD (Trail Tech Vapor look)
    PIXEL;   // the PicoPanel's own 1-bit look, whole pixels on black

    companion object {
        fun from(name: String?) = entries.firstOrNull { it.name == name } ?: MODERN
    }
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
    val button2: Int = 0,      // optional second XInput mask, pressed together (combo)
    val momentary: Boolean = true, // BUTTON: hold vs toggle. JOYSTICK: spring back to centre vs stay put
    val shift: Boolean = false, // BUTTON: a gear-shift, so grind feedback applies
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
        put("button2", button2)
        put("momentary", momentary)
        put("shift", shift)
    }

    companion object {
        /** Returns null for unknown/removed control types so old layouts that
         *  reference them just drop those elements instead of failing to load. */
        fun fromJson(o: JSONObject): Element? {
            val type = try {
                ControlType.valueOf(o.getString("type"))
            } catch (e: Exception) {
                return null
            }
            return Element(
                id = o.optString("id", UUID.randomUUID().toString()),
                type = type,
                x = o.getDouble("x").toFloat(),
                y = o.getDouble("y").toFloat(),
                w = o.getDouble("w").toFloat(),
                h = o.getDouble("h").toFloat(),
                label = o.optString("label", ""),
                button = o.optInt("button", 0),
                button2 = o.optInt("button2", 0),
                momentary = o.optBoolean("momentary", true),
                shift = o.optBoolean("shift", false),
            )
        }

        fun newOf(type: ControlType): Element {
            val (w, h) = when (type) {
                ControlType.SPEEDOMETER, ControlType.TACHOMETER -> 0.18f to 0.45f
                ControlType.STEERING_BAR -> 0.5f to 0.06f
                ControlType.STEERING_WHEEL -> 0.28f to 0.55f
                ControlType.GEAR_TEXT, ControlType.SPEED_TEXT -> 0.1f to 0.18f
                ControlType.TURBO -> 0.14f to 0.34f
                ControlType.FUEL, ControlType.ENGINE_TEMP -> 0.12f to 0.16f
                ControlType.DASH_LIGHTS -> 0.3f to 0.1f
                ControlType.TURN_SIGNALS -> 0.2f to 0.14f
                ControlType.THROTTLE_SLIDER, ControlType.BRAKE_SLIDER, ControlType.CLUTCH_SLIDER -> 0.12f to 0.6f
                // Roughly square on a typical 20:9 phone held sideways.
                ControlType.JOYSTICK -> 0.22f to 0.5f
                // The panel's own proportions, minus the header.
                ControlType.PANEL_SCREEN -> 0.6f to 0.24f
                else -> 0.16f to 0.3f
            }
            val label = when (type) {
                ControlType.GAS, ControlType.THROTTLE_SLIDER -> "GAS"
                ControlType.BRAKE, ControlType.BRAKE_SLIDER -> "BRAKE"
                ControlType.CLUTCH_SLIDER -> "CLUTCH"
                ControlType.JOYSTICK -> "LOOK"
                ControlType.BUTTON -> "A"
                else -> ""
            }
            val button = if (type == ControlType.BUTTON) XInput.A else 0
            return Element(UUID.randomUUID().toString(), type, 0.4f, 0.4f, w, h, label, button)
        }
    }
}

/** A named, saveable HUD layout the user can switch between. */
data class LayoutPreset(val name: String, val elements: List<Element>) {
    fun toJson(): JSONObject = JSONObject().apply {
        put("name", name)
        put("elements", JSONArray().apply { elements.forEach { put(it.toJson()) } })
    }

    companion object {
        fun fromJson(o: JSONObject): LayoutPreset {
            val arr = o.getJSONArray("elements")
            return LayoutPreset(
                o.getString("name"),
                (0 until arr.length()).mapNotNull { Element.fromJson(arr.getJSONObject(it)) },
            )
        }
    }
}

/** The whole HUD plus steering tuning and the saved server IP. */
data class Config(
    val serverIp: String = "192.168.1.141",
    val sensitivity: Float = 1f,
    val deadZone: Float = 0.04f,
    val maxAngleDeg: Float = 90f,
    val touchWheel: Boolean = false,    // steer from the on-screen wheel widget instead of motion sensors
    val maxSpeed: Float = 260f,
    val maxRpm: Float = 8000f,
    val redlineRpm: Float = 6500f,      // where the rpm gauge turns red; match BeamNG per car
    val autoRpm: Boolean = true,        // auto-fit tach max/redline to the car (learned)
    // Which preset is active: "manual", "automatic", or a saved custom name.
    val activePreset: String = "manual",
    val presets: List<LayoutPreset> = emptyList(), // user-saved custom layouts
    val digitalGauges: Boolean = false, // false = analog dials, true = digital numbers (all gauges)
    val imperial: Boolean = false,      // false = metric (km/h, C, bar), true = imperial (mph, F, psi)
    val design: Design = Design.MODERN, // visual skin for gauges/readouts
    // Force-feedback (vibration). See [HapticSettings].
    val haptics: Boolean = true,
    val hapticIntensity: Float = 1f,     // master 0..1
    val hapticIgnition: Boolean = true,  // starter rattle when the engine catches
    val hapticShift: Boolean = true,     // knock on gear change
    val hapticGrind: Boolean = true,     // buzz when shifting without clutch
    val hapticDrift: Boolean = true,     // rumble while sliding (real slip)
    val hapticCollision: Boolean = true, // jolt on impact
    val elements: List<Element>,
) {
    /** The vibration channels, shaped for [HapticsEngine]. */
    fun hapticSettings() = HapticSettings(
        enabled = haptics,
        intensity = hapticIntensity,
        ignition = hapticIgnition,
        shift = hapticShift,
        grind = hapticGrind,
        drift = hapticDrift,
        collision = hapticCollision,
    )

    fun toJson(): JSONObject = JSONObject().apply {
        put("serverIp", serverIp)
        put("sensitivity", sensitivity.toDouble())
        put("deadZone", deadZone.toDouble())
        put("maxAngleDeg", maxAngleDeg.toDouble())
        put("touchWheel", touchWheel)
        put("maxSpeed", maxSpeed.toDouble())
        put("maxRpm", maxRpm.toDouble())
        put("redlineRpm", redlineRpm.toDouble())
        put("autoRpm", autoRpm)
        put("activePreset", activePreset)
        put("presets", JSONArray().apply { presets.forEach { put(it.toJson()) } })
        put("digitalGauges", digitalGauges)
        put("imperial", imperial)
        put("design", design.name)
        put("haptics", haptics)
        put("hapticIntensity", hapticIntensity.toDouble())
        put("hapticIgnition", hapticIgnition)
        put("hapticShift", hapticShift)
        put("hapticGrind", hapticGrind)
        put("hapticDrift", hapticDrift)
        put("hapticCollision", hapticCollision)
        put("elements", JSONArray().apply { elements.forEach { put(it.toJson()) } })
    }

    companion object {
        fun fromJson(o: JSONObject): Config {
            val arr = o.getJSONArray("elements")
            val els = (0 until arr.length()).mapNotNull { Element.fromJson(arr.getJSONObject(it)) }
            return Config(
                serverIp = o.optString("serverIp", "192.168.1.141"),
                sensitivity = o.optDouble("sensitivity", 1.0).toFloat(),
                deadZone = o.optDouble("deadZone", 0.04).toFloat(),
                maxAngleDeg = o.optDouble("maxAngleDeg", 90.0).toFloat(),
                touchWheel = o.optBoolean("touchWheel", false),
                maxSpeed = o.optDouble("maxSpeed", 260.0).toFloat(),
                maxRpm = o.optDouble("maxRpm", 8000.0).toFloat(),
                redlineRpm = o.optDouble("redlineRpm", 6500.0).toFloat(),
                autoRpm = o.optBoolean("autoRpm", true),
                activePreset = o.optString("activePreset", "manual"),
                presets = o.optJSONArray("presets")?.let { arr ->
                    (0 until arr.length()).map { LayoutPreset.fromJson(arr.getJSONObject(it)) }
                } ?: emptyList(),
                digitalGauges = o.optBoolean("digitalGauges", o.optBoolean("speedoDigital", false)),
                imperial = o.optBoolean("imperial", false),
                design = Design.from(o.optString("design", "MODERN")),
                haptics = o.optBoolean("haptics", true),
                hapticIntensity = o.optDouble("hapticIntensity", 1.0).toFloat(),
                hapticIgnition = o.optBoolean("hapticIgnition", true),
                hapticShift = o.optBoolean("hapticShift", true),
                hapticGrind = o.optBoolean("hapticGrind", true),
                hapticDrift = o.optBoolean("hapticDrift", true),
                hapticCollision = o.optBoolean("hapticCollision", true),
                elements = els,
            )
        }

        /** The stock config: manual layout, gearbox auto-detected. */
        fun default(): Config = Config(elements = manualLayout())

        /** Gauges, gear, shift buttons and handbrake shared by both layouts. */
        private fun common(): List<Element> = listOf(
            Element(id("steer"), ControlType.STEERING_BAR, 0.30f, 0.01f, 0.40f, 0.06f),
            Element(id("speedo"), ControlType.SPEEDOMETER, 0.31f, 0.10f, 0.18f, 0.45f),
            Element(id("tacho"), ControlType.TACHOMETER, 0.51f, 0.10f, 0.18f, 0.45f),
            Element(id("gear"), ControlType.GEAR_TEXT, 0.455f, 0.60f, 0.09f, 0.18f),
            Element(id("up"), ControlType.BUTTON, 0.83f, 0.06f, 0.15f, 0.13f, "SHIFT ↑", XInput.RB, shift = true),
            Element(id("down"), ControlType.BUTTON, 0.83f, 0.21f, 0.15f, 0.13f, "SHIFT ↓", XInput.LB, shift = true),
            Element(id("hand"), ControlType.BUTTON, 0.02f, 0.06f, 0.15f, 0.16f, "HAND", XInput.A),
        )

        /** Manual gearbox: clutch on the left, brake + throttle on the right. */
        fun manualLayout(): List<Element> = common() + listOf(
            Element(id("clutch"), ControlType.CLUTCH_SLIDER, 0.02f, 0.36f, 0.11f, 0.60f, "CLUTCH"),
            Element(id("brake"), ControlType.BRAKE_SLIDER, 0.74f, 0.36f, 0.11f, 0.60f, "BRAKE"),
            Element(id("gas"), ControlType.THROTTLE_SLIDER, 0.87f, 0.36f, 0.11f, 0.60f, "GAS"),
        )

        /** Automatic gearbox: no clutch, brake on the left, throttle on the right. */
        fun automaticLayout(): List<Element> = common() + listOf(
            Element(id("brake"), ControlType.BRAKE_SLIDER, 0.02f, 0.36f, 0.11f, 0.60f, "BRAKE"),
            Element(id("gas"), ControlType.THROTTLE_SLIDER, 0.87f, 0.36f, 0.11f, 0.60f, "GAS"),
        )

        private fun id(s: String) = "$s-${UUID.randomUUID().toString().take(4)}"
    }
}

/**
 * Persists [Config] as a JSON blob in SharedPreferences.
 *
 * There used to be exactly one copy: anything that made [Config.fromJson] throw
 * fell back to [Config.default], and the next save - a drag, a rebind, anything
 * - wrote that default over the only copy, taking every saved preset with it.
 * So there are three keys now: the live blob, the one before it, and a parking
 * spot for a blob we couldn't read. Nothing is ever overwritten by a fallback.
 */
class ConfigStore(context: Context) {
    private val prefs = context.getSharedPreferences("corsaconnect", Context.MODE_PRIVATE)

    fun load(): Config {
        val raw = prefs.getString(KEY, null) ?: return Config.default()
        try {
            return Config.fromJson(JSONObject(raw))
        } catch (e: Exception) {
            // Park the unreadable blob under its own key before anything else
            // can write over it, then fall back to the last known good one.
            prefs.edit().putString(KEY_BROKEN, raw).apply()
        }
        return prefs.getString(KEY_PREV, null)
            ?.let { try { Config.fromJson(JSONObject(it)) } catch (e: Exception) { null } }
            ?: Config.default()
    }

    fun save(config: Config) {
        val json = config.toJson().toString()
        val current = prefs.getString(KEY, null)
        prefs.edit().apply {
            if (current != null && current != json) putString(KEY_PREV, current)
            putString(KEY, json)
        }.apply()
    }

    /** The blob [load] couldn't parse, kept for recovery. Null if there is none. */
    fun broken(): String? = prefs.getString(KEY_BROKEN, null)

    private companion object {
        const val KEY = "config"
        const val KEY_PREV = "config.prev"
        const val KEY_BROKEN = "config.broken"
    }
}
