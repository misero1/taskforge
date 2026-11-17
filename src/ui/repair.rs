//! "Fix Windows" page: one-click built-in Windows diagnostics and repairs,
//! searchable by symptom.

use eframe::egui::{self, Align, Layout, RichText, Stroke};

use crate::app::{App, Follow};
use crate::platform;
use crate::repair::{self, Action, Cat, Fix};
use crate::ui::{theme, widgets};

impl App {
    pub fn page_repair(&mut self, ui: &mut egui::Ui) {
        let pal = self.pal;
        let terms: Vec<String> = self
            .repair_search
            .split_whitespace()
            .map(str::to_lowercase)
            .collect();
        let shown: Vec<&'static Fix> = repair::FIXES
            .iter()
            .filter(|f| self.repair_cat.is_none_or(|c| c == f.cat) && f.matches(&terms))
            .collect();
        let mut run: Option<&'static Fix> = None;
        let mut confirmed: Option<&'static Fix> = None;
        let mut cancel = false;

        egui::ScrollArea::vertical().auto_shrink([false, false]).show(ui, |ui| {
            ui.set_max_width(1040.0);
            // ── Search and categories ──
            widgets::card(ui, &pal, |ui| {
                ui.set_width(ui.available_width());
                ui.horizontal(|ui| {
                    ui.add(
                        egui::TextEdit::singleline(&mut self.repair_search)
                            .hint_text("🔍  Describe the problem: no sound, no internet, blue screen, update stuck…")
                            .desired_width(460.0),
                    );
                    if !self.repair_search.is_empty() && ui.small_button("✖").on_hover_text("Clear").clicked() {
                        self.repair_search.clear();
                    }
                    ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                        widgets::faint(ui, &pal, format!("{} of {} fixes", shown.len(), repair::FIXES.len()));
                    });
                });
                ui.add_space(6.0);
                ui.horizontal_wrapped(|ui| {
                    if ui.selectable_label(self.repair_cat.is_none(), "All").clicked() {
                        self.repair_cat = None;
                    }
                    for c in Cat::ALL {
                        let on = self.repair_cat == Some(c);
                        if ui.selectable_label(on, format!("{} {}", c.icon(), c.label())).clicked() {
                            self.repair_cat = if on { None } else { Some(c) };
                        }
                    }
                });
                ui.add_space(4.0);
                let admin_note = if platform::is_admin() {
                    "TaskForge is running as administrator, so admin fixes start straight away."
                } else {
                    "Fixes marked Admin ask Windows for permission first (the usual UAC prompt)."
                };
                widgets::faint(
                    ui,
                    &pal,
                    format!(
                        "Command fixes open in their own window so you can watch the progress and results. {admin_note} \
                         Not sure where to start? Try the ★ recommended fixes first."
                    ),
                );
            });
            ui.add_space(10.0);

            if shown.is_empty() {
                widgets::empty_state(
                    ui,
                    &pal,
                    "🔎",
                    "No fixes match that",
                    "Try other words for the symptom, pick All, or open the Windows troubleshooters.",
                );
                ui.vertical_centered(|ui| {
                    if ui.button("Open Windows troubleshooters").clicked() {
                        run = repair::by_id("troubleshooters");
                    }
                });
                return;
            }

            for cat in Cat::ALL {
                let items: Vec<&&Fix> = shown.iter().filter(|f| f.cat == cat).collect();
                if items.is_empty() {
                    continue;
                }
                ui.add_space(4.0);
                ui.label(RichText::new(format!("{}  {}", cat.icon(), cat.label())).size(16.0).strong());
                ui.add_space(4.0);
                for f in items {
                    let f: &'static Fix = f;
                    let border = if f.recommended {
                        widgets::with_alpha(pal.accent, 0.55)
                    } else {
                        pal.border
                    };
                    egui::Frame::new()
                        .fill(pal.card)
                        .stroke(Stroke::new(1.0f32, border))
                        .corner_radius(12.0)
                        .inner_margin(egui::Margin::same(14))
                        .show(ui, |ui| {
                            ui.set_width(ui.available_width());
                            // Title row: name, badges, buttons.
                            ui.horizontal(|ui| {
                                ui.label(RichText::new(f.title).size(15.0).strong());
                                if f.recommended {
                                    widgets::pill(ui, "★ Try first", pal.accent);
                                }
                                if f.admin {
                                    widgets::pill(ui, "🛡 Admin", pal.warn);
                                }
                                widgets::pill(ui, &format!("⏱ {}", f.time), pal.text_dim);
                                if f.restart {
                                    widgets::pill(ui, "⟲ Restart needed", pal.mem);
                                }
                                if matches!(f.action, Action::Open(_)) {
                                    widgets::pill(ui, "↗ Opens a Windows tool", pal.text_dim);
                                }
                                ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                                    if theme::primary_button(ui, &pal, "▶ Run").clicked() {
                                        run = Some(f);
                                    }
                                    if ui
                                        .button("📋")
                                        .on_hover_text("Copy the command to run it yourself")
                                        .clicked()
                                    {
                                        ui.ctx().copy_text(f.command_text());
                                    }
                                });
                            });
                            ui.add_space(2.0);
                            ui.add(egui::Label::new(RichText::new(f.what).color(pal.text)).wrap());
                            ui.add_space(4.0);
                            ui.horizontal_wrapped(|ui| {
                                ui.spacing_mut().item_spacing.x = 6.0;
                                ui.label(RichText::new("Helps with:").size(12.0).strong().color(pal.text_dim));
                                for (i, s) in f.symptoms.iter().enumerate() {
                                    if i > 0 {
                                        ui.label(RichText::new("·").size(12.0).color(pal.text_faint));
                                    }
                                    ui.label(RichText::new(*s).size(12.0).color(pal.text_dim));
                                }
                            });
                            if self.repair_confirm == Some(f.id) {
                                ui.add_space(6.0);
                                egui::Frame::new()
                                    .fill(widgets::with_alpha(pal.warn, 0.12))
                                    .corner_radius(8.0)
                                    .inner_margin(egui::Margin::same(10))
                                    .show(ui, |ui| {
                                        ui.set_width(ui.available_width());
                                        ui.label(
                                            RichText::new(format!("⚠ {}", f.caution.unwrap_or_default()))
                                                .color(pal.warn),
                                        );
                                        ui.horizontal(|ui| {
                                            if theme::primary_button(ui, &pal, "Run it").clicked() {
                                                confirmed = Some(f);
                                            }
                                            if ui.button("Cancel").clicked() {
                                                cancel = true;
                                            }
                                        });
                                    });
                            } else if let Some(c) = f.caution {
                                ui.label(RichText::new(format!("⚠ {c}")).size(12.0).color(pal.text_faint));
                            }
                            egui::CollapsingHeader::new(RichText::new("Show command").size(12.0).color(pal.text_dim))
                                .id_salt(("fixcmd", f.id))
                                .show(ui, |ui| {
                                    ui.label(RichText::new(f.command_text()).monospace().size(11.5));
                                });
                        });
                    ui.add_space(6.0);
                }
            }
            ui.add_space(6.0);
            widgets::faint(
                ui,
                &pal,
                "Every fix uses tools built into Windows. Nothing is downloaded except clean system files, which DISM gets from Windows Update.",
            );
        });

        if cancel {
            self.repair_confirm = None;
        }
        if let Some(f) = run {
            if f.caution.is_some() && !matches!(f.action, Action::Open(_)) {
                self.repair_confirm = Some(f.id);
            } else {
                self.run_fix(f);
            }
        }
        if let Some(f) = confirmed {
            self.repair_confirm = None;
            self.run_fix(f);
        }
    }

    fn run_fix(&mut self, f: &'static Fix) {
        self.mon
            .log_action(0, "Fix Windows", format!("Ran: {}", f.title));
        crate::diag::info(&format!("Fix Windows: {}", f.id));
        self.job(0, f.title, Follow::None, false, move || repair::run(f));
    }
}
