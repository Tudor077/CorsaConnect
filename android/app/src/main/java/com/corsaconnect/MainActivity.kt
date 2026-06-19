package com.corsaconnect

import android.os.Bundle
import android.view.WindowManager
import androidx.activity.ComponentActivity
import androidx.activity.compose.setContent
import androidx.compose.foundation.background
import androidx.compose.foundation.border
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

    // Live input the sender thread reads each tick.
    @Volatile private var throttle = 0
    @Volatile private var brake = 0
    @Volatile private var clutch = 0
    @Volatile private var buttonsState = 0

    private var network: NetworkService? = null
    private var latestTelemetry by mutableStateOf(Protocol.Telemetry())

    override fun onCreate(savedInstanceState: Bundle?) {
        super.onCreate(savedInstanceState)
        window.addFlags(WindowManager.LayoutParams.FLAG_KEEP_SCREEN_ON)
        steering = SteeringSensor(this)
        store = ConfigStore(this)

        setContent {
            MaterialTheme(colorScheme = darkColorScheme()) {
                Root()
            }
        }
    }

    override fun onResume() { super.onResume(); steering.start() }
    override fun onPause() { super.onPause(); steering.stop() }
    override fun onDestroy() { super.onDestroy(); network?.stop() }

    private fun applyTuning(c: Config) {
        steering.sensitivity = c.sensitivity
        steering.deadZone = c.deadZone
        steering.maxAngleRad = Math.toRadians(c.maxAngleDeg.toDouble()).toFloat()
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
    }

    @Composable
    private fun Root() {
        var config by remember { mutableStateOf(store.load()) }
        val elements = remember { mutableStateListOf<Element>().apply { addAll(config.elements) } }
        var editMode by remember { mutableStateOf(false) }
        var connected by remember { mutableStateOf(false) }
        var selected by remember { mutableStateOf<Element?>(null) }
        var showSettings by remember { mutableStateOf(false) }
        var steerDisplay by remember { mutableStateOf(0f) }

        LaunchedEffect(config) { applyTuning(config) }
        LaunchedEffect(Unit) {
            while (true) { steerDisplay = steering.steerNormalized(); delay(33) }
        }

        fun persist() {
            config = config.copy(elements = elements.toList())
            store.save(config)
        }
        fun update(id: String, transform: (Element) -> Element) {
            val i = elements.indexOfFirst { it.id == id }
            if (i >= 0) elements[i] = transform(elements[i])
        }

        Surface(Modifier.fillMaxSize(), color = Color(0xFF0E0E12)) {
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
                    DriveBar(
                        serverIp = config.serverIp,
                        onIpChange = { config = config.copy(serverIp = it); store.save(config) },
                        connected = connected,
                        onConnectToggle = {
                            if (connected) network?.stop() else connect(config.serverIp)
                            connected = !connected
                        },
                        onCalibrate = { steering.calibrate() },
                        onEdit = { editMode = true },
                        modifier = Modifier.align(Alignment.TopCenter),
                    )
                }

                // --- Selected element config (edit mode) ---
                selected?.let { sel ->
                    if (editMode) {
                        ElementConfigSheet(
                            element = sel,
                            onChange = { upd -> update(sel.id) { upd }; selected = upd },
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
    }

    /** Renders the live content of an element (drive mode wires up interaction). */
    @Composable
    private fun ElementContent(el: Element, editMode: Boolean, steerDisplay: Float, config: Config) {
        val t = latestTelemetry
        when (el.type) {
            ControlType.SPEEDOMETER -> Speedometer(t.speedKmh, config.maxSpeed, config.speedoDigital)
            ControlType.TACHOMETER -> Tachometer(t.rpm, config.maxRpm, config.redlineRpm, config.tachoDigital)
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

@Composable
private fun DriveBar(
    serverIp: String,
    onIpChange: (String) -> Unit,
    connected: Boolean,
    onConnectToggle: () -> Unit,
    onCalibrate: () -> Unit,
    onEdit: () -> Unit,
    modifier: Modifier = Modifier,
) {
    Row(
        modifier.fillMaxWidth().padding(8.dp),
        horizontalArrangement = Arrangement.spacedBy(8.dp),
        verticalAlignment = Alignment.CenterVertically,
    ) {
        OutlinedTextField(
            value = serverIp,
            onValueChange = onIpChange,
            label = { Text("IP PC", fontSize = 11.sp) },
            singleLine = true,
            enabled = !connected,
            modifier = Modifier.width(170.dp),
        )
        Button(onClick = onConnectToggle) { Text(if (connected) "Stop" else "Connect") }
        OutlinedButton(onClick = onCalibrate) { Text("Center") }
        Spacer(Modifier.weight(1f))
        OutlinedButton(onClick = onEdit) { Text("✎ Edit") }
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
                    "Gas (button)" to ControlType.GAS,
                    "Brake (button)" to ControlType.BRAKE,
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
    onChange: (Element) -> Unit,
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
                    onValueChange = { onChange(element.copy(label = it)) },
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
                                onClick = { onChange(element.copy(button = mask)); bindMenu = false },
                            )
                        }
                    }
                }
                FilterChip(
                    selected = !element.momentary,
                    onClick = { onChange(element.copy(momentary = !element.momentary)) },
                    label = { Text(if (element.momentary) "Hold" else "Toggle") },
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
            Column(verticalArrangement = Arrangement.spacedBy(4.dp)) {
                SliderRow("Sensitivity", c.sensitivity, 0.3f, 2.5f) { c = c.copy(sensitivity = it) }
                SliderRow("Dead zone", c.deadZone, 0f, 0.2f) { c = c.copy(deadZone = it) }
                SliderRow("Max steering angle°", c.maxAngleDeg, 30f, 180f) { c = c.copy(maxAngleDeg = it) }
                SliderRow("Speedo max km/h", c.maxSpeed, 100f, 400f) { c = c.copy(maxSpeed = it) }
                SliderRow("Tacho max rpm", c.maxRpm, 4000f, 12000f) { c = c.copy(maxRpm = it) }
                SliderRow("Redline rpm", c.redlineRpm, 2000f, 12000f) { c = c.copy(redlineRpm = it) }
                ToggleRow("Speedo", c.speedoDigital) { c = c.copy(speedoDigital = it) }
                ToggleRow("Tacho", c.tachoDigital) { c = c.copy(tachoDigital = it) }
            }
        },
    )
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
