//! Tools page (window finder, file-lock finder, quick fixes) and the
//! Automation (rules) page.

use eframe::egui::{self, vec2, Align, Layout, RichText, Sense, Stroke};

use crate::app::{App, Follow, ToastKind};
use crate::native;
use crate::platform::{self, Priority};
use crate::rules::{Rule, RuleAction};
use crate::ui::theme;
use crate::ui::widgets::{self, fmt_bytes};

impl App {
    pub fn page_tools(&mut self, ui: &mut egui::Ui) {
        let pal = self.pal;
        let s = self.snap.clone();
        egui::ScrollArea::vertical().auto_shrink([false, false]).show(ui, |ui| {
            let half = ((ui.available_width() - 24.0) / 2.0).max(300.0);
            ui.horizontal_top(|ui| {
                ui.spacing_mut().item_spacing.x = 10.0;

                // ── Find a window's process ──
                ui.allocate_ui(vec2(half, 420.0), |ui| {
                    widgets::card(ui, &pal, |ui| {
                        ui.set_width(half - 28.0);
                        ui.label(RichText::new("🎯 Which app owns this window?").size(15.0).strong());
                        widgets::faint(ui, &pal, "Click Pick, then hover over any window within 3 seconds.");
                        ui.add_space(6.0);
                        ui.horizontal(|ui| {
                            match self.pick_at {
                                Some(at) => {
                                    let left = 3.0 - at.elapsed().as_secs_f32();
                                    ui.spinner();
                                    ui.label(RichText::new(format!("Move the pointer over the window… {left:.1}s")).color(pal.accent));
                                }
                                None => {
                                    if theme::primary_button(ui, &pal, "🎯 Pick a window").clicked() {
                                        self.pick_at = Some(std::time::Instant::now());
                                        self.picked = None;
                                    }
                                }
                            }
                        });
                        if let Some(w) = self.picked.clone() {
                            ui.add_space(6.0);
                            let proc_ = s.procs.iter().find(|p| p.pid == w.pid);
                            egui::Frame::new()
                                .fill(widgets::with_alpha(pal.accent, 0.10))
                                .corner_radius(8.0)
                                .inner_margin(egui::Margin::same(10))
                                .show(ui, |ui| {
                                    ui.set_width(ui.available_width());
                                    ui.label(RichText::new(format!("“{}”", w.title)).strong());
                                    match proc_ {
                                        Some(p) => {
                                            ui.horizontal(|ui| {
                                                widgets::avatar(ui, &p.name, pal.dark);
                                                ui.label(format!("{} (PID {})", p.name, p.pid));
                                            });
                                            ui.label(RichText::new(&*p.exe).monospace().color(pal.text_dim));
                                            ui.horizontal(|ui| {
                                                if ui.button("☰ Show in Processes").clicked() {
                                                    let pid = p.pid;
                                                    self.reveal_process(pid);
                                                }
                                                if theme::danger_button(ui, &pal, "End task", true).clicked() {
                                                    let (pid, n) = (p.pid, p.name.clone());
                                                    self.request_kill(pid, &n, false);
                                                }
                                            });
                                        }
                                        None => {
                                            ui.label(format!("PID {}", w.pid));
                                        }
                                    }
                                });
                        }
                        ui.add_space(10.0);
                        ui.horizontal(|ui| {
                            widgets::section_title(ui, &pal, "Open windows");
                            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                                if ui.small_button("⟳ Refresh").clicked() || (self.windows_list.is_empty() && platform::IS_WINDOWS) {
                                    self.windows_list = native::top_windows();
                                }
                            });
                        });
                        if self.windows_list.is_empty() {
                            widgets::faint(ui, &pal, "Available on Windows.");
                        }
                        let mut reveal = None;
                        egui::ScrollArea::vertical().id_salt("wins").max_height(220.0).show(ui, |ui| {
                            for w in &self.windows_list {
                                let name = s.procs.iter().find(|p| p.pid == w.pid).map(|p| &*p.name).unwrap_or("?");
                                ui.horizontal(|ui| {
                                    widgets::avatar(ui, name, pal.dark);
                                    if ui.link(name).clicked() {
                                        reveal = Some(w.pid);
                                    }
                                    ui.add(egui::Label::new(RichText::new(&w.title).color(pal.text_dim)).truncate());
                                });
                            }
                        });
                        if let Some(pid) = reveal {
                            self.reveal_process(pid);
                        }
                    });
                });

                // ── File lock finder ──
                ui.allocate_ui(vec2(half, 420.0), |ui| {
                    widgets::card(ui, &pal, |ui| {
                        ui.set_width(half - 28.0);
                        ui.label(RichText::new("🔓 What's locking this file?").size(15.0).strong());
                        widgets::faint(ui, &pal, "Find the programs that keep a file or folder “in use” so you can close them.");
                        ui.add_space(6.0);
                        ui.horizontal(|ui| {
                            if ui.button("📄 Choose file…").clicked() {
                                if let Some(f) = rfd::FileDialog::new().pick_files() {
                                    self.lock_paths = f;
                                    self.find_lockers();
                                }
                            }
                            if ui.button("📁 Choose folder…").clicked() {
                                if let Some(f) = rfd::FileDialog::new().pick_folder() {
                                    self.lock_paths = vec![f];
                                    self.find_lockers();
                                }
                            }
                            if !self.lock_paths.is_empty() && ui.button("⟳ Check again").clicked() {
                                self.find_lockers();
                            }
                        });
                        for p in self.lock_paths.iter().take(3) {
                            ui.label(RichText::new(p.display().to_string()).monospace().color(pal.text_dim));
                        }
                        ui.add_space(6.0);
                        if self.lockers.loading {
                            ui.horizontal(|ui| {
                                ui.spinner();
                                ui.label("Checking…");
                            });
                        } else if let Some(e) = &self.lockers.error {
                            ui.label(RichText::new(format!("⚠ {e}")).color(pal.bad));
                        } else if self.lockers.loaded_at.is_some() {
                            if self.lockers.data.is_empty() {
                                ui.label(RichText::new("✔ Nothing has it open — it should be free to delete or move.").color(pal.good));
                            } else {
                                ui.label(RichText::new(format!("{} program(s) are using it:", self.lockers.data.len())).strong());
                                let mut kill = None;
                                let mut reveal = None;
                                for l in &self.lockers.data {
                                    let name = s.procs.iter().find(|p| p.pid == l.pid).map(|p| p.name.to_string()).unwrap_or_else(|| l.app.clone());
                                    ui.horizontal(|ui| {
                                        widgets::avatar(ui, &name, pal.dark);
                                        if ui.link(&name).clicked() {
                                            reveal = Some(l.pid);
                                        }
                                        let extra = if l.service.is_empty() { format!("PID {}", l.pid) } else { format!("PID {} · service {}", l.pid, l.service) };
                                        ui.label(RichText::new(extra).color(pal.text_dim));
                                        ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                                            if ui.small_button("End").clicked() {
                                                kill = Some((l.pid, name.clone()));
                                            }
                                        });
                                    });
                                }
                                if let Some((pid, n)) = kill {
                                    self.request_kill(pid, &n, false);
                                }
                                if let Some(pid) = reveal {
                                    self.reveal_process(pid);
                                }
                            }
                        }
                    });
                });
            });
            ui.add_space(10.0);

            // ── Quick fixes & Windows tools ──
            widgets::card(ui, &pal, |ui| {
                ui.set_width(ui.available_width());
                ui.horizontal(|ui| {
                    ui.label(RichText::new("🧰 Quick fixes & Windows tools").size(15.0).strong());
                    ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                        if ui.link("🚑 More repairs on the Fix Windows page →").clicked() {
                            self.page = crate::app::Page::Repair;
                        }
                    });
                });
                ui.add_space(6.0);
                ui.horizontal_wrapped(|ui| {
                    if ui.button("⟲ Restart Windows Explorer").on_hover_text("Fixes a frozen taskbar, Start menu or desktop").clicked() {
                        self.job(0, "Explorer", Follow::None, false, platform::restart_explorer);
                    }
                    for (label, cmd, tip) in [
                        ("📊 Resource Monitor", "resmon", "Detailed per-process disk and network activity"),
                        ("📜 Event Viewer", "eventvwr.msc", "Windows logs: crashes, errors, warnings"),
                        ("⚙ Services console", "services.msc", "The classic services manager"),
                        ("🖥 Device Manager", "devmgmt.msc", "Drivers and hardware"),
                        ("📅 Task Scheduler", "taskschd.msc", "Scheduled tasks (another place apps start from)"),
                        ("ℹ System Information", "msinfo32", "Full hardware and software report"),
                        ("❤ Reliability history", "perfmon /rel", "Timeline of crashes and problems"),
                    ] {
                        if ui.button(label).on_hover_text(tip).clicked() {
                            let c = cmd.to_string();
                            self.job(0, label, Follow::None, true, move || platform::launch_tool(&c));
                        }
                    }
                });
            });
        });
    }

    fn find_lockers(&mut self) {
        let paths = self.lock_paths.clone();
        self.lockers
            .load(&self.ctx.clone(), move || native::file_lockers(&paths));
    }

    // ───────────────────────────── Automation ─────────────────────────────

    pub fn page_rules(&mut self, ui: &mut egui::Ui) {
        let pal = self.pal;
        let s = self.snap.clone();
        let mut changed = false;
        egui::ScrollArea::vertical().auto_shrink([false, false]).show(ui, |ui| {
            ui.set_max_width(980.0);
            // ── New rule form ──
            widgets::card(ui, &pal, |ui| {
                ui.set_width(ui.available_width());
                ui.label(RichText::new("New rule").size(15.0).strong());
                widgets::faint(ui, &pal, "Rules run while TaskForge is open (tip: enable “Start when I sign in” in Settings).");
                ui.add_space(6.0);
                let d = &mut self.rule_draft;
                egui::Grid::new("rule_form").num_columns(2).spacing(vec2(12.0, 8.0)).show(ui, |ui| {
                    ui.label("When the process is");
                    ui.horizontal(|ui| {
                        ui.add(egui::TextEdit::singleline(&mut d.pattern).hint_text("e.g. chrome.exe, game*.exe, *updater*").desired_width(260.0));
                        ui.menu_button("Pick running…", |ui| {
                            let mut names: Vec<&str> = s.procs.iter().map(|p| &*p.name).collect();
                            names.sort_unstable_by_key(|n| n.to_lowercase());
                            names.dedup();
                            egui::ScrollArea::vertical().max_height(300.0).show(ui, |ui| {
                                for n in names {
                                    if ui.button(n).clicked() {
                                        d.pattern = n.to_string();
                                        ui.close_menu();
                                    }
                                }
                            });
                        });
                    });
                    ui.end_row();
                    ui.label("Do this");
                    egui::ComboBox::from_id_salt("rule_kind")
                        .width(320.0)
                        .selected_text(RULE_KINDS[d.kind])
                        .show_ui(ui, |ui| {
                            for (i, k) in RULE_KINDS.iter().enumerate() {
                                ui.selectable_value(&mut d.kind, i, *k);
                            }
                        });
                    ui.end_row();
                    match d.kind {
                        0 => {
                            ui.label("Priority");
                            egui::ComboBox::from_id_salt("rule_pri").selected_text(d.priority.label()).show_ui(ui, |ui| {
                                for p in Priority::ALL {
                                    ui.selectable_value(&mut d.priority, p, p.label());
                                }
                            });
                            ui.end_row();
                        }
                        1 => {
                            ui.label("CPUs");
                            ui.horizontal_wrapped(|ui| {
                                for i in 0..s.logical_cores.clamp(1, 64) {
                                    let mut on = d.mask & (1 << i) != 0;
                                    if ui.checkbox(&mut on, format!("{i}")).changed() {
                                        d.mask ^= 1 << i;
                                    }
                                }
                            });
                            ui.end_row();
                        }
                        3 | 4 => {
                            ui.label("CPU above");
                            ui.add(egui::Slider::new(&mut d.over, 10.0..=100.0).suffix(" %").integer());
                            ui.end_row();
                            ui.label("For at least");
                            ui.add(egui::Slider::new(&mut d.secs, 3..=600).suffix(" s").logarithmic(true));
                            ui.end_row();
                        }
                        5 => {
                            ui.label("Memory above");
                            ui.add(egui::Slider::new(&mut d.mb, 100..=32768).suffix(" MB").logarithmic(true));
                            ui.end_row();
                        }
                        _ => {}
                    }
                });
                ui.add_space(6.0);
                let d = &self.rule_draft;
                let ok = !d.pattern.trim().is_empty() && !(d.kind == 1 && d.mask == 0);
                if ui.add_enabled(ok, egui::Button::new(RichText::new("+ Add rule").strong())).clicked() {
                    let action = match d.kind {
                        0 => RuleAction::Priority(d.priority),
                        1 => RuleAction::Affinity(d.mask),
                        2 => RuleAction::Block,
                        3 => RuleAction::Tame { over: d.over, secs: d.secs },
                        4 => RuleAction::Notify { over: d.over, secs: d.secs },
                        _ => RuleAction::MemoryLimit { mb: d.mb },
                    };
                    self.settings.rules.push(Rule { enabled: true, pattern: d.pattern.trim().to_string(), action });
                    self.rule_draft.pattern.clear();
                    changed = true;
                }
            });
            ui.add_space(10.0);

            // ── Existing rules ──
            if self.settings.rules.is_empty() {
                widgets::empty_state(ui, &pal, "⚡", "No rules yet", "Ideas: give games High priority, block an annoying updater, or tame a browser that hogs the CPU.");
                ui.add_space(10.0);
                ui.horizontal(|ui| {
                    widgets::faint(ui, &pal, "Quick add:");
                    for (label, rule) in [
                        ("Tame any app above 90% CPU for 2 min", Rule { enabled: true, pattern: "*".into(), action: RuleAction::Tame { over: 90.0, secs: 120 } }),
                        ("Warn when any app uses > 4 GB", Rule { enabled: true, pattern: "*".into(), action: RuleAction::MemoryLimit { mb: 4096 } }),
                    ] {
                        if ui.button(label).clicked() {
                            self.settings.rules.push(rule);
                            changed = true;
                        }
                    }
                });
            } else {
                let mut remove = None;
                widgets::card(ui, &pal, |ui| {
                    ui.set_width(ui.available_width());
                    ui.label(RichText::new(format!("Rules ({})", self.settings.rules.len())).size(15.0).strong());
                    ui.add_space(4.0);
                    for (i, r) in self.settings.rules.iter_mut().enumerate() {
                        let matching = s.procs.iter().filter(|p| native::wildcard_match(r.pattern.trim(), &p.name)).count();
                        egui::Frame::new()
                            .stroke(Stroke::new(1.0f32, pal.border))
                            .corner_radius(8.0)
                            .inner_margin(egui::Margin::symmetric(10, 6))
                            .show(ui, |ui| {
                                ui.set_width(ui.available_width());
                                ui.horizontal(|ui| {
                                    if crate::ui::system::toggle_switch(ui, &pal, &mut r.enabled).clicked() {
                                        changed = true;
                                    }
                                    ui.label(RichText::new(&r.pattern).monospace().strong());
                                    ui.label(RichText::new("⏵").color(pal.text_faint));
                                    ui.label(r.action.describe());
                                    ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                                        if ui.small_button("🗑").on_hover_text("Delete rule").clicked() {
                                            remove = Some(i);
                                        }
                                        widgets::pill(ui, &format!("{matching} running"), if matching > 0 { pal.accent } else { pal.text_faint });
                                    });
                                });
                            });
                        ui.add_space(4.0);
                    }
                });
                if let Some(i) = remove {
                    self.settings.rules.remove(i);
                    changed = true;
                }
            }
            ui.add_space(8.0);
            widgets::faint(ui, &pal, "Rules that set priority, affinity or block apply once to each process, including ones already running. Everything a rule does is recorded in the Activity log.");
            let _ = (fmt_bytes, Sense::hover());
        });
        if changed {
            self.toast(ToastKind::Ok, "Rules updated");
        }
    }
}

const RULE_KINDS: [&str; 6] = [
    "Set its priority",
    "Limit it to certain CPUs (affinity)",
    "Block it (end it whenever it starts)",
    "Tame it if it hogs the CPU (lower priority)",
    "Notify me if it hogs the CPU",
    "Notify me if it uses too much memory",
];
