//! Health check page ("Why is my PC slow?") and diagnostics helpers.

use eframe::egui::{self, vec2, Align, Layout, RichText, Stroke};

use crate::app::{App, Page, ToastKind};
use crate::diagnose::{self, Fix, Severity};
use crate::platform::{self, Priority};
use crate::ui::widgets::{self, fmt_bytes};

impl App {
    pub fn page_health(&mut self, ui: &mut egui::Ui) {
        let pal = self.pal;
        let s = self.snap.clone();
        if self.startup.loaded_at.is_none() && !self.startup.loading {
            self.load_startup();
        }
        let shared = self.mon.shared.clone();
        let (findings, leaks) = {
            let g = shared.lock().unwrap();
            let cpu: Vec<f32> = g.history.cpu.iter().copied().collect();
            let disk: Vec<f32> = g
                .history
                .disk_r
                .iter()
                .zip(&g.history.disk_w)
                .map(|(r, w)| r + w)
                .collect();
            let suspended: Vec<u32> = self.suspended.iter().copied().collect();
            let f = diagnose::diagnose(&diagnose::Inputs {
                snap: &s,
                cpu_recent: &cpu,
                disk_recent: &disk,
                leaks: &g.leak_suspects,
                startup: &self.startup.data,
                impacts: &g.impact.results,
                suspended: &suspended,
                is_admin: platform::is_admin(),
            });
            (f, g.leak_suspects.clone())
        };
        let score = diagnose::score(&findings);
        let mut fix: Option<Fix> = None;

        egui::ScrollArea::vertical().auto_shrink([false, false]).show(ui, |ui| {
            ui.set_max_width(1000.0);
            widgets::card(ui, &pal, |ui| {
                ui.set_width(ui.available_width());
                ui.horizontal(|ui| {
                    let col = if score >= 80 { pal.good } else if score >= 55 { pal.warn } else { pal.bad };
                    widgets::ring(ui, &pal, score as f32 / 100.0, col, &score.to_string(), "health", 110.0);
                    ui.add_space(12.0);
                    ui.vertical(|ui| {
                        let bad = findings.iter().filter(|f| f.severity >= Severity::Warn).count();
                        let headline = match bad {
                            0 => "Your PC looks healthy".to_string(),
                            1 => "1 thing is slowing your PC down".to_string(),
                            n => format!("{n} things are slowing your PC down"),
                        };
                        ui.label(RichText::new(headline).size(20.0).strong());
                        widgets::faint(
                            ui,
                            &pal,
                            "Checked just now: processor, memory, drives, GPU, memory leaks, startup apps and more. This page updates live.",
                        );
                        ui.add_space(6.0);
                        ui.horizontal(|ui| {
                            widgets::pill(ui, &format!("CPU {:.0}%", s.cpu_total), pal.cpu);
                            widgets::pill(ui, &format!("Memory {:.0}%", s.mem_pct()), pal.mem);
                            if !s.gpus.is_empty() {
                                widgets::pill(ui, &format!("GPU {:.0}%", s.gpu_util()), pal.gpu);
                            }
                            widgets::pill(ui, &format!("Up {}", widgets::fmt_dur(s.uptime)), pal.text_dim);
                        });
                    });
                });
            });
            ui.add_space(10.0);

            for f in &findings {
                let (icon, col) = match f.severity {
                    Severity::Bad => ("⛔", pal.bad),
                    Severity::Warn => ("⚠", pal.warn),
                    Severity::Info => ("ℹ", pal.accent),
                    Severity::Good => ("✔", pal.good),
                };
                egui::Frame::new()
                    .fill(pal.card)
                    .stroke(Stroke::new(1.0f32, widgets::with_alpha(col, 0.45)))
                    .corner_radius(12.0)
                    .inner_margin(egui::Margin::same(14))
                    .show(ui, |ui| {
                        ui.set_width(ui.available_width());
                        ui.horizontal(|ui| {
                            ui.label(RichText::new(icon).size(18.0).color(col));
                            ui.vertical(|ui| {
                                ui.label(RichText::new(&f.title).size(15.0).strong());
                                ui.add(egui::Label::new(RichText::new(&f.detail).color(pal.text_dim)).wrap());
                                if !f.fixes.is_empty() {
                                    ui.add_space(4.0);
                                    ui.horizontal_wrapped(|ui| {
                                        for (i, (label, fx)) in f.fixes.iter().enumerate() {
                                            let btn = if i == 0 {
                                                egui::Button::new(RichText::new(label).color(egui::Color32::WHITE).strong()).fill(pal.accent)
                                            } else {
                                                egui::Button::new(label)
                                            };
                                            if ui.add(btn).clicked() {
                                                fix = Some(fx.clone());
                                            }
                                        }
                                    });
                                }
                            });
                        });
                    });
                ui.add_space(8.0);
            }

            ui.horizontal(|ui| {
                widgets::faint(ui, &pal, "Something else wrong (no sound, no internet, crashes, updates failing)?");
                if ui.button("🚑 Fix Windows →").clicked() {
                    fix = Some(Fix::OpenRepair);
                }
            });
            ui.add_space(8.0);

            // ── Memory trend table ──
            widgets::card(ui, &pal, |ui| {
                ui.set_width(ui.available_width());
                widgets::section_title(ui, &pal, "📈 Memory-leak watch");
                widgets::faint(
                    ui,
                    &pal,
                    "Every running app's memory is sampled once a minute. Apps that grow steadily for 15+ minutes are flagged here.",
                );
                ui.add_space(4.0);
                if leaks.is_empty() {
                    ui.label(RichText::new("✔ No leaking apps detected so far.").color(pal.good));
                } else {
                    egui::Grid::new("leaks").num_columns(5).striped(true).spacing(vec2(16.0, 4.0)).show(ui, |ui| {
                        for h in ["App", "Started at", "Now", "Growth", "Watched for"] {
                            ui.label(RichText::new(h).size(11.0).color(pal.text_dim));
                        }
                        ui.end_row();
                        for l in &leaks {
                            ui.horizontal(|ui| {
                                widgets::avatar(ui, &l.name, pal.dark);
                                if ui.link(&*l.name).clicked() {
                                    fix = Some(Fix::ShowProcess(l.pid));
                                }
                            });
                            ui.label(fmt_bytes(l.start_mem));
                            ui.label(RichText::new(fmt_bytes(l.now_mem)).strong());
                            ui.label(RichText::new(format!("+{:.0} MB/h", l.mb_per_hour)).color(pal.warn))
                                .on_hover_text(format!("Steadiness of the growth: {:.0}%", l.fit * 100.0));
                            ui.label(format!("{} min", l.minutes));
                            ui.end_row();
                        }
                    });
                }
            });
        });

        if let Some(f) = fix {
            self.apply_fix(f);
        }
    }

    pub fn apply_fix(&mut self, f: Fix) {
        match f {
            Fix::EndProcess { pid, name } => self.request_kill(pid, &name, false),
            Fix::LowerPriority { pid, name } => {
                self.set_priority(pid, &name, Priority::BelowNormal)
            }
            Fix::Restart { pid } => self.restart(pid),
            Fix::Resume { pid, name } => self.set_suspended(pid, &name, false),
            Fix::ShowProcess(pid) => self.reveal_process(pid),
            Fix::OpenStartup => self.page = Page::Startup,
            Fix::OpenPerformance => self.page = Page::Performance,
            Fix::OpenRepair => self.page = Page::Repair,
            Fix::Launch(cmd) => {
                if let Err(e) = platform::launch_tool(&cmd) {
                    self.toast(ToastKind::Err, e);
                }
            }
        }
    }

    /// Settings → Diagnostics section.
    pub fn diagnostics_ui(&mut self, ui: &mut egui::Ui) {
        let pal = self.pal;
        let problems = crate::diag::problem_count();
        ui.horizontal(|ui| {
            if problems == 0 {
                ui.label(RichText::new("✔ No errors logged this session").color(pal.good));
            } else {
                ui.label(
                    RichText::new(format!(
                        "⚠ {problems} warning(s)/error(s) logged this session"
                    ))
                    .color(pal.warn),
                );
            }
        });
        widgets::faint(
            ui,
            &pal,
            crate::diag::log_path()
                .map(|p| format!("Log file: {}", p.display()))
                .unwrap_or_default(),
        );
        ui.horizontal(|ui| {
            if ui
                .button("📋 Copy diagnostics")
                .on_hover_text(
                    "Copies system info and recent log lines — paste it when reporting a problem",
                )
                .clicked()
            {
                let text = crate::diag::report(&self.snap);
                ui.ctx().copy_text(text);
                self.toast(ToastKind::Ok, "Diagnostics copied to the clipboard");
            }
            if ui.button("📂 Open log folder").clicked() {
                if let Some(p) = crate::diag::log_path() {
                    let _ = platform::open_location(&p.display().to_string());
                }
            }
        });
        egui::CollapsingHeader::new("Recent log lines").show(ui, |ui| {
            egui::ScrollArea::vertical()
                .max_height(200.0)
                .show(ui, |ui| {
                    for l in crate::diag::recent(80) {
                        let col = if l.contains("[Error]") {
                            pal.bad
                        } else if l.contains("[Warn]") {
                            pal.warn
                        } else {
                            pal.text_dim
                        };
                        ui.label(RichText::new(l).monospace().size(11.0).color(col));
                    }
                });
        });
        let _ = (Align::Center, Layout::left_to_right(Align::Center));
    }
}
