//! The launcher window: shows the PC's IP and a Launch button, plus live status
//! and a log panel. Built on eframe/egui so it ships as a single .exe.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use eframe::egui::{self, Color32, FontId, RichText};

use crate::headtracker;
use crate::server::{self, Game, Shared};

const BG: Color32 = Color32::from_rgb(14, 14, 18);
const CARD: Color32 = Color32::from_rgb(24, 24, 31);
const ACCENT: Color32 = Color32::from_rgb(76, 141, 255);
const RED: Color32 = Color32::from_rgb(196, 22, 28);
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
    head: HeadUi,
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
        App {
            shared: Shared::new(),
            stop: Arc::new(AtomicBool::new(false)),
            running: false,
            ip: server::local_ipv4()
                .map(|a| a.to_string())
                .unwrap_or_else(|| "not on a network".to_string()),
            game: Game::BeamNg,
            head: HeadUi::new(),
        }
    }

    fn launch(&mut self) {
        self.stop.store(false, Ordering::Relaxed);
        let shared = Arc::clone(&self.shared);
        let stop = Arc::clone(&self.stop);
        let game = self.game;
        std::thread::spawn(move || server::run(shared, stop, game));
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
        if self.running && status.error.is_some() && !status.vigem_ok {
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
                    RichText::new("Type this on your phone, then tap Connect.")
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
                dot_row(ui, "ViGEmBus (virtual controller)", status.vigem_ok);
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
                    "Needs ViGEmBus. In BeamNG enable OutGauge (127.0.0.1:4444) and, for \
                     slide/crash feedback, MotionSim/OutSim (127.0.0.1:4445).",
                )
                .size(11.0)
                .color(MUTED),
            );
          });
        });
    }
}

impl App {
    /// The HEAD TRACKING card: webcam pose -> TrackIR for any game.
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
