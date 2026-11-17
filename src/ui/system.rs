//! Connections, Startup apps, Services and Settings pages.

use std::collections::HashMap;
use std::time::Duration;

use eframe::egui::{self, vec2, Align, Layout, RichText, Sense, Stroke};
use egui_extras::{Column, TableBuilder};

use crate::app::{AddStartupDlg, App, Confirm, ConfirmAction, Follow, ToastKind, ViewMode};
use crate::platform::{self, ServiceAction};
use crate::ui::theme::{self, ACCENTS};
use crate::ui::widgets::{self, fmt_bytes, fmt_rate};

fn table_frame<R>(
    ui: &mut egui::Ui,
    pal: &theme::Palette,
    add: impl FnOnce(&mut egui::Ui) -> R,
) -> R {
    egui::Frame::new()
        .fill(pal.card)
        .stroke(Stroke::new(1.0f32, pal.border))
        .corner_radius(12.0)
        .inner_margin(egui::Margin::symmetric(6, 4))
        .show(ui, add)
        .inner
}

/// Full-width error banner shown under a page toolbar.
fn error_banner(ui: &mut egui::Ui, pal: &theme::Palette, error: &Option<String>) {
    if let Some(e) = error {
        egui::Frame::new()
            .fill(widgets::with_alpha(pal.bad, 0.12))
            .corner_radius(8.0)
            .inner_margin(egui::Margin::symmetric(12, 8))
            .show(ui, |ui| {
                ui.set_width(ui.available_width());
                let hint = if platform::is_admin() {
                    ""
                } else {
                    "  Some information needs administrator rights."
                };
                ui.add(
                    egui::Label::new(
                        RichText::new(format!("⚠ Couldn't load this list: {e}.{hint}"))
                            .color(pal.bad),
                    )
                    .wrap(),
                );
            });
        ui.add_space(6.0);
    }
}

fn head(ui: &mut egui::Ui, pal: &theme::Palette, t: &str) {
    ui.label(RichText::new(t).size(11.0).color(pal.text_dim));
}

fn loading_row(
    ui: &mut egui::Ui,
    pal: &theme::Palette,
    loading: bool,
    _error: &Option<String>,
    age: Option<Duration>,
) {
    if loading {
        ui.spinner();
        widgets::faint(ui, pal, "Loading…");
    } else if let Some(a) = age {
        widgets::faint(ui, pal, format!("Updated {}s ago", a.as_secs()));
    }
}

impl App {
    // ───────────────────────────── Connections ─────────────────────────────

    pub fn page_network(&mut self, ui: &mut egui::Ui) {
        let pal = self.pal;
        let s = self.snap.clone();
        if self.conns.loaded_at.is_none() && !self.conns.loading {
            self.load_conns();
        }
        if self.conn_auto && self.conns.age().is_some_and(|a| a > Duration::from_secs(5)) {
            self.load_conns();
        }

        ui.horizontal(|ui| {
            ui.add(
                egui::TextEdit::singleline(&mut self.conn_filter)
                    .hint_text("🔍  Filter by process, address, port or state")
                    .desired_width(320.0),
            );
            ui.checkbox(&mut self.conn_established_only, "Established only");
            ui.checkbox(&mut self.conn_external_only, "Internet only")
                .on_hover_text("Hide loopback, LAN and listening sockets");
            ui.checkbox(&mut self.conn_auto, "Auto-refresh");
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                if ui.button("⟳ Refresh").clicked() {
                    self.load_conns();
                }
                loading_row(
                    ui,
                    &pal,
                    self.conns.loading,
                    &self.conns.error,
                    self.conns.age(),
                );
            });
        });
        ui.add_space(6.0);
        error_banner(ui, &pal, &self.conns.error);

        // ── Bandwidth by app ──
        match &s.net_status {
            Some(crate::native::NetStatus::Running) => {
                let mut v: Vec<_> = s.procs.iter().filter(|p| p.net_rx + p.net_tx > 0).collect();
                v.sort_by_key(|p| std::cmp::Reverse(p.net_rx + p.net_tx));
                egui::Frame::new()
                    .fill(pal.card)
                    .stroke(Stroke::new(1.0f32, pal.border))
                    .corner_radius(12.0)
                    .inner_margin(egui::Margin::same(12))
                    .show(ui, |ui| {
                        ui.set_width(ui.available_width());
                        widgets::section_title(ui, &pal, "Bandwidth by app (live)");
                        if v.is_empty() {
                            widgets::faint(ui, &pal, "No network traffic right now.");
                        }
                        let maxv = v
                            .first()
                            .map(|p| (p.net_rx + p.net_tx) as f32)
                            .unwrap_or(1.0)
                            .max(1.0);
                        egui::Grid::new("bw")
                            .num_columns(4)
                            .spacing(vec2(16.0, 4.0))
                            .show(ui, |ui| {
                                for p in v.iter().take(6) {
                                    ui.horizontal(|ui| {
                                        widgets::avatar(ui, &p.name, pal.dark);
                                        ui.label(&*p.name);
                                    });
                                    ui.label(
                                        RichText::new(format!("⬇ {}", fmt_rate(p.net_rx as f32)))
                                            .color(pal.net_rx),
                                    );
                                    ui.label(
                                        RichText::new(format!("⬆ {}", fmt_rate(p.net_tx as f32)))
                                            .color(pal.net_tx),
                                    );
                                    ui.allocate_ui(vec2(220.0, 12.0), |ui| {
                                        ui.set_width(220.0);
                                        widgets::bar(
                                            ui,
                                            &pal,
                                            (p.net_rx + p.net_tx) as f32 / maxv,
                                            pal.net_rx,
                                            6.0,
                                        );
                                    });
                                    ui.end_row();
                                }
                            });
                    });
                ui.add_space(8.0);
            }
            Some(crate::native::NetStatus::NeedsAdmin) => {
                widgets::faint(ui, &pal, "ℹ Per-app download/upload speed needs administrator rights — use “Restart as admin” at the top.");
                ui.add_space(6.0);
            }
            _ => {}
        }

        let names: HashMap<u32, String> = s
            .procs
            .iter()
            .map(|p| (p.pid, p.name.to_string()))
            .collect();
        let q = self.conn_filter.to_lowercase();
        let rows: Vec<_> = self
            .conns
            .data
            .iter()
            .filter(|c| !self.conn_established_only || c.state == "ESTABLISHED")
            .filter(|c| !self.conn_external_only || c.is_external())
            .map(|c| {
                (
                    c,
                    names.get(&c.pid).cloned().unwrap_or_else(|| {
                        if c.pid == 0 {
                            "System Idle".into()
                        } else {
                            format!("PID {}", c.pid)
                        }
                    }),
                )
            })
            .filter(|(c, n)| {
                q.is_empty()
                    || crate::ui::processes::contains_ci(n, &q)
                    || crate::ui::processes::contains_ci(&c.local, &q)
                    || crate::ui::processes::contains_ci(&c.remote, &q)
                    || crate::ui::processes::contains_ci(&c.state, &q)
                    || c.pid.to_string() == q
            })
            .collect();

        // Summary chips
        let est = self
            .conns
            .data
            .iter()
            .filter(|c| c.state == "ESTABLISHED")
            .count();
        let listen = self
            .conns
            .data
            .iter()
            .filter(|c| c.state == "LISTENING" || c.state == "LISTEN")
            .count();
        let ext = self
            .conns
            .data
            .iter()
            .filter(|c| c.is_external() && c.state == "ESTABLISHED")
            .count();
        let mut per_app: HashMap<&str, usize> = HashMap::new();
        for (c, n) in &rows {
            if c.state == "ESTABLISHED" {
                *per_app.entry(n.as_str()).or_default() += 1;
            }
        }
        let mut top: Vec<_> = per_app.into_iter().collect();
        top.sort_by(|a, b| b.1.cmp(&a.1));
        ui.horizontal_wrapped(|ui| {
            widgets::pill(ui, &format!("{} total", self.conns.data.len()), pal.accent);
            widgets::pill(ui, &format!("{est} established"), pal.good);
            widgets::pill(ui, &format!("{ext} to the internet"), pal.net_rx);
            widgets::pill(ui, &format!("{listen} listening"), pal.mem);
            if !top.is_empty() {
                widgets::faint(ui, &pal, "  Most connected:");
                for (n, c) in top.iter().take(4) {
                    widgets::pill(ui, &format!("{n} · {c}"), pal.text_dim);
                }
            }
        });
        ui.add_space(6.0);

        if rows.is_empty() {
            if !self.conns.loading {
                widgets::empty_state(
                    ui,
                    &pal,
                    "🌐",
                    "No connections match",
                    "Adjust the filters above.",
                );
            }
            return;
        }

        let mut kill: Option<(u32, String)> = None;
        let mut goto: Option<u32> = None;
        table_frame(ui, &pal, |ui| {
            TableBuilder::new(ui)
                .striped(true)
                .resizable(true)
                .sense(Sense::click())
                .cell_layout(Layout::left_to_right(Align::Center))
                .auto_shrink([false, false])
                .column(Column::initial(200.0).at_least(120.0).clip(true))
                .column(Column::initial(70.0))
                .column(Column::initial(56.0))
                .column(Column::initial(200.0).clip(true))
                .column(Column::initial(220.0).clip(true))
                .column(Column::remainder().at_least(90.0))
                .header(26.0, |mut h| {
                    for t in [
                        "Process",
                        "PID",
                        "Proto",
                        "Local address",
                        "Remote address",
                        "State",
                    ] {
                        h.col(|ui| head(ui, &pal, t));
                    }
                })
                .body(|body| {
                    body.rows(26.0, rows.len(), |mut row| {
                        let (c, name) = &rows[row.index()];
                        row.col(|ui| {
                            widgets::avatar(ui, name, pal.dark);
                            ui.label(name);
                        });
                        row.col(|ui| {
                            ui.label(RichText::new(c.pid.to_string()).color(pal.text_dim));
                        });
                        row.col(|ui| {
                            ui.label(RichText::new(&c.proto).color(pal.text_dim));
                        });
                        row.col(|ui| {
                            ui.label(RichText::new(&c.local).monospace());
                        });
                        row.col(|ui| {
                            let col = if c.is_external() {
                                pal.net_rx
                            } else {
                                pal.text_dim
                            };
                            ui.label(RichText::new(&c.remote).monospace().color(col));
                        });
                        row.col(|ui| {
                            let col = match c.state.as_str() {
                                "ESTABLISHED" | "ESTAB" => pal.good,
                                "LISTENING" | "LISTEN" => pal.mem,
                                "" => pal.text_faint,
                                _ => pal.warn,
                            };
                            if !c.state.is_empty() {
                                widgets::pill(ui, &c.state, col);
                            }
                        });
                        row.response().context_menu(|ui| {
                            if ui.button("☰  Show process").clicked() {
                                goto = Some(c.pid);
                                ui.close_menu();
                            }
                            if ui.button("✖  End process").clicked() {
                                kill = Some((c.pid, name.clone()));
                                ui.close_menu();
                            }
                            ui.separator();
                            let host = c
                                .remote_host()
                                .trim_matches(|ch| ch == '[' || ch == ']')
                                .to_string();
                            if ui
                                .add_enabled(
                                    c.is_external(),
                                    egui::Button::new("🔎  Look up remote IP"),
                                )
                                .clicked()
                            {
                                platform::open_url(&format!("https://ipinfo.io/{host}"));
                                ui.close_menu();
                            }
                            if ui.button("📋  Copy remote address").clicked() {
                                ui.ctx().copy_text(c.remote.clone());
                                ui.close_menu();
                            }
                            if ui.button("📋  Copy local address").clicked() {
                                ui.ctx().copy_text(c.local.clone());
                                ui.close_menu();
                            }
                        });
                    });
                });
        });
        if let Some((pid, n)) = kill {
            self.request_kill(pid, &n, false);
        }
        if let Some(pid) = goto {
            self.reveal_process(pid);
        }
    }

    // ───────────────────────────── Startup ─────────────────────────────

    pub fn page_startup(&mut self, ui: &mut egui::Ui) {
        let pal = self.pal;
        if self.startup.loaded_at.is_none() && !self.startup.loading {
            self.load_startup();
        }
        let n_tasks = self
            .sched
            .data
            .iter()
            .filter(|t| self.show_ms_tasks || !t.is_microsoft())
            .count();
        let n_svc = platform::third_party_autostart(&self.services.data).len();
        ui.horizontal(|ui| {
            for (i, label) in [
                format!("🚀 Apps ({})", self.startup.data.len()),
                format!(
                    "📅 Scheduled tasks{}",
                    if self.sched.loaded_at.is_some() {
                        format!(" ({n_tasks})")
                    } else {
                        String::new()
                    }
                ),
                format!(
                    "⚙ Services{}",
                    if self.services.loaded_at.is_some() {
                        format!(" ({n_svc})")
                    } else {
                        String::new()
                    }
                ),
            ]
            .into_iter()
            .enumerate()
            {
                if ui
                    .selectable_label(self.startup_tab == i, RichText::new(label).size(14.0))
                    .clicked()
                {
                    self.startup_tab = i;
                }
            }
        });
        ui.add_space(6.0);
        match self.startup_tab {
            1 => self.startup_tasks_tab(ui),
            2 => self.startup_services_tab(ui),
            _ => self.startup_apps_tab(ui),
        }
        let _ = pal;
    }

    fn startup_apps_tab(&mut self, ui: &mut egui::Ui) {
        let pal = self.pal;
        let s = self.snap.clone();
        let impacts = self
            .mon
            .shared
            .lock()
            .map(|g| g.impact.results.clone())
            .unwrap_or_default();
        let enabled = self.startup.data.iter().filter(|i| i.enabled).count();
        ui.horizontal(|ui| {
            widgets::pill(ui, &format!("{enabled} enabled"), pal.good);
            widgets::pill(
                ui,
                &format!("{} disabled", self.startup.data.len() - enabled),
                pal.text_dim,
            );
            if !platform::is_admin() {
                widgets::faint(
                    ui,
                    &pal,
                    "  “All users” items need administrator rights to change.",
                );
            }
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                if theme::primary_button(ui, &pal, "+ Add app").clicked() {
                    self.add_startup = Some(AddStartupDlg::default());
                }
                if ui.button("⟳ Refresh").clicked() {
                    self.load_startup();
                }
                loading_row(ui, &pal, self.startup.loading, &self.startup.error, None);
            });
        });
        ui.add_space(8.0);
        error_banner(ui, &pal, &self.startup.error);

        if self.startup.data.is_empty() {
            if !self.startup.loading {
                widgets::empty_state(
                    ui,
                    &pal,
                    "🚀",
                    "No startup apps found",
                    "Nothing is set to launch automatically.",
                );
            }
            return;
        }

        // Memory currently used by each startup app, matched by executable name.
        let mut running: HashMap<String, (usize, u64)> = HashMap::new();
        for p in &s.procs {
            let e = running.entry(p.name.to_lowercase()).or_default();
            e.0 += 1;
            e.1 += p.mem;
        }

        let items = self.startup.data.clone();
        let mut toggle: Option<(usize, bool)> = None;
        let mut del: Option<usize> = None;
        table_frame(ui, &pal, |ui| {
            TableBuilder::new(ui)
                .striped(true)
                .resizable(true)
                .sense(Sense::click())
                .cell_layout(Layout::left_to_right(Align::Center))
                .auto_shrink([false, false])
                .column(Column::initial(220.0).at_least(140.0).clip(true))
                .column(Column::initial(110.0))
                .column(Column::initial(96.0))
                .column(Column::initial(120.0))
                .column(Column::initial(190.0).clip(true))
                .column(Column::remainder().clip(true))
                .header(26.0, |mut h| {
                    for t in ["Name", "Status", "Startup impact", "Running now", "Source", "Command"] {
                        h.col(|ui| head(ui, &pal, t));
                    }
                })
                .body(|body| {
                    body.rows(34.0, items.len(), |mut row| {
                        let i = row.index();
                        let it = &items[i];
                        row.col(|ui| {
                            widgets::avatar(ui, &it.name, pal.dark);
                            ui.label(RichText::new(&*it.name).strong());
                            if it.needs_admin && !platform::is_admin() {
                                ui.label(RichText::new("🔒").color(pal.text_faint))
                                    .on_hover_text(
                                        "Changing this item requires administrator rights",
                                    );
                            }
                        });
                        row.col(|ui| {
                            let mut on = it.enabled;
                            if toggle_switch(ui, &pal, &mut on).clicked() {
                                toggle = Some((i, on));
                            }
                            ui.label(
                                RichText::new(if it.enabled { "Enabled" } else { "Disabled" })
                                    .color(if it.enabled { pal.good } else { pal.text_faint }),
                            );
                        });
                        let exe_name = std::path::Path::new(&*it.exe)
                            .file_name()
                            .map(|f| f.to_string_lossy().to_lowercase())
                            .unwrap_or_default();
                        row.col(|ui| match impacts.get(&exe_name) {
                            Some(m) => {
                                let col = match m.level {
                                    crate::insights::ImpactLevel::High => pal.bad,
                                    crate::insights::ImpactLevel::Medium => pal.warn,
                                    crate::insights::ImpactLevel::Low => pal.good,
                                };
                                widgets::pill(ui, m.level.label(), col).on_hover_text(format!(
                                    "In its first minute: {:.1} s of CPU time and {} of disk I/O.\nStarted {} s after boot · measured {}",
                                    m.cpu_ms as f32 / 1000.0,
                                    fmt_bytes(m.disk_bytes),
                                    m.after_boot_s,
                                    widgets::fmt_epoch(m.measured as u64)
                                ));
                            }
                            None => {
                                ui.label(RichText::new("Not measured").color(pal.text_faint)).on_hover_text(
                                    "Measured automatically when TaskForge is running at sign-in.\nTurn on Settings › Installation › “Start when I sign in”.",
                                );
                            }
                        });
                        row.col(|ui| {
                            match running.get(&exe_name) {
                                Some((n, mem)) => {
                                    ui.label(
                                        RichText::new(format!("• {}", fmt_bytes(*mem)))
                                            .color(pal.good),
                                    )
                                    .on_hover_text(format!("{n} running instance(s)"));
                                }
                                None => {
                                    ui.label(RichText::new("—").color(pal.text_faint));
                                }
                            }
                        });
                        row.col(|ui| {
                            ui.label(RichText::new(&it.source).color(pal.text_dim));
                        });
                        row.col(|ui| {
                            ui.label(RichText::new(&it.command).monospace().color(pal.text_dim))
                                .on_hover_text(&it.command);
                        });
                        row.response().context_menu(|ui| {
                            let label = if it.enabled {
                                "⏸  Disable"
                            } else {
                                "▶  Enable"
                            };
                            if ui.button(label).clicked() {
                                toggle = Some((i, !it.enabled));
                                ui.close_menu();
                            }
                            if ui.button("📂  Open file location").clicked() {
                                let _ = platform::open_location(&platform::expand_env(&it.exe));
                                ui.close_menu();
                            }
                            if ui.button("🔎  Search online").clicked() {
                                platform::open_url(&format!(
                                    "https://www.bing.com/search?q={}",
                                    platform::url_encode(&format!("{} startup", it.name))
                                ));
                                ui.close_menu();
                            }
                            if ui.button("📋  Copy command").clicked() {
                                ui.ctx().copy_text(it.command.clone());
                                ui.close_menu();
                            }
                            ui.separator();
                            if ui
                                .button(RichText::new("🗑  Remove from startup").color(pal.bad))
                                .clicked()
                            {
                                del = Some(i);
                                ui.close_menu();
                            }
                        });
                    });
                });
        });

        if let Some((i, on)) = toggle {
            let item = items[i].clone();
            if let Some(d) = self.startup.data.get_mut(i) {
                d.enabled = on; // optimistic update
            }
            let n = item.name.clone();
            self.job(0, &n, Follow::Startup, false, move || {
                platform::set_startup_enabled(&item, on)
            });
        }
        if let Some(i) = del {
            let item = items[i].clone();
            self.confirm = Some(Confirm {
                title: format!("Remove {}?", item.name),
                body: "This deletes the startup entry entirely. Prefer “Disable” if you might want it back.".into(),
                warn: None,
                action: ConfirmAction::DeleteStartup(item),
            });
        }
    }

    fn startup_tasks_tab(&mut self, ui: &mut egui::Ui) {
        let pal = self.pal;
        if self.sched.loaded_at.is_none() && !self.sched.loading {
            self.sched
                .load(&self.ctx.clone(), platform::list_scheduled_startup);
        }
        ui.horizontal(|ui| {
            widgets::faint(ui, &pal, "Tasks set to run when you sign in or when Windows starts — a common hiding place for auto-start apps.");
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                if ui.button("⟳ Refresh").clicked() {
                    self.sched.load(&self.ctx.clone(), platform::list_scheduled_startup);
                }
                ui.checkbox(&mut self.show_ms_tasks, "Show Microsoft tasks");
                loading_row(ui, &pal, self.sched.loading, &self.sched.error, None);
            });
        });
        ui.add_space(6.0);
        error_banner(ui, &pal, &self.sched.error);
        let tasks: Vec<platform::SchedTask> = self
            .sched
            .data
            .iter()
            .filter(|t| self.show_ms_tasks || !t.is_microsoft())
            .cloned()
            .collect();
        if tasks.is_empty() {
            if !self.sched.loading && self.sched.error.is_none() {
                widgets::empty_state(
                    ui,
                    &pal,
                    "📅",
                    "No sign-in or boot tasks",
                    "Nothing (besides Windows itself) is scheduled to start automatically.",
                );
            }
            return;
        }
        let mut toggle: Option<(platform::SchedTask, bool)> = None;
        table_frame(ui, &pal, |ui| {
            TableBuilder::new(ui)
                .striped(true)
                .resizable(true)
                .sense(Sense::click())
                .cell_layout(Layout::left_to_right(Align::Center))
                .auto_shrink([false, false])
                .column(Column::initial(260.0).at_least(140.0).clip(true))
                .column(Column::initial(110.0))
                .column(Column::initial(100.0))
                .column(Column::initial(160.0).clip(true))
                .column(Column::remainder().clip(true))
                .header(26.0, |mut h| {
                    for t in ["Task", "Status", "Runs at", "Folder", "Runs"] {
                        h.col(|ui| head(ui, &pal, t));
                    }
                })
                .body(|body| {
                    body.rows(34.0, tasks.len(), |mut row| {
                        let t = &tasks[row.index()];
                        row.col(|ui| {
                            widgets::avatar(ui, &t.name, pal.dark);
                            ui.label(RichText::new(&*t.name).strong()).on_hover_text(
                                if t.author.is_empty() {
                                    String::new()
                                } else {
                                    format!("Author: {}", t.author)
                                },
                            );
                        });
                        row.col(|ui| {
                            let mut on = t.enabled();
                            if toggle_switch(ui, &pal, &mut on).clicked() {
                                toggle = Some((t.clone(), on));
                            }
                            ui.label(
                                RichText::new(if t.enabled() { "Enabled" } else { "Disabled" })
                                    .color(if t.enabled() {
                                        pal.good
                                    } else {
                                        pal.text_faint
                                    }),
                            );
                        });
                        row.col(|ui| {
                            ui.label(RichText::new(&t.trigger).color(pal.text_dim));
                        });
                        row.col(|ui| {
                            ui.label(RichText::new(&t.path).color(pal.text_dim));
                        });
                        row.col(|ui| {
                            ui.label(RichText::new(&t.action).monospace().color(pal.text_dim))
                                .on_hover_text(&t.action);
                        });
                        row.response().context_menu(|ui| {
                            if ui
                                .button(if t.enabled() {
                                    "⏸  Disable"
                                } else {
                                    "▶  Enable"
                                })
                                .clicked()
                            {
                                toggle = Some((t.clone(), !t.enabled()));
                                ui.close_menu();
                            }
                            if ui.button("📂  Open file location").clicked() {
                                let _ = platform::open_location(&t.exe());
                                ui.close_menu();
                            }
                            if ui.button("🔎  Search online").clicked() {
                                platform::open_url(&format!(
                                    "https://www.bing.com/search?q={}",
                                    platform::url_encode(&format!("{} scheduled task", t.name))
                                ));
                                ui.close_menu();
                            }
                            if ui.button("📅  Open Task Scheduler").clicked() {
                                let _ = platform::launch_tool("taskschd.msc");
                                ui.close_menu();
                            }
                        });
                    });
                });
        });
        if let Some((t, on)) = toggle {
            if let Some(d) = self
                .sched
                .data
                .iter_mut()
                .find(|x| x.name == t.name && x.path == t.path)
            {
                d.state = if on {
                    "Ready".into()
                } else {
                    "Disabled".into()
                };
            }
            let n = t.name.clone();
            let ctx = self.ctx.clone();
            let _ = ctx;
            self.job(0, &n, Follow::Sched, false, move || {
                platform::set_task_enabled(&t, on)
            });
        }
    }

    fn startup_services_tab(&mut self, ui: &mut egui::Ui) {
        let pal = self.pal;
        if self.services.loaded_at.is_none() && !self.services.loading {
            self.load_services();
        }
        ui.horizontal(|ui| {
            widgets::faint(ui, &pal, "Non-Windows services that start automatically with your PC. Set ones you don't need to Manual — they'll still start when an app asks for them.");
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                if ui.button("⟳ Refresh").clicked() {
                    self.load_services();
                }
                loading_row(ui, &pal, self.services.loading, &self.services.error, None);
            });
        });
        ui.add_space(6.0);
        error_banner(ui, &pal, &self.services.error);
        let list = platform::third_party_autostart(&self.services.data);
        if list.is_empty() {
            if !self.services.loading {
                widgets::empty_state(
                    ui,
                    &pal,
                    "⚙",
                    "No third-party auto-start services",
                    "Only Windows' own services start automatically.",
                );
            }
            return;
        }
        let mut change: Option<(String, &'static str)> = None;
        table_frame(ui, &pal, |ui| {
            TableBuilder::new(ui)
                .striped(true)
                .resizable(true)
                .sense(Sense::click())
                .cell_layout(Layout::left_to_right(Align::Center))
                .auto_shrink([false, false])
                .column(Column::initial(260.0).at_least(140.0).clip(true))
                .column(Column::initial(96.0))
                .column(Column::initial(160.0))
                .column(Column::remainder().clip(true))
                .header(26.0, |mut h| {
                    for t in ["Service", "Status", "Start type", "Program"] {
                        h.col(|ui| head(ui, &pal, t));
                    }
                })
                .body(|body| {
                    body.rows(34.0, list.len(), |mut row| {
                        let sv = &list[row.index()];
                        row.col(|ui| {
                            ui.label(RichText::new(&sv.display).strong())
                                .on_hover_text(&sv.description);
                        });
                        row.col(|ui| {
                            let col = if sv.state.eq_ignore_ascii_case("running") {
                                pal.good
                            } else {
                                pal.text_faint
                            };
                            widgets::pill(ui, &sv.state, col);
                        });
                        row.col(|ui| {
                            egui::ComboBox::from_id_salt(("svc_start", &sv.name))
                                .selected_text(&sv.start_mode)
                                .width(150.0)
                                .show_ui(ui, |ui| {
                                    for (label, mode) in platform::START_MODES {
                                        if ui
                                            .selectable_label(sv.start_mode == label, label)
                                            .clicked()
                                        {
                                            change = Some((sv.name.clone(), mode));
                                        }
                                    }
                                });
                        });
                        row.col(|ui| {
                            ui.label(RichText::new(&sv.path).monospace().color(pal.text_dim))
                                .on_hover_text(&sv.path);
                        });
                    });
                });
        });
        if let Some((name, mode)) = change {
            let n = name.clone();
            self.job(0, &n, Follow::Services, false, move || {
                platform::set_service_start(&name, mode)
            });
        }
    }

    // ───────────────────────────── Services ─────────────────────────────

    pub fn page_services(&mut self, ui: &mut egui::Ui) {
        let pal = self.pal;
        if self.services.loaded_at.is_none() && !self.services.loading {
            self.load_services();
        }
        let running = self
            .services
            .data
            .iter()
            .filter(|s| s.state.eq_ignore_ascii_case("running"))
            .count();
        ui.horizontal(|ui| {
            ui.add(
                egui::TextEdit::singleline(&mut self.svc_filter)
                    .hint_text("🔍  Search services")
                    .desired_width(280.0),
            );
            for (i, l) in ["All", "Running", "Stopped"].iter().enumerate() {
                if ui.selectable_label(self.svc_state == i, *l).clicked() {
                    self.svc_state = i;
                }
            }
            widgets::pill(
                ui,
                &format!("{running} running / {}", self.services.data.len()),
                pal.good,
            );
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                let sel = self.svc_selected.clone();
                let has = sel.is_some();
                if ui
                    .add_enabled(has, egui::Button::new("⟲ Restart"))
                    .clicked()
                {
                    self.svc_action(sel.clone().unwrap(), ServiceAction::Restart);
                }
                if ui.add_enabled(has, egui::Button::new("■ Stop")).clicked() {
                    self.svc_stop(sel.clone().unwrap());
                }
                if ui.add_enabled(has, egui::Button::new("▶ Start")).clicked() {
                    self.svc_action(sel.unwrap(), ServiceAction::Start);
                }
                if ui.button("⟳").on_hover_text("Refresh").clicked() {
                    self.load_services();
                }
                loading_row(ui, &pal, self.services.loading, &self.services.error, None);
            });
        });
        ui.add_space(8.0);
        error_banner(ui, &pal, &self.services.error);

        let q = self.svc_filter.to_lowercase();
        let st = self.svc_state;
        let rows: Vec<_> = self
            .services
            .data
            .iter()
            .filter(|s| match st {
                1 => s.state.eq_ignore_ascii_case("running"),
                2 => !s.state.eq_ignore_ascii_case("running"),
                _ => true,
            })
            .filter(|s| {
                q.is_empty()
                    || crate::ui::processes::contains_ci(&s.name, &q)
                    || crate::ui::processes::contains_ci(&s.display, &q)
                    || crate::ui::processes::contains_ci(&s.description, &q)
                    || (s.pid > 0 && s.pid.to_string() == q)
            })
            .cloned()
            .collect();

        if rows.is_empty() {
            if !self.services.loading {
                widgets::empty_state(ui, &pal, "⚙", "No services match", "Try another search.");
            }
            return;
        }

        let mut select: Option<String> = None;
        let mut act: Option<(String, ServiceAction)> = None;
        let mut start_mode: Option<(String, &'static str)> = None;
        let mut goto: Option<u32> = None;
        table_frame(ui, &pal, |ui| {
            TableBuilder::new(ui)
                .striped(true)
                .resizable(true)
                .sense(Sense::click())
                .cell_layout(Layout::left_to_right(Align::Center))
                .auto_shrink([false, false])
                .column(Column::initial(260.0).at_least(140.0).clip(true))
                .column(Column::initial(160.0).clip(true))
                .column(Column::initial(96.0))
                .column(Column::initial(150.0))
                .column(Column::initial(70.0))
                .column(Column::remainder().clip(true))
                .header(26.0, |mut h| {
                    for t in [
                        "Display name",
                        "Name",
                        "Status",
                        "Start type",
                        "PID",
                        "Description",
                    ] {
                        h.col(|ui| head(ui, &pal, t));
                    }
                })
                .body(|body| {
                    body.rows(28.0, rows.len(), |mut row| {
                        let sv = &rows[row.index()];
                        row.set_selected(self.svc_selected.as_deref() == Some(&*sv.name));
                        row.col(|ui| {
                            ui.label(&sv.display);
                        });
                        row.col(|ui| {
                            ui.label(RichText::new(&*sv.name).color(pal.text_dim));
                        });
                        row.col(|ui| {
                            let col = match sv.state.to_lowercase().as_str() {
                                "running" => pal.good,
                                "stopped" | "dead" | "inactive" => pal.text_faint,
                                _ => pal.warn,
                            };
                            widgets::pill(ui, &sv.state, col);
                        });
                        row.col(|ui| {
                            let col = if sv.start_mode == "Disabled" {
                                pal.bad
                            } else {
                                pal.text_dim
                            };
                            ui.label(RichText::new(&sv.start_mode).color(col));
                        });
                        row.col(|ui| {
                            if sv.pid > 0
                                && ui
                                    .link(sv.pid.to_string())
                                    .on_hover_text("Show hosting process")
                                    .clicked()
                            {
                                goto = Some(sv.pid);
                            }
                        });
                        row.col(|ui| {
                            ui.label(RichText::new(&sv.description).color(pal.text_dim))
                                .on_hover_ui(|ui| {
                                    ui.set_max_width(420.0);
                                    ui.label(&sv.description);
                                    if !sv.path.is_empty() {
                                        ui.label(
                                            RichText::new(&sv.path).monospace().color(pal.text_dim),
                                        );
                                    }
                                });
                        });
                        let resp = row.response();
                        if resp.clicked() {
                            select = Some(sv.name.clone());
                        }
                        resp.context_menu(|ui| {
                            select = Some(sv.name.clone());
                            for (l, a) in [
                                ("▶  Start", ServiceAction::Start),
                                ("■  Stop", ServiceAction::Stop),
                                ("⟲  Restart", ServiceAction::Restart),
                            ] {
                                if ui.button(l).clicked() {
                                    act = Some((sv.name.clone(), a));
                                    ui.close_menu();
                                }
                            }
                            ui.menu_button("⚙  Start type", |ui| {
                                for (label, mode) in platform::START_MODES {
                                    if ui.radio(sv.start_mode == label, label).clicked() {
                                        start_mode = Some((sv.name.clone(), mode));
                                        ui.close_menu();
                                    }
                                }
                            });
                            ui.separator();
                            if ui
                                .add_enabled(sv.pid > 0, egui::Button::new("☰  Go to process"))
                                .clicked()
                            {
                                goto = Some(sv.pid);
                                ui.close_menu();
                            }
                            if ui.button("🔎  Search online").clicked() {
                                platform::open_url(&format!(
                                    "https://www.bing.com/search?q={}",
                                    platform::url_encode(&format!("{} service windows", sv.name))
                                ));
                                ui.close_menu();
                            }
                            if ui.button("📋  Copy name").clicked() {
                                ui.ctx().copy_text(sv.name.clone());
                                ui.close_menu();
                            }
                        });
                    });
                });
        });
        if let Some(n) = select {
            self.svc_selected = Some(n);
        }
        if let Some((n, a)) = act {
            if a == ServiceAction::Stop {
                self.svc_stop(n);
            } else {
                self.svc_action(n, a);
            }
        }
        if let Some((n, m)) = start_mode {
            let nn = n.clone();
            self.job(0, &nn, Follow::Services, false, move || {
                platform::set_service_start(&n, m)
            });
        }
        if let Some(pid) = goto {
            self.reveal_process(pid);
        }
    }

    fn svc_action(&mut self, name: String, a: ServiceAction) {
        let n = name.clone();
        self.toast(ToastKind::Info, format!("{a:?} {n}…"));
        self.job(0, &n, Follow::Services, false, move || {
            platform::service_action(&name, a)
        });
    }

    fn svc_stop(&mut self, name: String) {
        if self.settings.confirm_kill {
            self.confirm = Some(Confirm {
                title: format!("Stop {name}?"),
                body: "Stopping a service can affect apps and Windows features that depend on it."
                    .into(),
                warn: None,
                action: ConfirmAction::StopService(name),
            });
        } else {
            self.svc_action(name, ServiceAction::Stop);
        }
    }

    // ───────────────────────────── Settings ─────────────────────────────

    pub fn page_settings(&mut self, ui: &mut egui::Ui, ctx: &egui::Context) {
        let pal = self.pal;
        egui::ScrollArea::vertical()
            .auto_shrink([false, false])
            .show(ui, |ui| {
                ui.set_max_width(760.0);
                let section =
                    |ui: &mut egui::Ui, title: &str, add: &mut dyn FnMut(&mut egui::Ui)| {
                        widgets::card(ui, &pal, |ui| {
                            ui.set_width(ui.available_width());
                            ui.label(RichText::new(title).size(15.0).strong());
                            ui.add_space(6.0);
                            add(ui);
                        });
                        ui.add_space(10.0);
                    };

                section(ui, "Appearance", &mut |ui| {
                    ui.horizontal(|ui| {
                        ui.label("Theme");
                        ui.selectable_value(&mut self.settings.dark, true, "🌙 Dark");
                        ui.selectable_value(&mut self.settings.dark, false, "☀ Light");
                    });
                    ui.horizontal(|ui| {
                        ui.label("Accent");
                        for (i, (name, col)) in ACCENTS.iter().enumerate() {
                            let (r, resp) =
                                ui.allocate_exact_size(vec2(26.0, 26.0), Sense::click());
                            ui.painter().circle_filled(r.center(), 10.0, *col);
                            if self.settings.accent == i {
                                ui.painter().circle_stroke(
                                    r.center(),
                                    12.5,
                                    Stroke::new(2.0f32, pal.text),
                                );
                            }
                            if resp.on_hover_text(*name).clicked() {
                                self.settings.accent = i;
                            }
                        }
                    });
                });

                section(ui, "Updates & graphs", &mut |ui| {
                    ui.horizontal(|ui| {
                        ui.label("Update speed");
                        for (ms, l) in [
                            (500, "Fast (0.5 s)"),
                            (1000, "Normal (1 s)"),
                            (2000, "Slow (2 s)"),
                            (5000, "Very slow (5 s)"),
                        ] {
                            ui.selectable_value(&mut self.settings.refresh_ms, ms, l);
                        }
                    });
                    ui.horizontal(|ui| {
                        ui.label("Graph history");
                        for (s, l) in [(60, "1 min"), (300, "5 min"), (900, "15 min")] {
                            ui.selectable_value(&mut self.settings.graph_secs, s, l);
                        }
                    });
                    widgets::faint(
                        ui,
                        &pal,
                        "History is kept for up to 900 samples (15 minutes at 1 s).",
                    );
                });

                section(ui, "Processes", &mut |ui| {
                    ui.horizontal(|ui| {
                        ui.label("Default view");
                        ui.selectable_value(
                            &mut self.settings.view_mode,
                            ViewMode::Grouped,
                            "Grouped",
                        );
                        ui.selectable_value(&mut self.settings.view_mode, ViewMode::Tree, "Tree");
                        ui.selectable_value(&mut self.settings.view_mode, ViewMode::Flat, "Flat");
                    });
                    ui.checkbox(
                        &mut self.settings.heatmap,
                        "Heat-map colouring for CPU, memory and disk cells",
                    );
                    ui.checkbox(&mut self.settings.compact_rows, "Compact rows");
                    ui.checkbox(
                        &mut self.settings.show_details,
                        "Show details panel for the selected process",
                    );
                    ui.checkbox(
                        &mut self.settings.confirm_kill,
                        "Ask before ending tasks and stopping services",
                    );
                    ui.horizontal(|ui| {
                        ui.label(format!(
                            "Pinned apps: {}",
                            if self.settings.pinned.is_empty() {
                                "none".into()
                            } else {
                                self.settings.pinned.join(", ")
                            }
                        ));
                        if !self.settings.pinned.is_empty() && ui.small_button("Clear").clicked() {
                            self.settings.pinned.clear();
                            self.rows_gen += 1;
                        }
                    });
                });

                section(ui, "Alerts", &mut |ui| {
                    ui.checkbox(
                        &mut self.settings.alerts_enabled,
                        "Notify me when the system is under sustained load",
                    );
                    ui.add_enabled_ui(self.settings.alerts_enabled, |ui| {
                        ui.horizontal(|ui| {
                            ui.label("CPU above");
                            ui.add(
                                egui::Slider::new(&mut self.settings.alert_cpu, 50.0..=100.0)
                                    .suffix(" %")
                                    .integer(),
                            );
                        });
                        ui.horizontal(|ui| {
                            ui.label("Memory above");
                            ui.add(
                                egui::Slider::new(&mut self.settings.alert_mem, 50.0..=100.0)
                                    .suffix(" %")
                                    .integer(),
                            );
                        });
                        ui.horizontal(|ui| {
                            ui.label("For at least");
                            ui.add(
                                egui::Slider::new(&mut self.settings.alert_secs, 3..=120)
                                    .suffix(" s"),
                            );
                        });
                    });
                    widgets::faint(
                        ui,
                        &pal,
                        "Alerts name the heaviest process and are saved to the Activity log.",
                    );
                });

                section(ui, "Window", &mut |ui| {
                    ui.checkbox(&mut self.settings.always_on_top, "Always on top");
                    if platform::IS_WINDOWS {
                        ui.checkbox(
                            &mut self.settings.tray_icon,
                            "Show a live CPU/GPU icon in the system tray (takes effect after restart)",
                        );
                        ui.add_enabled_ui(self.settings.tray_icon, |ui| {
                            ui.checkbox(
                                &mut self.settings.close_to_tray,
                                "Closing the window keeps TaskForge running in the tray (rules and alerts keep working)",
                            );
                        });
                    }
                    ui.horizontal(|ui| {
                        if ui.button("🗗 Open mini monitor").clicked() {
                            self.mini_request = true;
                        }
                        widgets::faint(ui, &pal, "A compact, always-on-top live view (Ctrl+M).");
                    });
                });

                if platform::IS_WINDOWS {
                section(ui, "Installation", &mut |ui| {
                    if platform::running_installed() {
                        ui.label(RichText::new("✔ Installed").color(pal.good).strong());
                        widgets::faint(ui, &pal, platform::install_dir().map(|d| d.display().to_string()).unwrap_or_default());
                    } else if platform::is_installed() {
                        ui.label("Installed, but you're running a separate copy.");
                    } else {
                        ui.label("Running without installing (portable).");
                    }
                    let mut login = self.start_at_login;
                    if ui.checkbox(&mut login, "Start when I sign in (mini monitor)").changed() {
                        match platform::set_start_at_login(login) {
                            Ok(()) => self.start_at_login = login,
                            Err(e) => self.toast(ToastKind::Err, e),
                        }
                    }
                    ui.horizontal(|ui| {
                        if !platform::running_installed() && ui.button("Install TaskForge…").clicked() {
                            self.install = Some(crate::app::InstallDlg { desktop: true, login: self.start_at_login, busy: false });
                        }
                        if platform::is_installed() && ui.button(RichText::new("Uninstall…").color(pal.bad)).clicked() {
                            self.confirm = Some(Confirm {
                                title: "Uninstall TaskForge?".into(),
                                body: "Removes TaskForge, its shortcuts and its start-at-sign-in entry. Your settings are kept in case you reinstall.".into(),
                                warn: None,
                                action: ConfirmAction::Uninstall,
                            });
                        }
                    });
                });
            }

            section(ui, "Diagnostics", &mut |ui| {
                self.diagnostics_ui(ui);
            });

            section(ui, "Keyboard shortcuts", &mut |ui| {
                    egui::Grid::new("keys")
                        .num_columns(2)
                        .spacing(vec2(24.0, 4.0))
                        .show(ui, |ui| {
                            for (k, d) in [
                                ("Ctrl+F", "Search processes"),
                                ("Del / Shift+Del", "End task / end process tree"),
                                ("Ctrl+R", "Run new task"),
                                ("Ctrl+E", "Export processes to CSV"),
                                ("Ctrl+P", "Pause / resume updates"),
                                ("Ctrl+M", "Mini monitor"),
                                ("F5", "Refresh now"),
                                ("Ctrl+1 … Ctrl+8", "Switch page"),
                                ("Esc", "Clear search / selection, close dialogs"),
                            ] {
                                ui.label(RichText::new(k).monospace().color(pal.accent));
                                ui.label(d);
                                ui.end_row();
                            }
                        });
                });

                section(ui, "About", &mut |ui| {
                    ui.label(
                        RichText::new(format!("TaskForge {}", env!("CARGO_PKG_VERSION"))).strong(),
                    );
                    widgets::faint(
                        ui,
                        &pal,
                        "A modern Task Manager written in Rust with egui and sysinfo.",
                    );
                    ui.horizontal(|ui| {
                        if ui.button("Reset all settings").clicked() {
                            self.settings = Default::default();
                        }
                    });
                });
            });
        if self.mini_request {
            self.mini_request = false;
            self.set_mini(ctx, true);
        }
        if self.uninstall_request {
            self.uninstall_request = false;
            // Run the installed copy's uninstaller, then close this window.
            if let Some(exe) = platform::installed_exe() {
                match platform::command(&exe.display().to_string())
                    .arg("--uninstall")
                    .spawn()
                {
                    Ok(_) => ctx.send_viewport_cmd(egui::ViewportCommand::Close),
                    Err(e) => {
                        self.toast(ToastKind::Err, format!("Could not start uninstaller: {e}"))
                    }
                }
            }
        }
    }
}

/// iOS-style toggle switch.
pub fn toggle_switch(ui: &mut egui::Ui, pal: &theme::Palette, on: &mut bool) -> egui::Response {
    let size = vec2(34.0, 18.0);
    let (rect, mut resp) = ui.allocate_exact_size(size, Sense::click());
    if resp.clicked() {
        *on = !*on;
        resp.mark_changed();
    }
    let t = ui.ctx().animate_bool_responsive(resp.id, *on);
    let bg = if *on { pal.good } else { pal.track };
    ui.painter().rect_filled(rect, 9.0, bg);
    let x = egui::lerp(rect.left() + 9.0..=rect.right() - 9.0, t);
    ui.painter()
        .circle_filled(egui::pos2(x, rect.center().y), 7.0, egui::Color32::WHITE);
    resp
}
