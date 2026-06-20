//! The launcher window: shows the PC's IP and a Launch button, plus live status
//! and a log panel. Built on eframe/egui so it ships as a single .exe.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::Duration;

use eframe::egui::{self, Color32, FontId, RichText};

use crate::server::{self, Game, Shared};

const BG: Color32 = Color32::from_rgb(14, 14, 18);
const CARD: Color32 = Color32::from_rgb(24, 24, 31);
const ACCENT: Color32 = Color32::from_rgb(76, 141, 255);
const RED: Color32 = Color32::from_rgb(196, 22, 28);
const GREEN: Color32 = Color32::from_rgb(60, 200, 110);
const MUTED: Color32 = Color32::from_rgb(138, 138, 149);

pub fn run() -> eframe::Result<()> {
    let mut viewport = egui::ViewportBuilder::default()
        .with_inner_size([480.0, 600.0])
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
}

impl eframe::App for App {
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        // Keep status/log fresh while running.
        ctx.request_repaint_after(Duration::from_millis(200));

        let status = self.shared.status();
        // A fatal error in the server thread flips us back to stopped.
        if self.running && status.error.is_some() && !status.vigem_ok {
            self.running = false;
        }

        egui::CentralPanel::default().show(ctx, |ui| {
            ui.add_space(14.0);
            ui.vertical_centered(|ui| {
                ui.label(RichText::new("CorsaConnect").size(26.0).strong());
                ui.label(
                    RichText::new("BeamNG steering wheel server")
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
                dot_row(ui, "BeamNG telemetry", status.beamng);
                dot_row(ui, "MotionSim (slide + crash)", status.motion);
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
    }
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
