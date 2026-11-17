//! Detailed GPU view for the Performance page.

use eframe::egui::{self, vec2, Align, Layout, RichText};

use crate::app::App;
use crate::monitor::{GpuHistory, Snapshot};
use crate::native::GpuAdapter;
use crate::ui::theme::Palette;
use crate::ui::widgets::{self, fmt_bytes, GraphOpts, Series};

impl App {
    pub fn gpu_detail(
        &mut self,
        ui: &mut egui::Ui,
        s: &Snapshot,
        h: Option<&GpuHistory>,
        idx: usize,
        window: usize,
        interval: u64,
    ) {
        let pal = self.pal;
        let Some(g) = s.gpus.get(idx) else {
            widgets::empty_state(
                ui,
                &pal,
                "🎮",
                "GPU not found",
                "It may have been disconnected.",
            );
            return;
        };
        let empty = std::collections::VecDeque::new();
        let hist =
            |f: fn(&GpuHistory) -> &std::collections::VecDeque<f32>| h.map(f).unwrap_or(&empty);

        // ── Header + overall utilisation ──
        widgets::card(ui, &pal, |ui| {
            ui.horizontal(|ui| {
                ui.label(RichText::new(format!("GPU {idx}")).size(18.0).strong());
                ui.label(RichText::new(&*g.name).color(pal.text_dim));
                widgets::pill(
                    ui,
                    if g.integrated {
                        "Integrated"
                    } else {
                        "Dedicated"
                    },
                    pal.gpu,
                );
                ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                    if ui
                        .selectable_label(self.gpu_engines_view, "Engines")
                        .clicked()
                    {
                        self.gpu_engines_view = true;
                    }
                    if ui
                        .selectable_label(!self.gpu_engines_view, "Overall")
                        .clicked()
                    {
                        self.gpu_engines_view = false;
                    }
                });
            });
            ui.add_space(6.0);
            if self.gpu_engines_view && !g.engines.is_empty() {
                let cols = 3usize.min(g.engines.len()).max(1);
                let cw = (ui.available_width() - 10.0 * (cols - 1) as f32) / cols as f32;
                egui::Grid::new("gpu_engines")
                    .spacing(vec2(10.0, 10.0))
                    .show(ui, |ui| {
                        for (i, e) in g.engines.iter().enumerate() {
                            ui.allocate_ui(vec2(cw, 110.0), |ui| {
                                ui.vertical(|ui| {
                                    ui.set_width(cw);
                                    ui.horizontal(|ui| {
                                        ui.label(RichText::new(&*e.name).size(12.0).strong());
                                        ui.with_layout(
                                            Layout::right_to_left(Align::Center),
                                            |ui| {
                                                ui.label(
                                                    RichText::new(format!("{:.0}%", e.util))
                                                        .size(12.0)
                                                        .color(pal.gpu),
                                                );
                                            },
                                        );
                                    });
                                    let data =
                                        h.and_then(|h| h.engines.get(&*e.name)).unwrap_or(&empty);
                                    widgets::graph(
                                        ui,
                                        &pal,
                                        &[Series {
                                            data,
                                            color: pal.gpu,
                                            label: &e.name,
                                        }],
                                        GraphOpts {
                                            height: 84.0,
                                            max: Some(100.0),
                                            window,
                                            grid: false,
                                            fmt: &|v| format!("{v:.0}%"),
                                            interval_ms: interval,
                                        },
                                    );
                                });
                            });
                            if (i + 1) % cols == 0 {
                                ui.end_row();
                            }
                        }
                    });
            } else {
                widgets::graph(
                    ui,
                    &pal,
                    &[Series {
                        data: hist(|h| &h.util),
                        color: pal.gpu,
                        label: "GPU",
                    }],
                    GraphOpts {
                        height: 220.0,
                        max: Some(100.0),
                        window,
                        grid: true,
                        fmt: &|v| format!("{v:.0}%"),
                        interval_ms: interval,
                    },
                );
                // Engine bars under the graph.
                ui.add_space(6.0);
                ui.horizontal_wrapped(|ui| {
                    for e in &g.engines {
                        ui.allocate_ui(vec2(170.0, 30.0), |ui| {
                            ui.vertical(|ui| {
                                ui.spacing_mut().item_spacing.y = 2.0;
                                ui.horizontal(|ui| {
                                    ui.label(
                                        RichText::new(&*e.name).size(11.0).color(pal.text_dim),
                                    );
                                    ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                                        ui.label(
                                            RichText::new(format!("{:.0}%", e.util)).size(11.0),
                                        );
                                    });
                                });
                                widgets::bar(ui, &pal, e.util / 100.0, pal.gpu, 5.0);
                            });
                        });
                    }
                });
            }
        });
        ui.add_space(10.0);

        // ── Memory ──
        let half = ((ui.available_width() - 20.0) / 2.0).max(200.0);
        ui.horizontal_top(|ui| {
            ui.spacing_mut().item_spacing.x = 10.0;
            mem_card(
                ui,
                &pal,
                half,
                "Dedicated GPU memory",
                g.ded_used,
                g.ded_total,
                hist(|h| &h.ded),
                window,
                interval,
                pal.mem,
            );
            mem_card(
                ui,
                &pal,
                half,
                "Shared GPU memory",
                g.shared_used,
                g.shared_total,
                hist(|h| &h.shared),
                window,
                interval,
                pal.disk,
            );
        });
        ui.add_space(10.0);

        // ── Sensors ──
        let has_sensors = g.temp_c.is_some()
            || g.power_pct.is_some()
            || g.core_mhz.is_some()
            || g.fan_rpm.is_some();
        if has_sensors {
            let quarter = ((ui.available_width() - 44.0) / 4.0).max(150.0);
            ui.horizontal_top(|ui| {
                ui.spacing_mut().item_spacing.x = 10.0;
                if let Some(t) = g.temp_c {
                    let max = g.temp_max_c.unwrap_or(95.0);
                    sensor_card(
                        ui,
                        &pal,
                        quarter,
                        "🌡 Temperature",
                        format!("{t:.0} °C"),
                        format!("Max {max:.0} °C"),
                        hist(|h| &h.temp),
                        Some(max.max(t)),
                        pal.load(t / max),
                        window,
                        interval,
                        &|v| format!("{v:.0} °C"),
                    );
                }
                if let Some(p) = g.power_pct {
                    sensor_card(
                        ui,
                        &pal,
                        quarter,
                        "⚡ Power",
                        format!("{p:.0}%"),
                        "of board power limit".into(),
                        hist(|h| &h.power),
                        Some(100.0),
                        pal.warn,
                        window,
                        interval,
                        &|v| format!("{v:.0}%"),
                    );
                }
                if let Some(c) = g.core_mhz {
                    let sub = g
                        .core_max_mhz
                        .map(|m| format!("Max {m:.0} MHz"))
                        .unwrap_or_default();
                    sensor_card(
                        ui,
                        &pal,
                        quarter,
                        "⏱ Core clock",
                        format!("{c:.0} MHz"),
                        sub,
                        hist(|h| &h.core_mhz),
                        g.core_max_mhz,
                        pal.cpu,
                        window,
                        interval,
                        &|v| format!("{v:.0} MHz"),
                    );
                }
                if let Some(f) = g.fan_rpm {
                    let sub = g
                        .fan_max_rpm
                        .map(|m| format!("Max {m} RPM"))
                        .unwrap_or_default();
                    sensor_card(
                        ui,
                        &pal,
                        quarter,
                        "🌀 Fan",
                        format!("{f} RPM"),
                        sub,
                        hist(|h| &h.fan),
                        g.fan_max_rpm.map(|m| m as f32),
                        pal.net_rx,
                        window,
                        interval,
                        &|v| format!("{v:.0} RPM"),
                    );
                }
            });
            ui.add_space(10.0);
        }

        // ── Details ──
        widgets::card(ui, &pal, |ui| {
            ui.set_width(ui.available_width());
            stats(ui, &pal, g);
            if !has_sensors {
                ui.add_space(6.0);
                widgets::faint(ui, &pal, "This GPU's driver doesn't report temperature, power, clocks or fan speed to Windows.");
            }
        });
        ui.add_space(10.0);

        // ── Top GPU processes ──
        widgets::card(ui, &pal, |ui| {
            ui.set_width(ui.available_width());
            widgets::section_title(ui, &pal, "Using the GPU");
            ui.add_space(4.0);
            let prefix = format!("GPU {idx} ");
            let mut v: Vec<_> = s
                .procs
                .iter()
                .filter(|p| p.gpu > 0.0 || p.gpu_mem > 0)
                .collect();
            v.sort_by(|a, b| b.gpu.total_cmp(&a.gpu).then(b.gpu_mem.cmp(&a.gpu_mem)));
            if v.is_empty() {
                widgets::faint(ui, &pal, "No process is using this GPU right now.");
            }
            let mut goto = None;
            egui::Grid::new("gpu_procs")
                .num_columns(5)
                .striped(true)
                .spacing(vec2(14.0, 4.0))
                .show(ui, |ui| {
                    for h in ["Process", "GPU", "Engine", "Dedicated", "Shared"] {
                        ui.label(RichText::new(h).size(11.0).color(pal.text_dim));
                    }
                    ui.end_row();
                    for p in v.into_iter().take(12) {
                        ui.horizontal(|ui| {
                            widgets::avatar(ui, &p.name, pal.dark);
                            if ui.link(&*p.name).clicked() {
                                goto = Some(p.pid);
                            }
                        });
                        ui.label(RichText::new(format!("{:.1}%", p.gpu)).strong());
                        let engine = p
                            .gpu_engine
                            .strip_prefix(&prefix)
                            .map(|e| e.trim_start_matches("- ").to_string())
                            .unwrap_or(p.gpu_engine.to_string());
                        ui.label(RichText::new(engine).color(pal.text_dim));
                        ui.label(fmt_bytes(p.gpu_mem));
                        ui.label(RichText::new(fmt_bytes(p.gpu_shared)).color(pal.text_dim));
                        ui.end_row();
                    }
                });
            if let Some(pid) = goto {
                self.reveal_process(pid);
            }
        });
    }
}

#[allow(clippy::too_many_arguments)]
fn mem_card(
    ui: &mut egui::Ui,
    pal: &Palette,
    w: f32,
    title: &str,
    used: u64,
    total: u64,
    data: &std::collections::VecDeque<f32>,
    window: usize,
    interval: u64,
    color: egui::Color32,
) {
    ui.allocate_ui(vec2(w, 200.0), |ui| {
        widgets::card(ui, pal, |ui| {
            ui.set_width(w - 28.0);
            ui.horizontal(|ui| {
                widgets::section_title(ui, pal, title);
                ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                    ui.label(
                        RichText::new(format!("{} / {}", fmt_bytes(used), fmt_bytes(total)))
                            .strong(),
                    );
                });
            });
            widgets::graph(
                ui,
                pal,
                &[Series {
                    data,
                    color,
                    label: title,
                }],
                GraphOpts {
                    height: 110.0,
                    max: Some(total.max(1) as f32),
                    window,
                    grid: true,
                    fmt: &|v| fmt_bytes(v as u64),
                    interval_ms: interval,
                },
            );
            widgets::bar(ui, pal, used as f32 / total.max(1) as f32, color, 6.0);
        });
    });
}

#[allow(clippy::too_many_arguments)]
fn sensor_card(
    ui: &mut egui::Ui,
    pal: &Palette,
    w: f32,
    title: &str,
    value: String,
    sub: String,
    data: &std::collections::VecDeque<f32>,
    max: Option<f32>,
    color: egui::Color32,
    window: usize,
    interval: u64,
    fmt: &dyn Fn(f32) -> String,
) {
    ui.allocate_ui(vec2(w, 170.0), |ui| {
        widgets::card(ui, pal, |ui| {
            ui.set_width(w - 28.0);
            ui.label(RichText::new(title).size(12.0).color(pal.text_dim));
            ui.label(RichText::new(value).size(20.0).strong().color(color));
            widgets::faint(ui, pal, sub);
            widgets::graph(
                ui,
                pal,
                &[Series {
                    data,
                    color,
                    label: title,
                }],
                GraphOpts {
                    height: 56.0,
                    max,
                    window,
                    grid: false,
                    fmt,
                    interval_ms: interval,
                },
            );
        });
    });
}

fn stats(ui: &mut egui::Ui, pal: &Palette, g: &GpuAdapter) {
    let mut items: Vec<(&str, String)> = vec![
        ("Utilization", format!("{:.0}%", g.util)),
        ("Vendor", g.vendor.clone()),
        (
            "Driver version",
            if g.driver.is_empty() {
                "—".into()
            } else {
                g.driver.clone()
            },
        ),
        ("Dedicated memory", fmt_bytes(g.ded_total)),
        ("Shared memory", fmt_bytes(g.shared_total)),
    ];
    if g.budget > 0 {
        items.push(("Memory budget", fmt_bytes(g.budget)));
    }
    if let Some(m) = g.mem_mhz {
        items.push(("Memory clock", format!("{m:.0} MHz")));
    }
    if let Some(v) = g.voltage_mv {
        items.push(("Core voltage", format!("{v} mV")));
    }
    if let Some(b) = g.mem_bandwidth {
        items.push(("Memory bandwidth", format!("{}/s", fmt_bytes(b as u64))));
    }
    if let Some(b) = g.pcie_bandwidth {
        items.push(("PCIe bandwidth", format!("{}/s", fmt_bytes(b as u64))));
    }
    ui.horizontal_wrapped(|ui| {
        ui.spacing_mut().item_spacing = vec2(28.0, 10.0);
        for (k, v) in items {
            ui.vertical(|ui| {
                ui.spacing_mut().item_spacing.y = 1.0;
                ui.label(RichText::new(k).size(11.0).color(pal.text_dim));
                ui.label(RichText::new(v).size(16.0).strong());
            });
        }
    });
}
