package com.corsaconnect

import android.os.Bundle
import android.view.WindowManager
import androidx.activity.ComponentActivity
import androidx.activity.compose.setContent
import androidx.compose.foundation.background
import androidx.compose.foundation.border
import androidx.compose.foundation.rememberScrollState
import androidx.compose.foundation.verticalScroll
import androidx.compose.foundation.gestures.awaitEachGesture
import androidx.compose.foundation.gestures.awaitFirstDown
import androidx.compose.foundation.gestures.detectDragGestures
import androidx.compose.foundation.gestures.detectTapGestures
import androidx.compose.foundation.layout.*
import androidx.compose.ui.layout.onSizeChanged
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.material3.*
import androidx.compose.runtime.*
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.draw.clip
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.input.pointer.pointerInput
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.unit.dp
import androidx.compose.ui.unit.sp
import kotlinx.coroutines.delay
import kotlin.math.roundToInt

class MainActivity : ComponentActivity() {

    private lateinit var steering: SteeringSensor
    private lateinit var store: ConfigStore
    private lateinit var haptics: HapticsEngine

    // Live input the sender thread reads each tick.
    @Volatile private var throttle = 0
    @Volatile private var brake = 0
    @Volatile private var clutch = 0
    @Volatile private var buttonsState = 0

    // For haptics: OR of the masks of buttons flagged as gear shifts, and whether
    // telemetry is expected to be flowing (i.e. we're connected).
    @Volatile private var shiftMask = 0
    @Volatile private var hapticsActive = false

    private var network: NetworkService? = null
    private var latestTelemetry by mutableStateOf(Protocol.Telemetry())

    override fun onCreate(savedInstanceState: Bundle?) {
        super.onCreate(savedInstanceState)
        window.addFlags(WindowManager.LayoutParams.FLAG_KEEP_SCREEN_ON)
        steering = SteeringSensor(this)
        store = ConfigStore(this)
        haptics = HapticsEngine(this) {
            val t = latestTelemetry
            HapticState(
                active = hapticsActive,
                rpm = t.rpm,
                gear = t.gear,
                clutch01 = clutch / 255f,
                throttle01 = throttle / 255f,
                shiftHeld = shiftMask != 0 && (buttonsState and shiftMask) != 0,
                slip = t.slip,
                impact = t.impact,
            )
        }

        setContent {
            MaterialTheme(colorScheme = darkColorScheme()) {
                Root()
            }
        }
    }

    override fun onResume() { super.onResume(); steering.start() }
    override fun onPause() { super.onPause(); steering.stop() }
    override fun onDestroy() { super.onDestroy(); network?.stop(); haptics.stop() }

    private fun applyTuning(c: Config) {
        steering.sensitivity = c.sensitivity
        steering.deadZone = c.deadZone
        steering.maxAngleRad = Math.toRadians(c.maxAngleDeg.toDouble()).toFloat()
        haptics.settings = c.hapticSettings()
    }

    private fun connect(ip: String) {
        network?.stop()
        network = NetworkService(
            serverIp = ip,
            inputProvider = {
                Protocol.Input(
                    steer = steering.steerShort(),
                    throttle = throttle,
                    brake = brake,
                    clutch = clutch,
                    buttons = buttonsState,
                )
            },
            onTelemetry = { latestTelemetry = it },
        ).also { it.start() }
        hapticsActive = true
        haptics.start()
    }

    private fun disconnect() {
        hapticsActive = false
        haptics.stop()
        network?.stop()
    }

    @Composable
    private fun Root() {
        var config by remember { mutableStateOf(store.load()) }
        val elements = remember { mutableStateListOf<Element>().apply { addAll(config.elements) } }
        var editMode by remember { mutableStateOf(false) }
        var connected by remember { mutableStateOf(false) }
        var selected by remember { mutableStateOf<Element?>(null) }
        var showSettings by remember { mutableStateOf(false) }
        var showPresets by remember { mutableStateOf(false) }
        var steerDisplay by remember { mutableStateOf(0f) }

        LaunchedEffect(config) { applyTuning(config) }
        LaunchedEffect(Unit) {
            while (true) { steerDisplay = steering.steerNormalized(); delay(33) }
        }
        // Keep the set of "shift" button masks current so grind feedback knows
        // which presses count as a gear change.
        LaunchedEffect(elements.toList()) {
            shiftMask = elements
                .filter { it.type == ControlType.BUTTON && it.shift }
                .fold(0) { acc, e -> acc or e.button }
        }

        fun persist() {
            config = config.copy(elements = elements.toList())
            store.save(config)
        }
        fun update(id: String, transform: (Element) -> Element) {
            val i = elements.indexOfFirst { it.id == id }
            if (i >= 0) elements[i] = transform(elements[i])
        }
        // Swap the live layout to a given element list (does not touch which
        // preset is "active").
        fun loadLayout(els: List<Element>) {
            elements.clear(); elements.addAll(els)
            config = config.copy(elements = elements.toList()); store.save(config)
        }
        // Apply a preset by name and remember it as the active one.
        fun applyPreset(name: String) {
            when (name) {
                "manual" -> loadLayout(Config.manualLayout())
                "automatic" -> loadLayout(Config.automaticLayout())
                else -> config.presets.firstOrNull { it.name == name }?.let { loadLayout(it.elements) }
            }
            config = config.copy(elements = elements.toList(), activePreset = name)
            store.save(config)
        }

        if (!connected) {
            StartScreen(
                ip = config.serverIp,
                onIpChange = { config = config.copy(serverIp = it); store.save(config) },
                onConnect = { connect(config.serverIp); connected = true },
            )
        } else Surface(Modifier.fillMaxSize(), color = Color(0xFF0E0E12)) {
            BoxWithConstraints(Modifier.fillMaxSize()) {
                val pxW = constraints.maxWidth.toFloat()
                val pxH = constraints.maxHeight.toFloat()
                val fullW = maxWidth
                val fullH = maxHeight

                // --- Elements ---
                elements.forEach { el ->
                    key(el.id) {
                        Box(
                            Modifier
                                .offset(x = fullW * el.x, y = fullH * el.y)
                                .size(width = fullW * el.w, height = fullH * el.h)
                                .then(
                                    if (editMode) Modifier
                                        .border(
                                            2.dp,
                                            if (selected?.id == el.id) Color(0xFF4C8DFF) else Color(0xFF55555F),
                                            RoundedCornerShape(8.dp),
                                        )
                                        .pointerInput(el.id) {
                                            detectTapGestures(onTap = { selected = el })
                                        }
                                        .pointerInput(el.id) {
                                            detectDragGestures { _, drag ->
                                                update(el.id) {
                                                    it.copy(
                                                        x = (it.x + drag.x / pxW).coerceIn(0f, 1f - it.w),
                                                        y = (it.y + drag.y / pxH).coerceIn(0f, 1f - it.h),
                                                    )
                                                }
                                                selected = elements.firstOrNull { it.id == el.id }
                                            }
                                        }
                                    else Modifier
                                ),
                            contentAlignment = Alignment.Center,
                        ) {
                            ElementContent(el, editMode, steerDisplay, config)

                            if (editMode) {
                                // Resize handle, bottom-right.
                                Box(
                                    Modifier
                                        .align(Alignment.BottomEnd)
                                        .size(26.dp)
                                        .clip(RoundedCornerShape(6.dp))
                                        .background(Color(0xFF4C8DFF))
                                        .pointerInput(el.id) {
                                            detectDragGestures { _, drag ->
                                                update(el.id) {
                                                    it.copy(
                                                        w = (it.w + drag.x / pxW).coerceIn(0.05f, 1f - it.x),
                                                        h = (it.h + drag.y / pxH).coerceIn(0.05f, 1f - it.y),
                                                    )
                                                }
                                            }
                                        },
                                )
                            }
                        }
                    }
                }

                // --- Top bar ---
                if (editMode) {
                    EditBar(
                        onAdd = { type -> elements.add(Element.newOf(type)); persist() },
                        onSettings = { showSettings = true },
                        onReset = {
                            elements.clear(); elements.addAll(Config.default().elements)
                            config = config.copy(elements = elements.toList()); store.save(config)
                        },
                        onDone = { persist(); editMode = false; selected = null },
                        modifier = Modifier.align(Alignment.TopCenter),
                    )
                } else {
                    MenuBar(
                        onPresets = { showPresets = true },
                        onEdit = { editMode = true },
                        onSettings = { showSettings = true },
                        onCenter = { steering.calibrate() },
                        onDisconnect = { disconnect(); connected = false },
                        modifier = Modifier.align(Alignment.TopStart),
                    )
                }

                // --- Selected element config (edit mode) ---
                selected?.let { sel ->
                    if (editMode) {
                        ElementConfigSheet(
                            element = sel,
                            onChange = { transform ->
                                update(sel.id, transform)
                                selected = elements.firstOrNull { it.id == sel.id }
                            },
                            onDelete = {
                                elements.removeAll { it.id == sel.id }; selected = null; persist()
                            },
                            onClose = { selected = null; persist() },
                            modifier = Modifier.align(Alignment.BottomCenter),
                        )
                    }
                }
            }
        }

        if (showSettings) {
            SettingsDialog(
                config = config,
                onApply = { config = it; store.save(it) },
                onClose = { showSettings = false },
            )
        }

        if (showPresets) {
            PresetsDialog(
                active = config.activePreset,
                custom = config.presets,
                onApply = { applyPreset(it) },
                onSaveCurrent = { name ->
                    val preset = LayoutPreset(name, elements.toList())
                    val others = config.presets.filterNot { it.name == name }
                    config = config.copy(presets = others + preset, activePreset = name)
                    store.save(config)
                },
                onDelete = { name ->
                    config = config.copy(
                        presets = config.presets.filterNot { it.name == name },
                        activePreset = if (config.activePreset == name) "manual" else config.activePreset,
                    )
                    store.save(config)
                },
                onClose = { showPresets = false },
            )
        }
    }

    /** Renders the live content of an element (drive mode wires up interaction). */
    @Composable
    private fun ElementContent(el: Element, editMode: Boolean, steerDisplay: Float, config: Config) {
        val t = latestTelemetry
        when (el.type) {
            ControlType.SPEEDOMETER -> Speedometer(t.speedKmh, config.maxSpeed, config.speedoDigital)
            ControlType.TACHOMETER -> {
                val auto = config.autoRpm && t.maxRpm > 100f
                val maxRpm = if (auto) t.maxRpm else config.maxRpm
                val redline = if (auto && t.redline > 100f) t.redline else config.redlineRpm
                Tachometer(t.rpm, maxRpm, redline, config.tachoDigital)
            }
            ControlType.GEAR_TEXT -> Readout(gearLabel(t.gear), "gear", big = true)
            ControlType.SPEED_TEXT -> Readout(t.speedKmh.roundToInt().toString(), "km/h", big = true)
            ControlType.STEERING_BAR -> SteeringBar(steerDisplay)
            ControlType.GAS -> PedalButton(el.label, Color(0xFF1F7A2E), enabled = !editMode) {
                throttle = if (it) 255 else 0
            }
            ControlType.BRAKE -> PedalButton(el.label, Color(0xFF7A1F1F), enabled = !editMode) {
                brake = if (it) 255 else 0
            }
            ControlType.THROTTLE_SLIDER -> VerticalAxis(el.label, Color(0xFF1F7A2E), enabled = !editMode) {
                throttle = it
            }
            ControlType.BRAKE_SLIDER -> VerticalAxis(el.label, Color(0xFF7A1F1F), enabled = !editMode) {
                brake = it
            }
            ControlType.CLUTCH_SLIDER -> VerticalAxis(el.label, Color(0xFF6A4AA8), enabled = !editMode) {
                clutch = it
            }
            ControlType.BUTTON -> XButton(el, enabled = !editMode)
        }
    }

    @Composable
    private fun XButton(el: Element, enabled: Boolean) {
        var toggled by remember(el.id) { mutableStateOf(false) }
        val active = toggled
        PedalButton(
            label = el.label.ifBlank { XInput.nameOf(el.button) },
            color = if (active) Color(0xFF3A5A8A) else Color(0xFF2A2A33),
            enabled = enabled,
        ) { pressed ->
            if (el.momentary) {
                buttonsState = if (pressed) buttonsState or el.button else buttonsState and el.button.inv()
            } else if (pressed) {
                toggled = !toggled
                buttonsState = if (toggled) buttonsState or el.button else buttonsState and el.button.inv()
            }
        }
    }
}

@Composable
private fun gearLabel(gear: Int) = when {
    gear <= 0 -> "R"
    gear == 1 -> "N"
    else -> (gear - 1).toString()
}

@Composable
private fun Readout(value: String, label: String, big: Boolean = false) {
    Column(horizontalAlignment = Alignment.CenterHorizontally) {
        Text(value, fontSize = if (big) 40.sp else 22.sp, fontWeight = FontWeight.Bold, color = Color.White)
        Text(label, fontSize = 11.sp, color = Color(0xFF8A8A95))
    }
}

@Composable
private fun SteeringBar(steer: Float) {
    Box(
        Modifier.fillMaxWidth().height(18.dp).clip(RoundedCornerShape(9.dp)).background(Color(0xFF1C1C24)),
    ) {
        BoxWithConstraints(Modifier.fillMaxSize()) {
            val half = maxWidth / 2
            Box(
                Modifier.align(Alignment.Center).offset(x = half * 0.96f * steer)
                    .size(width = 8.dp, height = 18.dp).clip(RoundedCornerShape(4.dp))
                    .background(Color(0xFF4C8DFF)),
            )
        }
    }
}

/** A press-and-hold button reporting pressed/released. */
@Composable
private fun PedalButton(
    label: String,
    color: Color,
    enabled: Boolean,
    onPressedChange: (Boolean) -> Unit,
) {
    Box(
        Modifier.fillMaxSize().clip(RoundedCornerShape(14.dp)).background(color)
            .then(
                if (enabled) Modifier.pointerInput(Unit) {
                    detectTapGestures(onPress = {
                        onPressedChange(true)
                        try { awaitRelease() } finally { onPressedChange(false) }
                    })
                } else Modifier
            ),
        contentAlignment = Alignment.Center,
    ) {
        Text(label, color = Color.White, fontWeight = FontWeight.Bold, fontSize = 16.sp)
    }
}

/**
 * A vertical analog pedal. Drag up to apply, lift to spring back to 0 — like a
 * real pedal. Reports 0..255. The fill height mirrors how far it's pressed.
 */
@Composable
private fun VerticalAxis(
    label: String,
    color: Color,
    enabled: Boolean,
    onValue: (Int) -> Unit,
) {
    var frac by remember { mutableStateOf(0f) }
    var heightPx by remember { mutableStateOf(1f) }

    Box(
        Modifier
            .fillMaxSize()
            .clip(RoundedCornerShape(14.dp))
            .background(Color(0xFF1C1C24))
            .onSizeChanged { heightPx = it.height.toFloat().coerceAtLeast(1f) }
            .then(
                if (enabled) Modifier.pointerInput(Unit) {
                    awaitEachGesture {
                        fun apply(y: Float) {
                            frac = ((heightPx - y) / heightPx).coerceIn(0f, 1f)
                            onValue((frac * 255).roundToInt())
                        }
                        val down = awaitFirstDown()
                        apply(down.position.y)
                        while (true) {
                            val event = awaitPointerEvent()
                            val change = event.changes.first()
                            if (!change.pressed) break
                            apply(change.position.y)
                        }
                        frac = 0f
                        onValue(0)
                    }
                } else Modifier
            ),
        contentAlignment = Alignment.BottomCenter,
    ) {
        // Fill rising from the bottom.
        Box(
            Modifier
                .fillMaxWidth()
                .fillMaxHeight(frac)
                .clip(RoundedCornerShape(14.dp))
                .background(color),
        )
        Text(
            label,
            color = Color.White,
            fontWeight = FontWeight.Bold,
            fontSize = 14.sp,
            modifier = Modifier.align(Alignment.TopCenter).padding(top = 8.dp),
        )
        Text(
            "${(frac * 100).roundToInt()}%",
            color = Color(0xFFB0B0B8),
            fontSize = 13.sp,
            modifier = Modifier.align(Alignment.Center),
        )
    }
}

/** Landing screen: type the PC's IP and connect. */
@Composable
private fun StartScreen(ip: String, onIpChange: (String) -> Unit, onConnect: () -> Unit) {
    Surface(Modifier.fillMaxSize(), color = Color(0xFF0E0E12)) {
        Column(
            Modifier.fillMaxSize().padding(24.dp),
            horizontalAlignment = Alignment.CenterHorizontally,
            verticalArrangement = Arrangement.Center,
        ) {
            Text("CorsaConnect", color = Color.White, fontSize = 34.sp, fontWeight = FontWeight.Bold)
            Text(
                "BeamNG steering wheel",
                color = Color(0xFF8A8A95),
                fontSize = 14.sp,
                modifier = Modifier.padding(bottom = 28.dp),
            )
            OutlinedTextField(
                value = ip,
                onValueChange = onIpChange,
                label = { Text("PC IP address") },
                singleLine = true,
                modifier = Modifier.width(240.dp),
            )
            Spacer(Modifier.height(16.dp))
            Button(
                onClick = onConnect,
                modifier = Modifier.width(240.dp).height(52.dp),
            ) {
                Text("Connect", fontSize = 18.sp, fontWeight = FontWeight.Bold)
            }
            Text(
                "Shown on the PC launcher.",
                color = Color(0xFF6A6A75),
                fontSize = 12.sp,
                modifier = Modifier.padding(top = 12.dp),
            )
        }
    }
}

/** The ≡ menu shown while driving: presets, edit, settings, center, disconnect. */
@Composable
private fun MenuBar(
    onPresets: () -> Unit,
    onEdit: () -> Unit,
    onSettings: () -> Unit,
    onCenter: () -> Unit,
    onDisconnect: () -> Unit,
    modifier: Modifier = Modifier,
) {
    var open by remember { mutableStateOf(false) }
    Box(modifier.padding(8.dp)) {
        Surface(
            color = Color(0xCC1C1C24),
            shape = RoundedCornerShape(10.dp),
            modifier = Modifier.size(46.dp).pointerInput(Unit) {
                detectTapGestures(onTap = { open = true })
            },
        ) {
            Box(contentAlignment = Alignment.Center) {
                Text("☰", color = Color.White, fontSize = 22.sp)
            }
        }
        DropdownMenu(expanded = open, onDismissRequest = { open = false }) {
            DropdownMenuItem(text = { Text("Presets") }, onClick = { open = false; onPresets() })
            DropdownMenuItem(text = { Text("Edit layout") }, onClick = { open = false; onEdit() })
            DropdownMenuItem(text = { Text("Settings") }, onClick = { open = false; onSettings() })
            DropdownMenuItem(text = { Text("Re-center wheel") }, onClick = { open = false; onCenter() })
            HorizontalDivider()
            DropdownMenuItem(
                text = { Text("Disconnect", color = Color(0xFFFF6B6B)) },
                onClick = { open = false; onDisconnect() },
            )
        }
    }
}

/** Pick / save / delete layout presets. */
@Composable
private fun PresetsDialog(
    active: String,
    custom: List<LayoutPreset>,
    onApply: (String) -> Unit,
    onSaveCurrent: (String) -> Unit,
    onDelete: (String) -> Unit,
    onClose: () -> Unit,
) {
    var newName by remember { mutableStateOf("") }
    AlertDialog(
        onDismissRequest = onClose,
        confirmButton = { Button(onClick = onClose) { Text("Done") } },
        title = { Text("Presets") },
        text = {
            Column(
                verticalArrangement = Arrangement.spacedBy(6.dp),
                modifier = Modifier.verticalScroll(rememberScrollState()),
            ) {
                PresetRow("Manual", active == "manual", onApply = { onApply("manual") })
                PresetRow("Automatic", active == "automatic", onApply = { onApply("automatic") })
                if (custom.isNotEmpty()) {
                    HorizontalDivider(Modifier.padding(vertical = 2.dp))
                    custom.forEach { p ->
                        PresetRow(p.name, active == p.name, onApply = { onApply(p.name) }, onDelete = { onDelete(p.name) })
                    }
                }
                HorizontalDivider(Modifier.padding(vertical = 2.dp))
                Text("Save current layout", fontSize = 12.sp, color = Color(0xFF8A8A95))
                Row(verticalAlignment = Alignment.CenterVertically) {
                    OutlinedTextField(
                        value = newName,
                        onValueChange = { newName = it },
                        label = { Text("Name", fontSize = 11.sp) },
                        singleLine = true,
                        modifier = Modifier.weight(1f),
                    )
                    Spacer(Modifier.width(8.dp))
                    Button(
                        onClick = { if (newName.isNotBlank()) { onSaveCurrent(newName.trim()); newName = "" } },
                        enabled = newName.isNotBlank(),
                    ) { Text("Save") }
                }
            }
        },
    )
}

@Composable
private fun PresetRow(
    name: String,
    isActive: Boolean,
    onApply: () -> Unit,
    onDelete: (() -> Unit)? = null,
) {
    Row(verticalAlignment = Alignment.CenterVertically) {
        TextButton(onClick = onApply, modifier = Modifier.weight(1f)) {
            Text(
                (if (isActive) "● " else "○ ") + name,
                color = if (isActive) Color(0xFF4C8DFF) else Color(0xFFD0D0D8),
                modifier = Modifier.fillMaxWidth(),
            )
        }
        if (onDelete != null) {
            TextButton(onClick = onDelete) { Text("✕", color = Color(0xFFFF6B6B)) }
        }
    }
}

@Composable
private fun EditBar(
    onAdd: (ControlType) -> Unit,
    onSettings: () -> Unit,
    onReset: () -> Unit,
    onDone: () -> Unit,
    modifier: Modifier = Modifier,
) {
    var addMenu by remember { mutableStateOf(false) }
    Row(
        modifier.fillMaxWidth().background(Color(0xCC16161C)).padding(8.dp),
        horizontalArrangement = Arrangement.spacedBy(8.dp),
        verticalAlignment = Alignment.CenterVertically,
    ) {
        Text("EDIT", color = Color(0xFF4C8DFF), fontWeight = FontWeight.Bold)
        Box {
            OutlinedButton(onClick = { addMenu = true }) { Text("+ Add") }
            DropdownMenu(expanded = addMenu, onDismissRequest = { addMenu = false }) {
                val items = listOf(
                    "Button" to ControlType.BUTTON,
                    "Throttle slider" to ControlType.THROTTLE_SLIDER,
                    "Brake slider" to ControlType.BRAKE_SLIDER,
                    "Clutch slider" to ControlType.CLUTCH_SLIDER,
                    "Speedometer" to ControlType.SPEEDOMETER,
                    "Tachometer" to ControlType.TACHOMETER,
                    "Gear" to ControlType.GEAR_TEXT,
                    "Speed text" to ControlType.SPEED_TEXT,
                    "Steering bar" to ControlType.STEERING_BAR,
                )
                items.forEach { (name, type) ->
                    DropdownMenuItem(text = { Text(name) }, onClick = { onAdd(type); addMenu = false })
                }
            }
        }
        OutlinedButton(onClick = onSettings) { Text("⚙ Settings") }
        OutlinedButton(onClick = onReset) { Text("Reset") }
        Spacer(Modifier.weight(1f))
        Button(onClick = onDone) { Text("Done") }
    }
}

@Composable
private fun ElementConfigSheet(
    element: Element,
    onChange: ((Element) -> Element) -> Unit,
    onDelete: () -> Unit,
    onClose: () -> Unit,
    modifier: Modifier = Modifier,
) {
    var bindMenu by remember { mutableStateOf(false) }
    Surface(
        modifier.fillMaxWidth(),
        color = Color(0xF015151B),
        shape = RoundedCornerShape(topStart = 14.dp, topEnd = 14.dp),
    ) {
        Row(
            Modifier.padding(12.dp),
            horizontalArrangement = Arrangement.spacedBy(8.dp),
            verticalAlignment = Alignment.CenterVertically,
        ) {
            Text(element.type.name, color = Color(0xFF8A8A95), fontSize = 12.sp)
            if (element.type == ControlType.BUTTON) {
                OutlinedTextField(
                    value = element.label,
                    onValueChange = { v -> onChange { it.copy(label = v) } },
                    label = { Text("Label", fontSize = 11.sp) },
                    singleLine = true,
                    modifier = Modifier.width(150.dp),
                )
                Box {
                    OutlinedButton(onClick = { bindMenu = true }) {
                        Text("Bind: ${XInput.nameOf(element.button)}")
                    }
                    DropdownMenu(expanded = bindMenu, onDismissRequest = { bindMenu = false }) {
                        XInput.named.forEach { (name, mask) ->
                            DropdownMenuItem(
                                text = { Text(name) },
                                onClick = { onChange { it.copy(button = mask) }; bindMenu = false },
                            )
                        }
                    }
                }
                FilterChip(
                    selected = !element.momentary,
                    onClick = { onChange { it.copy(momentary = !it.momentary) } },
                    label = { Text(if (element.momentary) "Hold" else "Toggle") },
                )
                FilterChip(
                    selected = element.shift,
                    onClick = { onChange { it.copy(shift = !it.shift) } },
                    label = { Text("Shift") },
                )
            }
            Spacer(Modifier.weight(1f))
            TextButton(onClick = onDelete) { Text("Delete", color = Color(0xFFFF6B6B)) }
            Button(onClick = onClose) { Text("OK") }
        }
    }
}

@Composable
private fun SettingsDialog(
    config: Config,
    onApply: (Config) -> Unit,
    onClose: () -> Unit,
) {
    var c by remember { mutableStateOf(config) }
    AlertDialog(
        onDismissRequest = onClose,
        confirmButton = { Button(onClick = { onApply(c); onClose() }) { Text("Save") } },
        dismissButton = { TextButton(onClick = onClose) { Text("Cancel") } },
        title = { Text("Settings") },
        text = {
            Column(
                verticalArrangement = Arrangement.spacedBy(4.dp),
                modifier = Modifier.verticalScroll(rememberScrollState()),
            ) {
                SliderRow("Sensitivity", c.sensitivity, 0.3f, 2.5f) { c = c.copy(sensitivity = it) }
                SliderRow("Dead zone", c.deadZone, 0f, 0.2f) { c = c.copy(deadZone = it) }
                SliderRow("Max steering angle°", c.maxAngleDeg, 30f, 180f) { c = c.copy(maxAngleDeg = it) }
                SliderRow("Speedo max km/h", c.maxSpeed, 100f, 400f) { c = c.copy(maxSpeed = it) }
                SwitchRow("Auto RPM (per car)", c.autoRpm) { c = c.copy(autoRpm = it) }
                if (!c.autoRpm) {
                    SliderRow("Tacho max rpm", c.maxRpm, 4000f, 12000f) { c = c.copy(maxRpm = it) }
                    SliderRow("Redline rpm", c.redlineRpm, 2000f, 12000f) { c = c.copy(redlineRpm = it) }
                }
                ToggleRow("Speedo", c.speedoDigital) { c = c.copy(speedoDigital = it) }
                ToggleRow("Tacho", c.tachoDigital) { c = c.copy(tachoDigital = it) }

                Spacer(Modifier.height(4.dp))
                Text("Force feedback", fontSize = 13.sp, fontWeight = FontWeight.Bold, color = Color(0xFF4C8DFF))
                SwitchRow("Vibration", c.haptics) { c = c.copy(haptics = it) }
                if (c.haptics) {
                    SliderRow("Intensity", c.hapticIntensity, 0f, 1f) { c = c.copy(hapticIntensity = it) }
                    SwitchRow("Ignition", c.hapticIgnition) { c = c.copy(hapticIgnition = it) }
                    SwitchRow("Gear shift", c.hapticShift) { c = c.copy(hapticShift = it) }
                    SwitchRow("Grind (no clutch)", c.hapticGrind) { c = c.copy(hapticGrind = it) }
                    SwitchRow("Drift / slide", c.hapticDrift) { c = c.copy(hapticDrift = it) }
                    SwitchRow("Collision", c.hapticCollision) { c = c.copy(hapticCollision = it) }
                }
            }
        },
    )
}

/** A labelled on/off switch row. */
@Composable
private fun SwitchRow(label: String, checked: Boolean, onChange: (Boolean) -> Unit) {
    Row(verticalAlignment = Alignment.CenterVertically) {
        Text(label, fontSize = 12.sp, color = Color(0xFFB0B0B8), modifier = Modifier.weight(1f))
        Switch(checked = checked, onCheckedChange = onChange)
    }
}

/** Analog vs digital chooser for a gauge. */
@Composable
private fun ToggleRow(label: String, digital: Boolean, onChange: (Boolean) -> Unit) {
    Row(verticalAlignment = Alignment.CenterVertically) {
        Text(label, fontSize = 12.sp, color = Color(0xFFB0B0B8), modifier = Modifier.width(70.dp))
        FilterChip(selected = !digital, onClick = { onChange(false) }, label = { Text("Analog") })
        Spacer(Modifier.width(8.dp))
        FilterChip(selected = digital, onClick = { onChange(true) }, label = { Text("Digital") })
    }
}

@Composable
private fun SliderRow(label: String, value: Float, min: Float, max: Float, onChange: (Float) -> Unit) {
    Column {
        Text("$label: ${"%.2f".format(value)}", fontSize = 12.sp, color = Color(0xFFB0B0B8))
        Slider(value = value, onValueChange = onChange, valueRange = min..max)
    }
}
