//! The actual UDP server: virtual pad + BeamNG telemetry relay.
//!
//! Pulled out of `main` so the GUI launcher can start and stop it on a button,
//! feed its log into a panel, and show live status (ViGEm / phone / BeamNG).

use std::collections::VecDeque;
use std::net::{IpAddr, Ipv4Addr, SocketAddr, UdpSocket};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use crate::motionsim;
use crate::outgauge;
use crate::protocol::{InputPacket, TelemetryPacket};
use crate::scstelemetry;
use vigem_client::{Client, TargetId, XButtons, XGamepad, Xbox360Wired};

/// Port the phone sends controller input to.
const INPUT_PORT: u16 = 5000;
/// Port BeamNG's OutGauge protocol targets (must match the in-game setting).
const OUTGAUGE_PORT: u16 = 4444;
/// Port BeamNG's MotionSim/OutSim protocol targets (slide + crash physics).
const MOTIONSIM_PORT: u16 = 4445;
/// How long a crash spike keeps fading after it lands.
const IMPACT_DECAY: Duration = Duration::from_millis(350);
/// Port on the phone that listens for telemetry.
const PHONE_TELEMETRY_PORT: u16 = 5001;
/// Recenter the pad if the phone goes quiet for this long; also the poll
/// interval at which the loops notice a stop request.
const INPUT_TIMEOUT: Duration = Duration::from_millis(250);

/// Live status the GUI reads each frame.
#[derive(Default, Clone)]
pub struct Status {
    pub vigem_ok: bool,
    pub phone: Option<IpAddr>,
    pub beamng: bool,
    pub motion: bool, // MotionSim packets are flowing (slide + crash feedback)
    pub last: Option<(f32, f32, i8)>, // speed km/h, rpm, gear
    pub error: Option<String>,
}

/// Latest physics derived from MotionSim, shared with the telemetry relay.
struct MotionState {
    slip: f32,
    impact: f32,
    impact_at: Instant,
}

/// Shared between the server threads and the GUI.
pub struct Shared {
    logs: Mutex<VecDeque<String>>,
    pub status: Mutex<Status>,
    motion: Mutex<MotionState>,
}

impl Shared {
    pub fn new() -> Arc<Self> {
        Arc::new(Shared {
            logs: Mutex::new(VecDeque::new()),
            status: Mutex::new(Status::default()),
            motion: Mutex::new(MotionState {
                slip: 0.0,
                impact: 0.0,
                impact_at: Instant::now(),
            }),
        })
    }

    /// Feed new physics: slip is instantaneous; impact latches its peak so a
    /// brief spike between relay sends isn't lost, then fades via [IMPACT_DECAY].
    fn update_motion(&self, slip: f32, impact: f32) {
        let mut m = self.motion.lock().unwrap();
        m.slip = slip;
        let now = Instant::now();
        if impact > decay_impact(m.impact, m.impact_at, now) {
            m.impact = impact;
            m.impact_at = now;
        }
    }

    /// (slip, decayed impact) for the next telemetry packet.
    fn motion_snapshot(&self) -> (f32, f32) {
        let m = self.motion.lock().unwrap();
        (m.slip, decay_impact(m.impact, m.impact_at, Instant::now()))
    }

    pub fn log(&self, msg: impl Into<String>) {
        let mut l = self.logs.lock().unwrap();
        l.push_back(msg.into());
        while l.len() > 300 {
            l.pop_front();
        }
    }

    /// Snapshot of recent log lines for display.
    pub fn log_lines(&self) -> Vec<String> {
        self.logs.lock().unwrap().iter().cloned().collect()
    }

    pub fn status(&self) -> Status {
        self.status.lock().unwrap().clone()
    }

    fn set_status(&self, f: impl FnOnce(&mut Status)) {
        f(&mut self.status.lock().unwrap());
    }
}

/// Linear fade of an impact peak to 0 over [IMPACT_DECAY].
fn decay_impact(impact: f32, at: Instant, now: Instant) -> f32 {
    let elapsed = now.saturating_duration_since(at).as_secs_f32();
    let span = IMPACT_DECAY.as_secs_f32();
    if elapsed >= span {
        0.0
    } else {
        impact * (1.0 - elapsed / span)
    }
}

/// Best-effort primary LAN IPv4. Picks the source address the OS would use to
/// reach the internet; no packets are actually sent. `None` if offline.
pub fn local_ipv4() -> Option<Ipv4Addr> {
    let sock = UdpSocket::bind(("0.0.0.0", 0)).ok()?;
    sock.connect(("8.8.8.8", 80)).ok()?;
    match sock.local_addr().ok()? {
        SocketAddr::V4(a) => Some(*a.ip()),
        _ => None,
    }
}

/// The game we're feeding. The virtual controller (steering, pedals, buttons)
/// works for every game; the choice only decides where dashboard + force
/// feedback telemetry is read from.
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Game {
    BeamNg,
    TruckSim, // ETS2 / ATS
    Wrc10,
}

impl Game {
    pub const ALL: [Game; 3] = [Game::BeamNg, Game::TruckSim, Game::Wrc10];

    pub fn name(self) -> &'static str {
        match self {
            Game::BeamNg => "BeamNG.drive",
            Game::TruckSim => "Euro / American Truck Sim",
            Game::Wrc10 => "WRC 10",
        }
    }

    /// One-line setup hint shown under the picker.
    pub fn hint(self) -> &'static str {
        match self {
            Game::BeamNg => {
                "Options > Other > Protocols: enable OutGauge (127.0.0.1:4444) and \
                 MotionSim/OutSim (4445)."
            }
            Game::TruckSim => {
                "Drop the scs-sdk-plugin DLL into the game's bin\\win_x64\\plugins folder. \
                 (Dashboard telemetry coming soon; the wheel already works.)"
            }
            Game::Wrc10 => {
                "Dashboard telemetry coming soon; steering and pedals already work via the \
                 virtual controller."
            }
        }
    }
}

/// Run the server until `stop` is set. Reports progress/errors via `shared`.
/// Returns when the input loop ends (stop requested or a fatal error).
pub fn run(shared: Arc<Shared>, stop: Arc<AtomicBool>, game: Game) {
    shared.set_status(|s| {
        *s = Status::default();
    });
    shared.log("Starting CorsaConnect server...");

    // Virtual Xbox 360 pad via ViGEmBus.
    let client = match Client::connect() {
        Ok(c) => c,
        Err(e) => {
            let msg = format!(
                "Could not connect to ViGEmBus ({e}). Install the ViGEmBus driver, then Launch again."
            );
            shared.log(&msg);
            shared.set_status(|s| s.error = Some(msg));
            return;
        }
    };
    let mut pad = Xbox360Wired::new(client, TargetId::default());
    if let Err(e) = pad.plugin().and_then(|_| pad.wait_ready()) {
        let msg = format!("Virtual controller failed to start: {e}");
        shared.log(&msg);
        shared.set_status(|s| s.error = Some(msg));
        return;
    }
    shared.set_status(|s| s.vigem_ok = true);
    shared.log("Virtual Xbox 360 controller plugged in.");

    // Where to send telemetry, learned from the phone's first input packet.
    let phone_addr: Arc<Mutex<Option<SocketAddr>>> = Arc::new(Mutex::new(None));

    // Telemetry (dashboard + force feedback) per game. The virtual controller
    // works everywhere; only the telemetry source differs.
    let mut telem: Vec<std::thread::JoinHandle<()>> = Vec::new();
    match game {
        Game::BeamNg => {
            telem.push({
                let shared = Arc::clone(&shared);
                let stop = Arc::clone(&stop);
                let phone_addr = Arc::clone(&phone_addr);
                std::thread::spawn(move || telemetry_relay(shared, stop, phone_addr))
            });
            telem.push({
                let shared = Arc::clone(&shared);
                let stop = Arc::clone(&stop);
                std::thread::spawn(move || motion_listener(shared, stop))
            });
        }
        Game::TruckSim => {
            telem.push({
                let shared = Arc::clone(&shared);
                let stop = Arc::clone(&stop);
                let phone_addr = Arc::clone(&phone_addr);
                std::thread::spawn(move || truck_telemetry(shared, stop, phone_addr))
            });
        }
        Game::Wrc10 => {
            shared.log(format!(
                "{}: wheel, pedals and buttons are live. Dashboard telemetry isn't wired for this game yet.",
                game.name()
            ));
        }
    }

    if let Err(e) = input_loop(&shared, &stop, &mut pad, &phone_addr) {
        let msg = format!("Input listener stopped: {e}");
        shared.log(&msg);
        shared.set_status(|s| s.error = Some(msg));
    }

    for h in telem {
        let _ = h.join();
    }
    // Dropping `pad` here unplugs the virtual controller.
    shared.set_status(|s| {
        s.vigem_ok = false;
        s.phone = None;
        s.beamng = false;
        s.motion = false;
    });
    shared.log("Server stopped.");
}

fn input_loop(
    shared: &Arc<Shared>,
    stop: &Arc<AtomicBool>,
    pad: &mut Xbox360Wired<Client>,
    phone_addr: &Arc<Mutex<Option<SocketAddr>>>,
) -> std::io::Result<()> {
    let sock = UdpSocket::bind(("0.0.0.0", INPUT_PORT))?;
    sock.set_read_timeout(Some(INPUT_TIMEOUT))?;
    shared.log(format!("Listening for phone input on UDP :{INPUT_PORT}"));

    let mut buf = [0u8; 64];
    while !stop.load(Ordering::Relaxed) {
        match sock.recv_from(&mut buf) {
            Ok((n, src)) => {
                if let Some(input) = InputPacket::parse(&buf[..n]) {
                    let _ = pad.update(&to_gamepad(&input));
                    let mut slot = phone_addr.lock().unwrap();
                    if slot.map(|a| a.ip()) != Some(src.ip()) {
                        *slot = Some(SocketAddr::new(src.ip(), PHONE_TELEMETRY_PORT));
                        shared.log(format!("Phone connected from {}", src.ip()));
                        shared.set_status(|s| s.phone = Some(src.ip()));
                    }
                }
            }
            Err(ref e)
                if e.kind() == std::io::ErrorKind::WouldBlock
                    || e.kind() == std::io::ErrorKind::TimedOut =>
            {
                // No input: recenter so the car doesn't run away.
                let _ = pad.update(&XGamepad::default());
            }
            Err(e) => return Err(e),
        }
    }
    Ok(())
}

fn to_gamepad(input: &InputPacket) -> XGamepad {
    let clutch_axis = (input.clutch as i32 * i16::MAX as i32 / 255) as i16;
    XGamepad {
        buttons: XButtons { raw: input.buttons },
        thumb_lx: input.steer,
        thumb_ly: 0,
        thumb_rx: 0,
        thumb_ry: clutch_axis,
        left_trigger: input.brake,
        right_trigger: input.throttle,
    }
}

fn telemetry_relay(
    shared: Arc<Shared>,
    stop: Arc<AtomicBool>,
    phone_addr: Arc<Mutex<Option<SocketAddr>>>,
) {
    let sock = match UdpSocket::bind(("0.0.0.0", OUTGAUGE_PORT)) {
        Ok(s) => s,
        Err(e) => {
            shared.log(format!("Could not bind OutGauge port {OUTGAUGE_PORT}: {e}"));
            return;
        }
    };
    if sock.set_read_timeout(Some(INPUT_TIMEOUT)).is_err() {
        shared.log("Could not set OutGauge read timeout.");
        return;
    }
    let tx = match UdpSocket::bind(("0.0.0.0", 0)) {
        Ok(s) => s,
        Err(e) => {
            shared.log(format!("Could not open telemetry sender socket: {e}"));
            return;
        }
    };
    shared.log(format!("Listening for BeamNG OutGauge on UDP :{OUTGAUGE_PORT}"));

    let mut buf = [0u8; 128];
    let mut announced = false;
    // Redline = the rev limiter, found as the rpm where flat-out the engine stops
    // climbing (the rev cut). Until then the tach stays large with no red zone, so
    // a fresh car looks normal at idle instead of pinning the needle to a tiny
    // redline; once the limiter is learned the gauge shrinks down to it.
    // BeamNG's OutGauge has no per-car id (`car` is always "beam") and never sets
    // the shift-light bit, so a pause in the stream is our only "car changed" cue.
    let mut peak_rpm = 0.0f32;
    let mut frames_since_peak = 0u32;
    let mut limiter = 0.0f32;
    let mut idle_ticks = 0u32;
    while !stop.load(Ordering::Relaxed) {
        let (n, _) = match sock.recv_from(&mut buf) {
            Ok(v) => v,
            Err(ref e)
                if e.kind() == std::io::ErrorKind::WouldBlock
                    || e.kind() == std::io::ErrorKind::TimedOut =>
            {
                idle_ticks += 1;
                continue;
            }
            Err(_) => continue,
        };
        let gap = idle_ticks;
        idle_ticks = 0;
        let Some(mut tel) = outgauge::parse(&buf[..n]) else {
            if !announced {
                shared.log(format!(
                    "Receiving UDP on :{OUTGAUGE_PORT} but {n} bytes isn't OutGauge (is MotionSim on instead of OutGauge?)"
                ));
                announced = true;
            }
            continue;
        };
        if !announced {
            shared.log("OutGauge: data flowing from BeamNG.");
            announced = true;
        }
        shared.set_status(|s| {
            s.beamng = true;
            s.last = Some((tel.speed_kmh, tel.rpm, tel.gear));
        });
        // Fold in the latest physics (0 if MotionSim isn't enabled in BeamNG).
        let (slip, impact) = shared.motion_snapshot();
        tel.slip = slip;
        tel.impact = impact;

        // A long pause in the stream (radial menu, vehicle spawn, world reload)
        // usually means a possible car change - relearn the rev range. gap counts
        // 250 ms read timeouts, so >=6 is ~1.5 s of silence.
        if gap >= 6 && (peak_rpm > 0.0 || limiter > 0.0) {
            peak_rpm = 0.0;
            limiter = 0.0;
            frames_since_peak = 0;
            shared.log("Telemetry resumed - relearning rev range.");
        }

        // Track the peak rpm and how long since it last rose.
        if tel.rpm > peak_rpm {
            peak_rpm = tel.rpm;
            frames_since_peak = 0;
            if peak_rpm > limiter + 100.0 {
                limiter = 0.0; // revved past the supposed limiter -> relearn higher
            }
        } else {
            frames_since_peak = frames_since_peak.saturating_add(1);
        }
        // Rev limiter: flat-out, high rpm, and the peak hasn't moved for a moment.
        if limiter == 0.0 && tel.throttle > 0.85 && peak_rpm > 3000.0 && frames_since_peak >= 30 {
            limiter = peak_rpm;
            shared.log(format!("Redline learned: {:.0} rpm.", limiter));
        }

        // Big gauge with no red until the limiter is known; then shrink to it.
        if limiter > 0.0 {
            tel.redline = limiter;
            tel.max_rpm = limiter * 1.08;
        } else {
            tel.max_rpm = 9000.0f32.max(peak_rpm * 1.05);
            tel.redline = tel.max_rpm;
        }
        if let Some(addr) = *phone_addr.lock().unwrap() {
            let _ = tx.send_to(&tel.encode(), addr);
        }
    }
}

/// Reads ETS2 / ATS telemetry from the scs-sdk-plugin shared memory and relays
/// speed / rpm / gear to the phone.
fn truck_telemetry(
    shared: Arc<Shared>,
    stop: Arc<AtomicBool>,
    phone_addr: Arc<Mutex<Option<SocketAddr>>>,
) {
    let tx = match UdpSocket::bind(("0.0.0.0", 0)) {
        Ok(s) => s,
        Err(e) => {
            shared.log(format!("Could not open telemetry sender socket: {e}"));
            return;
        }
    };
    shared.log("Looking for ETS2/ATS telemetry (install the scs-sdk-plugin DLL)...");

    // Wait for the shared memory to appear (game + plugin running).
    let scs = loop {
        if stop.load(Ordering::Relaxed) {
            return;
        }
        if let Some(s) = scstelemetry::ScsShared::open() {
            break s;
        }
        std::thread::sleep(Duration::from_millis(500));
    };
    shared.log("Truck telemetry connected.");
    shared.set_status(|s| s.beamng = true);

    while !stop.load(Ordering::Relaxed) {
        let mut tel = TelemetryPacket {
            speed_kmh: scs.speed_ms() * 3.6,
            rpm: scs.rpm(),
            // SCS gear: 1 = 1st, 0 = neutral, -1 = reverse. Ours: 0 = R, 1 = N, 2 = 1st.
            gear: (scs.gear() + 1) as i8,
            fuel: scs.fuel_frac(),
            engine_temp: scs.water_temp(),
            show_lights: scs.show_lights(),
            ..Default::default()
        };
        // Sensible truck rev range until we read the real one from the plugin.
        tel.max_rpm = 3000.0;
        tel.redline = 2600.0;

        shared.set_status(|s| s.last = Some((tel.speed_kmh, tel.rpm, tel.gear)));
        if let Some(addr) = *phone_addr.lock().unwrap() {
            let _ = tx.send_to(&tel.encode(), addr);
        }
        std::thread::sleep(Duration::from_millis(16));
    }
}

/// Listens for BeamNG MotionSim packets and turns them into slide + crash
/// strength for the telemetry relay to forward.
fn motion_listener(shared: Arc<Shared>, stop: Arc<AtomicBool>) {
    let sock = match UdpSocket::bind(("0.0.0.0", MOTIONSIM_PORT)) {
        Ok(s) => s,
        Err(e) => {
            shared.log(format!("Could not bind MotionSim port {MOTIONSIM_PORT}: {e}"));
            return;
        }
    };
    if sock.set_read_timeout(Some(INPUT_TIMEOUT)).is_err() {
        shared.log("Could not set MotionSim read timeout.");
        return;
    }
    shared.log(format!(
        "Listening for BeamNG MotionSim on UDP :{MOTIONSIM_PORT} (slide + crash)"
    ));

    let mut buf = [0u8; 128];
    let mut announced = false;
    let mut slip_est = motionsim::SlipEstimator::new();
    while !stop.load(Ordering::Relaxed) {
        let (n, _) = match sock.recv_from(&mut buf) {
            Ok(v) => v,
            Err(ref e)
                if e.kind() == std::io::ErrorKind::WouldBlock
                    || e.kind() == std::io::ErrorKind::TimedOut =>
            {
                continue;
            }
            Err(_) => continue,
        };
        let Some(m) = motionsim::parse(&buf[..n]) else {
            continue;
        };
        if !announced {
            shared.log("MotionSim: data flowing from BeamNG.");
            shared.set_status(|s| s.motion = true);
            announced = true;
        }
        let slip = slip_est.update(&m);
        shared.update_motion(slip, motionsim::impact_fraction(&m));
    }
}
