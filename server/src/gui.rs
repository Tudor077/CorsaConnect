//! The launcher window: shows the PC's IP and a Launch button, plus live status
//! and a log panel. Built on eframe/egui so it ships as a single .exe.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use eframe::egui::{self, Color32, FontId, RichText};

use crate::headtracker;
use crate::picopanel;
use crate::prefs::Prefs;
use crate::server::{self, DeviceMode, Game, Shared};
use crate::vjoy;

const BG: Color32 = Color32::from_rgb(14, 14, 18);
const CARD: Color32 = Color32::from_rgb(24, 24, 31);
const ACCENT: Color32 = Color32::from_rgb(76, 141, 255);
const RED: Color32 = Color32::from_rgb(196, 22, 28);
const AMBER: Color32 = Color32::from_rgb(230, 168, 60);
const GREEN: Color32 = Color32::from_rgb(60, 200, 110);
const MUTED: Color32 = Color32::from_rgb(138, 138, 149);

pub fn run() -> eframe::Result<()> {
    let mut viewport = egui::ViewportBuilder::default()
        .with_inner_size([480.0, 720.0])
        .with_min_inner_size([420.0, 520.0])
        .with_title("CorsaConnect");
    if let Some(icon) = load_icon() {
        viewport = viewport.with_icon(icon);
    }
    let options = eframe::NativeOptions {
        viewport,
        ..Default::default()
    };
    eframe::run_native(
        "CorsaConnect",
        options,
        Box::new(|cc| {
            style(&cc.egui_ctx);
            Ok(Box::new(App::new()))
        }),
    )
}

fn load_icon() -> Option<egui::IconData> {
    let bytes = include_bytes!("../assets/icon_256.png");
    let img = image::load_from_memory(bytes).ok()?.into_rgba8();
    let (w, h) = img.dimensions();
    Some(egui::IconData {
        rgba: img.into_raw(),
        width: w,
        height: h,
    })
}

fn style(ctx: &egui::Context) {
    let mut visuals = egui::Visuals::dark();
    visuals.panel_fill = BG;
    visuals.window_fill = BG;
    visuals.override_text_color = Some(Color32::from_rgb(235, 235, 240));
    ctx.set_visuals(visuals);
}

struct App {
    shared: Arc<Shared>,
    stop: Arc<AtomicBool>,
    running: bool,
    ip: String,
    game: Game,
    device: DeviceMode,
    /// vJoy's version + configuration, or why it can't be used. Probed at startup.
    vjoy: Result<vjoy::Probe, String>,
    head: HeadUi,
    pico: PicoUi,
}

/// GUI-side state for the PicoPanel card. See [crate::picopanel].
struct PicoUi {
    /// Mirror telemetry to PicoPanel. Mirrored straight into [Shared] so the
    /// toggle works while the server is running.
    on: bool,
    /// Where to send it, as typed. Kept as text so a half-finished edit doesn't
    /// throw the setting away; only a parsable address is applied.
    addr: String,
    exe: Option<std::path::PathBuf>,
    /// The instance we started, if we started one. Dropping it doesn't kill the
    /// process, which is what we want: closing the launcher shouldn't yank the
    /// panel out from under a race.
    child: Option<std::process::Child>,
    /// Result of the last button press, shown under the buttons.
    note: Option<(String, bool)>, // (message, is_error)
    /// Cached so we don't shell out to tasklist on every repaint.
    seen_running: bool,
    last_poll: std::time::Instant,
}

impl PicoUi {
    fn new(prefs: &Prefs) -> PicoUi {
        PicoUi {
            on: prefs.bool("picopanel.mirror", false),
            addr: prefs
                .get("picopanel.addr")
                .unwrap_or(picopanel::DEFAULT_MIRROR)
                .to_string(),
            exe: prefs
                .get("picopanel.exe")
                .map(std::path::PathBuf::from)
                .filter(|p| p.is_file())
                .or_else(picopanel::find_exe),
            child: None,
            note: None,
            seen_running: false,
            last_poll: std::time::Instant::now() - Duration::from_secs(5),
        }
    }

    /// The typed address, if it's a valid one.
    fn parsed(&self) -> Option<std::net::SocketAddr> {
        self.addr.trim().parse().ok()
    }
}

/// GUI-side state for the head tracking card.
struct HeadUi {
    stop: Arc<AtomicBool>,
    running: bool,
    settings: Arc<Mutex<headtracker::Settings>>,
    cameras: Vec<(u32, String)>,
    show_preview: bool,
    texture: Option<egui::TextureHandle>,
}

impl HeadUi {
    fn new() -> HeadUi {
        HeadUi {
            stop: Arc::new(AtomicBool::new(false)),
            running: false,
            settings: Arc::new(Mutex::new(headtracker::Settings::default())),
            cameras: headtracker::list_cameras(),
            show_preview: false,
            texture: None,
        }
    }
}

impl App {
    fn new() -> Self {
        let vjoy = vjoy::probe();
        let prefs = Prefs::load();
        let pico = PicoUi::new(&prefs);
        let shared = Shared::new();
        // A saved "on" beats the environment variable Shared started with, so
        // the checkbox you left ticked is still ticked next time.
        if pico.on {
            if let Some(a) = pico.parsed() {
                shared.set_mirror(Some(a));
            }
        } else if prefs.get("picopanel.mirror").is_some() {
            shared.set_mirror(None);
        }
        App {
            shared,
            stop: Arc::new(AtomicBool::new(false)),
            running: false,
            ip: server::local_ipv4()
                .map(|a| a.to_string())
                .unwrap_or_else(|| "not on a network".to_string()),
            game: Game::BeamNg,
            // Prefer the wheel; fall back to the Xbox pad if vJoy isn't there,
            // so a fresh install still works on the first Launch.
            device: if vjoy.is_ok() {
                DeviceMode::Wheel
            } else {
                DeviceMode::Xbox360
            },
            vjoy,
            head: HeadUi::new(),
            pico,
        }
    }

    /// Push the card's choices into the running server and onto disk.
    fn apply_pico(&mut self) {
        let addr = self.pico.parsed();
        self.shared
            .set_mirror(if self.pico.on { addr } else { None });
        let mut prefs = Prefs::load();
        prefs.set_bool("picopanel.mirror", self.pico.on);
        prefs.set("picopanel.addr", self.pico.addr.trim());
        if let Some(exe) = &self.pico.exe {
            prefs.set("picopanel.exe", exe.display().to_string());
        }
        prefs.save();
    }

    fn launch(&mut self) {
        self.stop.store(false, Ordering::Relaxed);
        let shared = Arc::clone(&self.shared);
        let stop = Arc::clone(&self.stop);
        let game = self.game;
        let device = self.device;
        std::thread::spawn(move || server::run(shared, stop, game, device));
        self.running = true;
    }

    fn stop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
        self.running = false;
    }

    fn start_head(&mut self) {
        self.head.stop.store(false, Ordering::Relaxed);
        let shared = Arc::clone(&self.shared);
        let stop = Arc::clone(&self.head.stop);
        let settings = Arc::clone(&self.head.settings);
        std::thread::spawn(move || headtracker::run(shared, stop, settings));
        self.head.running = true;
    }

    fn stop_head(&mut self) {
        self.head.stop.store(true, Ordering::Relaxed);
        self.head.running = false;
    }
}

impl eframe::App for App {
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        // Keep status/log fresh while running; repaint fast while the head
        // tracking preview is live so it doesn't look like a slideshow.
        let repaint = if self.head.running && self.head.show_preview {
            Duration::from_millis(33)
        } else {
            Duration::from_millis(200)
        };
        ctx.request_repaint_after(repaint);

        let status = self.shared.status();
        // A fatal error in the server thread flips us back to stopped.
        if self.running && status.error.is_some() && !status.device_ok {
            self.running = false;
        }
        // Same for the head tracking thread.
        {
            let head = self.shared.head.status.lock().unwrap().clone();
            if self.head.running && !head.running && head.error.is_some() {
                self.head.running = false;
            }
        }

        egui::CentralPanel::default().show(ctx, |ui| {
          egui::ScrollArea::vertical()
            .auto_shrink([false, false])
            .show(ui, |ui| {
            ui.add_space(14.0);
            ui.vertical_centered(|ui| {
                ui.label(RichText::new("CorsaConnect").size(26.0).strong());
                ui.label(
                    RichText::new("Phone steering wheel + telemetry server")
                        .size(13.0)
                        .color(MUTED),
                );
            });
            ui.add_space(14.0);

            // --- IP card ---
            frame_card(ui, |ui| {
                ui.label(RichText::new("PC IP ADDRESS").size(12.0).color(MUTED));
                ui.add_space(2.0);
                ui.label(
                    RichText::new(&self.ip)
                        .font(FontId::monospace(34.0))
                        .color(ACCENT)
                        .strong(),
                );
                ui.add_space(4.0);
                ui.label(
                    RichText::new(
                        "After Launch the phone finds this PC by itself; no firewall rule needed.",
                    )
                        .size(12.0)
                        .color(MUTED),
                );
            });

            ui.add_space(12.0);

            // --- Game picker ---
            frame_card(ui, |ui| {
                ui.label(RichText::new("GAME").size(12.0).color(MUTED));
                ui.add_space(4.0);
                ui.add_enabled_ui(!self.running, |ui| {
                    ui.horizontal_wrapped(|ui| {
                        for g in Game::ALL {
                            if ui.selectable_label(self.game == g, g.name()).clicked() {
                                self.game = g;
                            }
                        }
                    });
                });
                ui.add_space(4.0);
                ui.label(RichText::new(self.game.hint()).size(11.0).color(MUTED));
            });

            ui.add_space(12.0);

            // --- Controller (what the phone shows up as) ---
            self.device_card(ui);

            ui.add_space(12.0);

            // --- Launch / Stop ---
            let (label, color) = if self.running {
                ("■  Stop", RED)
            } else {
                ("▶  Launch", ACCENT)
            };
            let btn = egui::Button::new(RichText::new(label).size(20.0).strong().color(Color32::WHITE))
                .fill(color)
                .min_size(egui::vec2(ui.available_width(), 52.0))
                .rounding(12.0);
            if ui.add(btn).clicked() {
                if self.running {
                    self.stop();
                } else {
                    self.launch();
                }
            }

            ui.add_space(12.0);

            // --- Status dots ---
            frame_card(ui, |ui| {
                dot_row(ui, self.device.dot(), status.device_ok);
                dot_row(ui, "Phone connected", status.phone.is_some());
                match self.game {
                    Game::BeamNg => {
                        dot_row(ui, "BeamNG telemetry", status.beamng);
                        dot_row(ui, "MotionSim (slide + crash)", status.motion);
                    }
                    Game::TruckSim => {
                        dot_row(ui, "Truck telemetry (SCS plugin)", status.beamng);
                    }
                    Game::Wrc10 => {
                        dot_row(ui, "Telemetry (not wired for WRC 10 yet)", false);
                    }
                }
                if let Some((spd, rpm, gear)) = status.last {
                    ui.add_space(2.0);
                    ui.label(
                        RichText::new(format!(
                            "{spd:.0} km/h   {rpm:.0} rpm   gear {gear}"
                        ))
                        .font(FontId::monospace(13.0))
                        .color(MUTED),
                    );
                }
            });

            ui.add_space(12.0);

            // --- PicoPanel (the other app that wants OutGauge) ---
            self.pico_card(ui);

            ui.add_space(12.0);

            // --- Head tracking ---
            self.head_card(ui);

            ui.add_space(10.0);

            // --- Log ---
            ui.label(RichText::new("LOG").size(12.0).color(MUTED));
            ui.add_space(2.0);
            let logs = self.shared.log_lines();
            egui::Frame::none()
                .fill(CARD)
                .rounding(10.0)
                .inner_margin(egui::Margin::same(8.0))
                .show(ui, |ui| {
                    egui::ScrollArea::vertical()
                        .stick_to_bottom(true)
                        .auto_shrink([false, false])
                        .show(ui, |ui| {
                            ui.set_min_height(120.0);
                            if logs.is_empty() {
                                ui.label(
                                    RichText::new("Press Launch to start the server.")
                                        .font(FontId::monospace(12.0))
                                        .color(MUTED),
                                );
                            }
                            for line in &logs {
                                ui.label(
                                    RichText::new(line)
                                        .font(FontId::monospace(12.0))
                                        .color(Color32::from_rgb(200, 200, 208)),
                                );
                            }
                        });
                });

            ui.add_space(8.0);
            ui.label(
                RichText::new(
                    "Needs vJoy (wheel mode) or ViGEmBus (Xbox mode). In BeamNG enable \
                     OutGauge (127.0.0.1:4444) and, for slide/crash feedback, MotionSim/OutSim \
                     (127.0.0.1:4445).",
                )
                .size(11.0)
                .color(MUTED),
            );
          });
        });
    }
}

impl App {
    /// The CONTROLLER card: which virtual device the phone drives. Wheel mode
    /// (vJoy) is its own DirectInput device, so a real Xbox pad can stay
    /// plugged in and bound; Xbox mode is the old ViGEmBus pad.
    fn device_card(&mut self, ui: &mut egui::Ui) {
        frame_card(ui, |ui| {
            ui.label(RichText::new("CONTROLLER").size(12.0).color(MUTED));
            ui.add_space(4.0);
            ui.add_enabled_ui(!self.running, |ui| {
                ui.horizontal_wrapped(|ui| {
                    for d in DeviceMode::ALL {
                        if ui.selectable_label(self.device == d, d.name()).clicked() {
                            self.device = d;
                        }
                    }
                });
            });
            ui.add_space(4.0);
            ui.label(RichText::new(self.device.hint()).size(11.0).color(MUTED));

            if self.device == DeviceMode::Wheel {
                ui.add_space(4.0);
                match &self.vjoy {
                    Ok(p) => {
                        ui.label(
                            RichText::new(format!("vJoy {} - {}", p.version, p.summary))
                                .size(11.0)
                                .color(GREEN),
                        );
                        // Missing axes / too few buttons: fixable in vJoyConf,
                        // and much better found here than mid-race.
                        for w in &p.warnings {
                            ui.label(RichText::new(w).size(11.0).color(AMBER));
                        }
                        if !p.warnings.is_empty() && ui.button(RichText::new("Re-check").size(11.0)).clicked()
                        {
                            self.vjoy = vjoy::probe();
                        }
                    }
                    Err(err) => {
                        ui.label(RichText::new(err).size(11.0).color(RED));
                        ui.horizontal(|ui| {
                            ui.hyperlink_to(
                                RichText::new("Get vJoy").size(11.0),
                                "https://github.com/njz3/vJoy/releases",
                            );
                            if ui
                                .add(egui::Button::new(RichText::new("Re-check").size(11.0)))
                                .clicked()
                            {
                                self.vjoy = vjoy::probe();
                            }
                        });
                    }
                }
            }
        });
    }

    /// The HEAD TRACKING card: webcam pose -> TrackIR for any game.
    /// PicoPanel: the RP2040 dashboard panel's PC app. It reads the same
    /// OutGauge stream we do, and only one process can hold UDP 4444 - so we
    /// keep the port and hand it a copy of the enriched telemetry instead.
    fn pico_card(&mut self, ui: &mut egui::Ui) {
        // tasklist is a process spawn; once every couple of seconds is plenty.
        if self.pico.last_poll.elapsed() > Duration::from_secs(2) {
            self.pico.seen_running = picopanel::is_running();
            self.pico.last_poll = std::time::Instant::now();
        }
        let sent = self.shared.mirror_sent();
        let yielding = picopanel::yields_outgauge();

        frame_card(ui, |ui| {
            ui.horizontal(|ui| {
                ui.label(RichText::new("PICOPANEL").size(12.0).color(MUTED));
                ui.label(
                    RichText::new("dashboard panel \u{2192} same telemetry")
                        .size(11.0)
                        .color(MUTED),
                );
            });
            ui.add_space(6.0);

            let mut changed = false;
            ui.horizontal(|ui| {
                changed |= ui
                    .checkbox(&mut self.pico.on, "Send it a copy")
                    .on_hover_text(
                        "OutGauge only talks to one program. We keep 4444 and PicoPanel \
                         reads our copy - with the learned redline, slide and impact \
                         already folded in.",
                    )
                    .changed();
                ui.add_enabled_ui(self.pico.on, |ui| {
                    changed |= ui
                        .add(
                            egui::TextEdit::singleline(&mut self.pico.addr)
                                .desired_width(140.0)
                                .font(FontId::monospace(12.0)),
                        )
                        .changed();
                });
            });
            if self.pico.on && self.pico.parsed().is_none() {
                ui.label(
                    RichText::new("Not an address - expected host:port, e.g. 127.0.0.1:5051")
                        .size(11.0)
                        .color(AMBER),
                );
            }
            if changed {
                self.apply_pico();
            }

            ui.add_space(4.0);
            dot_row(ui, "PicoPanel running", self.pico.seen_running);
            dot_row(
                ui,
                match yielding {
                    Some(true) => "Leaves port 4444 to us",
                    Some(false) => "Still binding 4444 itself",
                    None => "Never saved a setting yet",
                },
                yielding == Some(true),
            );
            if self.pico.on {
                ui.label(
                    RichText::new(if sent > 0 {
                        format!("{sent} packets copied")
                    } else if self.running {
                        "waiting for telemetry from the game".to_string()
                    } else {
                        "starts with the server".to_string()
                    })
                    .font(FontId::monospace(12.0))
                    .color(if sent > 0 { GREEN } else { MUTED }),
                );
            }

            ui.add_space(6.0);
            ui.horizontal(|ui| {
                let can_start = self.pico.exe.is_some() && !self.pico.seen_running;
                if ui
                    .add_enabled(
                        can_start,
                        egui::Button::new(RichText::new("\u{25B6} Start PicoPanel")),
                    )
                    .on_hover_text("Sets it to leave 4444 alone first, then starts it")
                    .clicked()
                {
                    // Order matters: PicoPanel reads that flag once, at startup,
                    // so setting it after spawning would be a run too late.
                    let flag = picopanel::set_yield_outgauge(true);
                    if let Err(e) = &flag {
                        self.pico.note = Some((e.clone(), true));
                    }
                    if !self.pico.on {
                        self.pico.on = true;
                        self.apply_pico();
                    }
                    let exe = self.pico.exe.clone().unwrap();
                    match picopanel::start(&exe) {
                        Ok(child) => {
                            self.pico.child = Some(child);
                            self.pico.seen_running = true;
                            self.shared.log("Started PicoPanel; it reads our telemetry copy.");
                            if flag.is_ok() {
                                self.pico.note =
                                    Some(("Started, set to read our copy.".to_string(), false));
                            }
                        }
                        Err(e) => {
                            self.shared.log(e.clone());
                            self.pico.note = Some((e, true));
                        }
                    }
                }
                if ui
                    .add_enabled(
                        yielding != Some(true),
                        egui::Button::new("Make it yield 4444"),
                    )
                    .on_hover_text("Writes yield_outgauge into PicoPanel's settings")
                    .clicked()
                {
                    self.pico.note = Some(match picopanel::set_yield_outgauge(true) {
                        Ok(p) => (
                            format!("Set in {}. Restart PicoPanel to apply.", p.display()),
                            false,
                        ),
                        Err(e) => (e, true),
                    });
                }
                if ui
                    .add_enabled(self.pico.exe.is_none(), egui::Button::new("\u{21BB}"))
                    .on_hover_text("Look for PicoPanel.exe again")
                    .clicked()
                {
                    self.pico.exe = picopanel::find_exe();
                    match &self.pico.exe {
                        Some(_) => self.apply_pico(),
                        None => {
                            self.pico.note =
                                Some(("Still no PicoPanel.exe found.".to_string(), true))
                        }
                    }
                }
            });

            match (&self.pico.note, &self.pico.exe) {
                (Some((msg, err)), _) => {
                    ui.label(
                        RichText::new(msg)
                            .size(11.0)
                            .color(if *err { AMBER } else { MUTED }),
                    );
                }
                (None, None) => {
                    ui.label(
                        RichText::new(
                            "PicoPanel.exe not found - start it yourself, the copy still reaches it.",
                        )
                        .size(11.0)
                        .color(MUTED),
                    );
                }
                (None, Some(exe)) => {
                    ui.label(
                        RichText::new(exe.display().to_string())
                            .size(10.0)
                            .color(MUTED),
                    );
                }
            }
        });
    }

    fn head_card(&mut self, ui: &mut egui::Ui) {
        let head = self.shared.head.status.lock().unwrap().clone();

        frame_card(ui, |ui| {
            ui.horizontal(|ui| {
                ui.label(RichText::new("HEAD TRACKING").size(12.0).color(MUTED));
                ui.label(
                    RichText::new("webcam \u{2192} TrackIR")
                        .size(11.0)
                        .color(MUTED),
                );
            });
            ui.add_space(6.0);

            // Camera picker + start/stop + center.
            ui.horizontal(|ui| {
                let mut settings = self.head.settings.lock().unwrap();
                ui.add_enabled_ui(!self.head.running, |ui| {
                    let current = self
                        .head
                        .cameras
                        .iter()
                        .find(|(i, _)| *i == settings.camera)
                        .map(|(_, n)| n.clone())
                        .unwrap_or_else(|| format!("Camera {}", settings.camera));
                    egui::ComboBox::from_id_salt("head_cam")
                        .selected_text(current)
                        .width(180.0)
                        .show_ui(ui, |ui| {
                            for (idx, name) in &self.head.cameras {
                                ui.selectable_value(&mut settings.camera, *idx, name);
                            }
                        });
                    if ui.button("\u{21BB}").on_hover_text("Rescan cameras").clicked() {
                        self.head.cameras = headtracker::list_cameras();
                    }
                });
                drop(settings);

                let (label, color) = if self.head.running {
                    ("■ Stop", RED)
                } else {
                    ("▶ Start", ACCENT)
                };
                if ui
                    .add(egui::Button::new(RichText::new(label).strong().color(Color32::WHITE)).fill(color))
                    .clicked()
                {
                    if self.head.running {
                        self.stop_head();
                    } else {
                        self.start_head();
                    }
                }
                if ui
                    .add_enabled(self.head.running, egui::Button::new("Center"))
                    .on_hover_text("Re-zero on your current head position")
                    .clicked()
                {
                    self.shared.head.center.store(true, Ordering::Relaxed);
                }
            });

            ui.add_space(4.0);
            dot_row(ui, "Camera", head.camera_ok && head.running);
            dot_row(ui, "Face found", head.face);
            dot_row(
                ui,
                if head.game_id != 0 {
                    "Game reading pose"
                } else {
                    "Game reading pose (start tracking, then the game)"
                },
                head.game_id != 0,
            );
            if head.running {
                ui.label(
                    RichText::new(format!(
                        "{:.0} fps   yaw {:+.0}\u{00B0}   pitch {:+.0}\u{00B0}",
                        head.fps, head.yaw, head.pitch
                    ))
                    .font(FontId::monospace(12.0))
                    .color(MUTED),
                );
            }
            if let Some(err) = &head.error {
                ui.label(RichText::new(err).size(11.0).color(RED));
            }

            ui.add_space(6.0);
            {
                let mut settings = self.head.settings.lock().unwrap();
                slider_row(ui, "Smoothing", &mut settings.smoothing, 0.0..=1.0, "");
                slider_row(ui, "Rotation gain", &mut settings.rot_gain, 0.5..=6.0, "x");
                slider_row(ui, "Position gain", &mut settings.pos_gain, 0.0..=3.0, "x");
                ui.add_enabled_ui(!self.head.running, |ui| {
                    slider_row(ui, "Camera FOV", &mut settings.fov, 40.0..=110.0, "\u{00B0}");
                });
                ui.checkbox(&mut settings.low_light, "Low light boost (full fps in a dark room)");
                ui.checkbox(&mut settings.opentrack_udp, "Send to opentrack (UDP :4242)")
                    .on_hover_text(
                        "For games with anti-cheat (BattlEye/EAC) that only load \
                         opentrack's whitelisted TrackIR DLL. In opentrack pick Input \
                         'UDP over network' and Output 'freetrack 2.0 Enhanced'.",
                    );

                ui.collapsing("Axes", |ui| {
                    egui::Grid::new("head_axes").spacing([16.0, 4.0]).show(ui, |ui| {
                        for (i, name) in headtracker::AXIS_NAMES.iter().enumerate() {
                            ui.checkbox(&mut settings.axis_on[i], *name);
                            ui.add_enabled_ui(settings.axis_on[i], |ui| {
                                ui.checkbox(&mut settings.axis_mirror[i], "Mirror");
                            });
                            ui.end_row();
                        }
                    });
                });
            }

            ui.add_space(4.0);
            ui.checkbox(&mut self.head.show_preview, "Camera preview");
            self.shared
                .head
                .preview_on
                .store(self.head.show_preview && self.head.running, Ordering::Relaxed);

            if self.head.show_preview && self.head.running {
                self.head_preview(ui);
            }
        });
    }

    fn head_preview(&mut self, ui: &mut egui::Ui) {
        let Some(frame) = self.shared.head.preview.lock().unwrap().take() else {
            // No new frame since last repaint; keep showing the old texture.
            if let Some(tex) = &self.head.texture {
                draw_preview(ui, tex, &[], None);
            }
            return;
        };
        let img = egui::ColorImage::from_rgb([frame.width, frame.height], &frame.rgb);
        match &mut self.head.texture {
            Some(tex) => tex.set(img, egui::TextureOptions::LINEAR),
            None => {
                self.head.texture =
                    Some(ui.ctx().load_texture("head_preview", img, egui::TextureOptions::LINEAR));
            }
        }
        let tex = self.head.texture.as_ref().unwrap();
        draw_preview(ui, tex, &frame.points, frame.face);
    }
}

/// Paint the preview image scaled to the card width, with landmarks on top.
fn draw_preview(
    ui: &mut egui::Ui,
    tex: &egui::TextureHandle,
    points: &[(f32, f32)],
    face: Option<(f32, f32, f32, f32)>,
) {
    let avail = ui.available_width();
    let size = tex.size_vec2();
    let scale = (avail / size.x).min(1.5);
    let (rect, _) =
        ui.allocate_exact_size(egui::vec2(size.x * scale, size.y * scale), egui::Sense::hover());
    ui.painter().image(
        tex.id(),
        rect,
        egui::Rect::from_min_max(egui::pos2(0.0, 0.0), egui::pos2(1.0, 1.0)),
        Color32::WHITE,
    );
    if let Some((x0, y0, x1, y1)) = face {
        let fr = egui::Rect::from_min_max(
            rect.min + egui::vec2(x0 * scale, y0 * scale),
            rect.min + egui::vec2(x1 * scale, y1 * scale),
        );
        ui.painter()
            .rect_stroke(fr, 4.0, egui::Stroke::new(1.0, ACCENT));
    }
    for (x, y) in points {
        ui.painter().circle_filled(
            rect.min + egui::vec2(x * scale, y * scale),
            1.5,
            GREEN,
        );
    }
}

fn slider_row(
    ui: &mut egui::Ui,
    label: &str,
    value: &mut f32,
    range: std::ops::RangeInclusive<f32>,
    suffix: &str,
) {
    ui.horizontal(|ui| {
        ui.label(RichText::new(label).size(12.0));
        ui.add(
            egui::Slider::new(value, range)
                .suffix(suffix)
                .fixed_decimals(1),
        );
    });
}

fn frame_card(ui: &mut egui::Ui, add: impl FnOnce(&mut egui::Ui)) {
    egui::Frame::none()
        .fill(CARD)
        .rounding(10.0)
        .inner_margin(egui::Margin::same(12.0))
        .show(ui, |ui| {
            ui.set_width(ui.available_width());
            add(ui);
        });
}

fn dot_row(ui: &mut egui::Ui, label: &str, on: bool) {
    ui.horizontal(|ui| {
        let color = if on { GREEN } else { Color32::from_rgb(70, 70, 78) };
        let (rect, _) = ui.allocate_exact_size(egui::vec2(12.0, 12.0), egui::Sense::hover());
        ui.painter().circle_filled(rect.center(), 5.0, color);
        ui.label(RichText::new(label).size(13.0));
    });
}
