//! History page: the last 24 hours, with the culprit behind every spike.

use eframe::egui::{self, vec2, Align, Layout, RichText};

use crate::app::App;
use crate::longterm::{self, Point};
use crate::ui::widgets::{self, fmt_bytes, fmt_epoch, fmt_rate};

impl App {
    pub fn page_history(&mut self, ui: &mut egui::Ui) {
        let pal = self.pal;
        let shared = self.mon.shared.clone();
        ui.horizontal(|ui| {
            ui.label(RichText::new("Show the last").color(pal.text_dim));
            for (secs, label) in [
                (900, "15 min"),
                (3600, "1 hour"),
                (6 * 3600, "6 hours"),
                (24 * 3600, "24 hours"),
            ] {
                if ui
                    .selectable_label(self.history_range == secs, label)
                    .clicked()
                {
                    self.history_range = secs;
                }
            }
            widgets::faint(
                ui,
                &pal,
                "· saved every 5 s and kept for 24 hours, even after you close TaskForge",
            );
        });
        ui.add_space(8.0);

        // Rebuilt only when a new point arrives (every 5 s) or the range changes.
        let latest = shared
            .lock()
            .map(|g| g.longterm.points.back().map(|p| p.t).unwrap_or(0))
            .unwrap_or(0);
        let fresh = matches!(&self.history_view, Some((r, t, _)) if *r == self.history_range && *t == latest);
        if !fresh {
            let pts: Vec<Point> = shared
                .lock()
                .map(|g| g.longterm.window(self.history_range).cloned().collect())
                .unwrap_or_default();
            self.history_view = Some((self.history_range, latest, std::rc::Rc::new(pts)));
        }
        let points = self
            .history_view
            .as_ref()
            .map(|v| v.2.clone())
            .unwrap_or_default();
        if points.len() < 2 {
            widgets::empty_state(
                ui,
                &pal,
                "📅",
                "Collecting history…",
                "A point is recorded every 5 seconds. Check back in a minute.",
            );
            return;
        }
        let t1 = longterm::now();
        let t0 = t1 - self.history_range;
        let has_gpu = points.iter().any(|p| p.gpu > 0.0);

        let mut goto: Option<String> = None;
        egui::ScrollArea::vertical()
            .auto_shrink([false, false])
            .show(ui, |ui| {
                let rows: Vec<(
                    &str,
                    egui::Color32,
                    Option<f32>,
                    fn(&Point) -> f32,
                    fn(&Point) -> String,
                    bool,
                )> = vec![
                    (
                        "CPU",
                        pal.cpu,
                        Some(100.0),
                        |p| p.cpu,
                        |p| top_line("Top process", &p.top_cpu),
                        true,
                    ),
                    (
                        "Memory",
                        pal.mem,
                        Some(100.0),
                        |p| p.mem,
                        |p| top_line("Largest process", &p.top_mem),
                        true,
                    ),
                    (
                        "GPU",
                        pal.gpu,
                        Some(100.0),
                        |p| p.gpu,
                        |p| top_line("Top GPU user", &p.top_gpu),
                        has_gpu,
                    ),
                    (
                        "Network",
                        pal.net_rx,
                        None,
                        |p| p.net,
                        |_| String::new(),
                        true,
                    ),
                    ("Disk", pal.disk, None, |p| p.disk, |_| String::new(), true),
                ];
                for (title, color, max, metric, detail, show) in rows {
                    if !show {
                        continue;
                    }
                    let series: Vec<(i64, f32)> = points.iter().map(|p| (p.t, metric(p))).collect();
                    let avg = series.iter().map(|x| x.1).sum::<f32>() / series.len() as f32;
                    let peak = series.iter().map(|x| x.1).fold(0.0, f32::max);
                    let pct = max.is_some();
                    let fmt: &dyn Fn(f32) -> String = if pct {
                        &|v| format!("{v:.0}%")
                    } else {
                        &|v| fmt_rate(v)
                    };
                    widgets::card(ui, &pal, |ui| {
                        ui.horizontal(|ui| {
                            widgets::section_title(ui, &pal, title);
                            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                                ui.label(
                                    RichText::new(format!("peak {}", fmt(peak)))
                                        .strong()
                                        .color(color),
                                );
                                ui.label(
                                    RichText::new(format!("avg {}", fmt(avg))).color(pal.text_dim),
                                );
                            });
                        });
                        let pts_ref = &points;
                        widgets::timeline(
                            ui,
                            &pal,
                            &series,
                            t0,
                            t1,
                            color,
                            max,
                            110.0,
                            fmt,
                            &|i| detail(&pts_ref[i]),
                        );
                    });
                    ui.add_space(8.0);
                }

                // ── Biggest spikes ──
                let refs: Vec<&Point> = points.iter().collect();
                let cols = if has_gpu { 3.0 } else { 2.0 };
                let w = ((ui.available_width() - 10.0 * (cols - 1.0)) / cols).max(220.0);
                ui.horizontal_top(|ui| {
                    ui.spacing_mut().item_spacing.x = 10.0;
                    let lists: Vec<(
                        &str,
                        fn(&Point) -> f32,
                        fn(&Point) -> &str,
                        egui::Color32,
                        bool,
                    )> = vec![
                        (
                            "Biggest CPU spikes",
                            |p| p.cpu,
                            |p| &*p.top_cpu,
                            pal.cpu,
                            true,
                        ),
                        ("Memory peaks", |p| p.mem, |p| &*p.top_mem, pal.mem, true),
                        (
                            "Biggest GPU spikes",
                            |p| p.gpu,
                            |p| &*p.top_gpu,
                            pal.gpu,
                            has_gpu,
                        ),
                    ];
                    for (title, metric, who, color, show) in lists {
                        if !show {
                            continue;
                        }
                        ui.allocate_ui(vec2(w, 300.0), |ui| {
                            widgets::card(ui, &pal, |ui| {
                                ui.set_width(w - 28.0);
                                widgets::section_title(ui, &pal, title);
                                ui.add_space(4.0);
                                for p in longterm::peaks(&refs, metric, 6, 120) {
                                    ui.horizontal(|ui| {
                                        ui.label(
                                            RichText::new(format!("{:.0}%", metric(p)))
                                                .strong()
                                                .color(color),
                                        );
                                        ui.label(
                                            RichText::new(short_time(p.t)).color(pal.text_dim),
                                        );
                                        let name = who(p);
                                        if !name.is_empty()
                                            && ui
                                                .link(name)
                                                .on_hover_text("Find it in Processes")
                                                .clicked()
                                        {
                                            goto = Some(name.to_string());
                                        }
                                    });
                                }
                            });
                        });
                    }
                });
                ui.add_space(6.0);
                widgets::faint(
                    ui,
                    &pal,
                    format!(
                        "{} points from {} to now · stored at {}",
                        points.len(),
                        fmt_epoch(points[0].t as u64),
                        eframe::storage_dir("TaskForge")
                            .map(|d| d.join("history.json").display().to_string())
                            .unwrap_or_default()
                    ),
                );
                let _ = fmt_bytes;
            });

        if let Some(name) = goto {
            self.page = crate::app::Page::Processes;
            self.search = name;
        }
    }
}

fn top_line(label: &str, name: &str) -> String {
    if name.is_empty() {
        String::new()
    } else {
        format!("{label}: {name}")
    }
}

fn short_time(t: i64) -> String {
    use chrono::TimeZone;
    chrono::Local
        .timestamp_opt(t, 0)
        .single()
        .map(|d| d.format("%H:%M:%S").to_string())
        .unwrap_or_default()
}
