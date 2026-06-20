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
use crate::protocol::InputPacket;
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

/// Run the server until `stop` is set. Reports progress/errors via `shared`.
/// Returns when the input loop ends (stop requested or a fatal error).
pub fn run(shared: Arc<Shared>, stop: Arc<AtomicBool>) {
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

    // OutGauge -> phone relay thread.
    let relay = {
        let shared = Arc::clone(&shared);
        let stop = Arc::clone(&stop);
        let phone_addr = Arc::clone(&phone_addr);
        std::thread::spawn(move || telemetry_relay(shared, stop, phone_addr))
    };

    // MotionSim listener: derives slide + crash and feeds the relay.
    let motion = {
        let shared = Arc::clone(&shared);
        let stop = Arc::clone(&stop);
        std::thread::spawn(move || motion_listener(shared, stop))
    };

    if let Err(e) = input_loop(&shared, &stop, &mut pad, &phone_addr) {
        let msg = format!("Input listener stopped: {e}");
        shared.log(&msg);
        shared.set_status(|s| s.error = Some(msg));
    }

    let _ = relay.join();
    let _ = motion.join();
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
    // Learned per car: the highest rpm seen (~the rev limiter / redline).
    let mut peak_rpm = 0.0f32;
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

        // Learn the car's rev range so the tach can auto-fit. The redline (red
        // zone) starts at the rev limiter, i.e. the highest rpm the car reaches;
        // the gauge gets a little headroom above so the needle can swing into it.
        if tel.rpm > peak_rpm {
            peak_rpm = tel.rpm;
        }
        if peak_rpm > 1000.0 {
            tel.redline = peak_rpm;
            tel.max_rpm = peak_rpm * 1.08;
        }
        if let Some(addr) = *phone_addr.lock().unwrap() {
            let _ = tx.send_to(&tel.encode(), addr);
        }
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
