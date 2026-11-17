//! Overview dashboard and Performance page.

use eframe::egui::{self, vec2, Align, Layout, RichText};

use crate::app::{App, Page, PerfSel};
use crate::monitor::{pct, EventKind};
use crate::platform;
use crate::ui::widgets::{
    self, fmt_bits, fmt_bytes, fmt_dur, fmt_epoch, fmt_rate, GraphOpts, Series,
};

impl App {
    pub fn page_overview(&mut self, ui: &mut egui::Ui) {
        let pal = self.pal;
        let s = self.snap.clone();
        let shared = self.mon.shared.clone();
        let mut goto: Option<u32> = None;
        let mut kill: Option<(u32, String)> = None;
        let mut nav: Option<Page> = None;

        egui::ScrollArea::vertical()
            .auto_shrink([false, false])
            .show(ui, |ui| {
                let g = shared.lock().unwrap();
                let h = &g.history;
                let sys_disk = s
                    .disks
                    .iter()
                    .find(|d| d.mount.eq_ignore_ascii_case("C:\\") || d.mount == "/")
                    .or(s.disks.first())
                    .cloned();

                // ── Gauge cards ──
                let gap = 12.0;
                let avail = ui.available_width() - 14.0;
                // The "main" GPU is the one with the most dedicated memory.
                let main_gpu = (0..s.gpus.len()).max_by_key(|&i| s.gpus[i].ded_total);
                let n_gauges = if main_gpu.is_some() { 5.0 } else { 4.0 };
                let w = ((avail - gap * (n_gauges - 1.0)) / n_gauges).max(150.0);
                ui.horizontal(|ui| {
                    ui.spacing_mut().item_spacing.x = gap;
                    let mut gauge = |ui: &mut egui::Ui,
                                     title: &str,
                                     frac: f32,
                                     big: String,
                                     sub: String,
                                     foot: String,
                                     col,
                                     data: &std::collections::VecDeque<f32>,
                                     max: Option<f32>,
                                     page: Page| {
                        ui.allocate_ui(vec2(w, 250.0), |ui| {
                            widgets::card(ui, &pal, |ui| {
                                ui.set_width(w - 28.0);
                                ui.horizontal(|ui| {
                                    widgets::section_title(ui, &pal, title);
                                    ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                                        if ui
                                            .small_button("›")
                                            .on_hover_text("Open details")
                                            .clicked()
                                        {
                                            nav = Some(page);
                                        }
                                    });
                                });
                                ui.vertical_centered(|ui| {
                                    widgets::ring(ui, &pal, frac, col, &big, &sub, 116.0);
                                });
                                widgets::faint(ui, &pal, foot);
                                widgets::sparkline(
                                    ui,
                                    &pal,
                                    data,
                                    col,
                                    max,
                                    vec2(ui.available_width(), 36.0),
                                );
                            });
                        });
                    };
                    gauge(
                        ui,
                        "CPU",
                        s.cpu_total / 100.0,
                        format!("{:.0}%", s.cpu_total),
                        format!("{} cores", s.logical_cores),
                        format!("{} MHz", s.cpu_freq_mhz),
                        pal.cpu,
                        &h.cpu,
                        Some(100.0),
                        Page::Performance,
                    );
                    gauge(
                        ui,
                        "Memory",
                        s.mem_pct() / 100.0,
                        format!("{:.0}%", s.mem_pct()),
                        fmt_bytes(s.mem_used),
                        format!(
                            "{} available of {}",
                            fmt_bytes(s.mem_available),
                            fmt_bytes(s.mem_total)
                        ),
                        pal.mem,
                        &h.mem,
                        Some(100.0),
                        Page::Performance,
                    );
                    if let Some(gi) = main_gpu {
                        let gp = &s.gpus[gi];
                        let empty = std::collections::VecDeque::new();
                        let data = h.gpus.get(gi).map(|x| &x.util).unwrap_or(&empty);
                        let foot = match gp.temp_c {
                            Some(t) => format!(
                                "{} / {} · {t:.0} °C",
                                fmt_bytes(gp.ded_used),
                                fmt_bytes(gp.ded_total)
                            ),
                            None => format!(
                                "{} / {} VRAM",
                                fmt_bytes(gp.ded_used),
                                fmt_bytes(gp.ded_total)
                            ),
                        };
                        let short: String = gp
                            .name
                            .split_whitespace()
                            .filter(|w| {
                                !matches!(
                                    *w,
                                    "NVIDIA" | "AMD" | "Intel(R)" | "GeForce" | "Radeon(TM)"
                                )
                            })
                            .take(3)
                            .collect::<Vec<_>>()
                            .join(" ");
                        gauge(
                            ui,
                            "GPU",
                            gp.util / 100.0,
                            format!("{:.0}%", gp.util),
                            short,
                            foot,
                            pal.gpu,
                            data,
                            Some(100.0),
                            Page::Performance,
                        );
                    }
                    let (dfrac, dlabel, dfoot) = match &sys_disk {
                        Some(d) => (
                            pct(d.total - d.available, d.total) / 100.0,
                            format!("{:.0}%", pct(d.total - d.available, d.total)),
                            format!("{} free on {}", fmt_bytes(d.available), d.mount),
                        ),
                        None => (0.0, "—".into(), String::new()),
                    };
                    gauge(
                        ui,
                        "System drive",
                        dfrac,
                        dlabel,
                        "used".into(),
                        format!(
                            "{dfoot} · I/O {}",
                            fmt_rate((s.disk_read_rate + s.disk_write_rate) as f32)
                        ),
                        pal.disk,
                        &h.disk_r,
                        None,
                        Page::Performance,
                    );
                    let net_total = (s.net_rx_rate + s.net_tx_rate) as f32;
                    gauge(
                        ui,
                        "Network",
                        (net_total / (12.5 * 1024.0 * 1024.0)).min(1.0),
                        fmt_bits(net_total),
                        "throughput".into(),
                        format!(
                            "⬇ {}   ⬆ {}",
                            fmt_rate(s.net_rx_rate as f32),
                            fmt_rate(s.net_tx_rate as f32)
                        ),
                        pal.net_rx,
                        &h.net_rx,
                        None,
                        Page::Network,
                    );
                });
                drop(g);
                ui.add_space(12.0);

                // ── Top consumers ──
                let half = ((avail - gap) / 2.0).max(260.0);
                let has_gpu = !s.gpus.is_empty();
                let third = if has_gpu {
                    ((avail - gap * 2.0) / 3.0).max(220.0)
                } else {
                    half
                };
                ui.horizontal_top(|ui| {
                    ui.spacing_mut().item_spacing.x = gap;
                    // metric: 0 = CPU, 1 = memory, 2 = GPU
                    let mut top_list = |ui: &mut egui::Ui, title: &str, metric: u8| {
                        let by_cpu = metric == 0;
                        ui.allocate_ui(vec2(third, 300.0), |ui| {
                            widgets::card(ui, &pal, |ui| {
                                ui.set_width(third - 28.0);
                                widgets::section_title(ui, &pal, title);
                                ui.add_space(4.0);
                                let mut v: Vec<_> = s.procs.iter().collect();
                                let val_of = |p: &crate::monitor::ProcInfo| match metric {
                                    0 => p.cpu,
                                    1 => p.mem as f32,
                                    _ => p.gpu,
                                };
                                v.sort_by(|a, b| val_of(b).total_cmp(&val_of(a)));
                                let _ = by_cpu;
                                let maxv = v.first().map(|p| val_of(p)).unwrap_or(1.0).max(0.01);
                                for p in v.into_iter().take(6) {
                                    let (val, txt, col) = match metric {
                                        0 => (p.cpu, format!("{:.1}%", p.cpu), pal.cpu),
                                        1 => (p.mem as f32, fmt_bytes(p.mem), pal.mem),
                                        _ => (p.gpu, format!("{:.1}%", p.gpu), pal.gpu),
                                    };
                                    ui.horizontal(|ui| {
                                        widgets::avatar(ui, &p.name, pal.dark);
                                        if ui
                                            .add(
                                                egui::Label::new(&*p.name)
                                                    .truncate()
                                                    .sense(egui::Sense::click()),
                                            )
                                            .on_hover_text("Show in Processes")
                                            .clicked()
                                        {
                                            goto = Some(p.pid);
                                        }
                                        ui.with_layout(
                                            Layout::right_to_left(Align::Center),
                                            |ui| {
                                                if ui
                                                    .small_button("✖")
                                                    .on_hover_text("End task")
                                                    .clicked()
                                                {
                                                    kill = Some((p.pid, p.name.to_string()));
                                                }
                                                ui.label(RichText::new(txt).strong());
                                            },
                                        );
                                    });
                                    widgets::bar(ui, &pal, val / maxv, col, 4.0);
                                    ui.add_space(2.0);
                                }
                            });
                        });
                    };
                    top_list(ui, "Top CPU", 0);
                    top_list(ui, "Top memory", 1);
                    if has_gpu {
                        top_list(ui, "Top GPU", 2);
                    }
                });
                ui.add_space(12.0);

                // ── System info + recent activity ──
                ui.horizontal_top(|ui| {
                    ui.spacing_mut().item_spacing.x = gap;
                    ui.allocate_ui(vec2(half, 280.0), |ui| {
                        widgets::card(ui, &pal, |ui| {
                            ui.set_width(half - 28.0);
                            widgets::section_title(ui, &pal, "System");
                            ui.add_space(4.0);
                            egui::Grid::new("sysinfo")
                                .num_columns(2)
                                .spacing(vec2(14.0, 6.0))
                                .show(ui, |ui| {
                                    let mut kv = |k: &str, v: String| {
                                        ui.label(RichText::new(k).color(pal.text_dim));
                                        ui.add(egui::Label::new(v).truncate());
                                        ui.end_row();
                                    };
                                    kv("Computer", s.host_name.clone());
                                    kv("OS", s.os_name.clone());
                                    kv("Kernel", s.kernel.clone());
                                    kv("Processor", s.cpu_brand.clone());
                                    kv(
                                        "Cores",
                                        format!(
                                            "{} logical{}",
                                            s.logical_cores,
                                            s.physical_cores
                                                .map(|p| format!(", {p} physical"))
                                                .unwrap_or_default()
                                        ),
                                    );
                                    kv("Memory", fmt_bytes(s.mem_total));
                                    kv("Up time", fmt_dur(s.uptime));
                                    kv("Booted", fmt_epoch(s.boot_time));
                                    kv("Processes", s.procs.len().to_string());
                                    kv(
                                        "Privileges",
                                        if platform::is_admin() {
                                            "Administrator".into()
                                        } else {
                                            "Standard user".into()
                                        },
                                    );
                                });
                        });
                    });
                    ui.allocate_ui(vec2(half, 280.0), |ui| {
                        widgets::card(ui, &pal, |ui| {
                            ui.set_width(half - 28.0);
                            ui.horizontal(|ui| {
                                widgets::section_title(ui, &pal, "Recent activity");
                                ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                                    if ui.small_button("View all ›").clicked() {
                                        nav = Some(Page::Activity);
                                    }
                                });
                            });
                            ui.add_space(4.0);
                            let g = shared.lock().unwrap();
                            if g.log.is_empty() {
                                widgets::faint(
                                    ui,
                                    &pal,
                                    "Process launches and exits will appear here.",
                                );
                            }
                            for e in g.log.iter().rev().take(9) {
                                ui.horizontal(|ui| {
                                    let (icon, col) = event_style(e.kind, &pal);
                                    ui.label(RichText::new(icon).color(col));
                                    ui.label(
                                        RichText::new(e.time.format("%H:%M:%S").to_string())
                                            .color(pal.text_faint)
                                            .monospace(),
                                    );
                                    ui.add(
                                        egui::Label::new(format!(
                                            "{} {}",
                                            e.name,
                                            if e.pid > 0 {
                                                format!("({})", e.pid)
                                            } else {
                                                String::new()
                                            }
                                        ))
                                        .truncate(),
                                    );
                                });
                            }
                        });
                    });
                });
            });

        if let Some(pid) = goto {
            self.reveal_process(pid);
        }
        if let Some((pid, name)) = kill {
            self.request_kill(pid, &name, false);
        }
        if let Some(p) = nav {
            self.page = p;
        }
    }

    pub fn page_performance(&mut self, ui: &mut egui::Ui) {
        let pal = self.pal;
        let s = self.snap.clone();
        let shared = self.mon.shared.clone();
        let window = self.graph_window();
        let interval = self.settings.refresh_ms;

        // Window length selector.
        ui.horizontal(|ui| {
            ui.label(RichText::new("History").color(pal.text_dim));
            for (secs, label) in [(60, "1 min"), (300, "5 min"), (900, "15 min")] {
                if ui
                    .selectable_label(self.settings.graph_secs == secs, label)
                    .clicked()
                {
                    self.settings.graph_secs = secs;
                }
            }
            widgets::faint(ui, &pal, "· hover any graph to read past values");
        });
        ui.add_space(6.0);

        let g = shared.lock().unwrap();
        let h = &g.history;

        // ── Left resource list ──
        egui::SidePanel::left("perf_list")
            .exact_width(240.0)
            .resizable(false)
            .frame(egui::Frame::new().inner_margin(egui::Margin {
                left: 0,
                right: 12,
                top: 0,
                bottom: 0,
            }))
            .show_inside(ui, |ui| {
                egui::ScrollArea::vertical().show(ui, |ui| {
                    let mut item = |ui: &mut egui::Ui,
                                    sel: PerfSel,
                                    title: String,
                                    sub: String,
                                    data: &std::collections::VecDeque<f32>,
                                    col,
                                    max: Option<f32>| {
                        let active = self.perf_sel == sel;
                        let fr = egui::Frame::new()
                            .fill(if active {
                                widgets::with_alpha(pal.accent, 0.14)
                            } else {
                                pal.card
                            })
                            .stroke(egui::Stroke::new(
                                1.0f32,
                                if active {
                                    widgets::with_alpha(pal.accent, 0.7)
                                } else {
                                    pal.border
                                },
                            ))
                            .corner_radius(10.0)
                            .inner_margin(egui::Margin::same(10))
                            .show(ui, |ui| {
                                ui.set_width(ui.available_width());
                                ui.horizontal(|ui| {
                                    ui.vertical(|ui| {
                                        ui.spacing_mut().item_spacing.y = 1.0;
                                        ui.label(RichText::new(title).strong());
                                        ui.label(RichText::new(sub).size(11.0).color(pal.text_dim));
                                    });
                                    ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                                        widgets::sparkline(
                                            ui,
                                            &pal,
                                            data,
                                            col,
                                            max,
                                            vec2(84.0, 34.0),
                                        );
                                    });
                                });
                            })
                            .response
                            .interact(egui::Sense::click());
                        if fr.clicked() {
                            self.perf_sel = sel;
                        }
                        ui.add_space(6.0);
                    };
                    item(
                        ui,
                        PerfSel::Cpu,
                        "CPU".into(),
                        format!(
                            "{:.0}%  {:.2} GHz",
                            s.cpu_total,
                            s.cpu_freq_mhz as f32 / 1000.0
                        ),
                        &h.cpu,
                        pal.cpu,
                        Some(100.0),
                    );
                    item(
                        ui,
                        PerfSel::Memory,
                        "Memory".into(),
                        format!(
                            "{} / {} ({:.0}%)",
                            fmt_bytes(s.mem_used),
                            fmt_bytes(s.mem_total),
                            s.mem_pct()
                        ),
                        &h.mem,
                        pal.mem,
                        Some(100.0),
                    );
                    item(
                        ui,
                        PerfSel::Disk,
                        "Disk".into(),
                        format!(
                            "R {}  W {}",
                            fmt_rate(s.disk_read_rate as f32),
                            fmt_rate(s.disk_write_rate as f32)
                        ),
                        &h.disk_r,
                        pal.disk,
                        None,
                    );
                    for (gi, gp) in s.gpus.iter().enumerate() {
                        let empty = std::collections::VecDeque::new();
                        let data = h.gpus.get(gi).map(|x| &x.util).unwrap_or(&empty);
                        let sub = match gp.temp_c {
                            Some(t) => format!("{:.0}%  ·  {t:.0} °C", gp.util),
                            None => format!("{:.0}%", gp.util),
                        };
                        let short: String = gp.name.chars().take(22).collect();
                        item(
                            ui,
                            PerfSel::Gpu(gi),
                            format!("GPU {gi}  {short}"),
                            sub,
                            data,
                            pal.gpu,
                            Some(100.0),
                        );
                    }
                    for n in s.networks.iter().filter(|n| n.total_rx + n.total_tx > 0) {
                        let empty = std::collections::VecDeque::new();
                        let data = h.per_if.get(&*n.name).map(|x| &x.0).unwrap_or(&empty);
                        item(
                            ui,
                            PerfSel::Net(n.name.clone()),
                            n.name.clone(),
                            format!(
                                "⬇ {}  ⬆ {}",
                                fmt_bits(n.rx_rate as f32),
                                fmt_bits(n.tx_rate as f32)
                            ),
                            data,
                            pal.net_rx,
                            None,
                        );
                    }
                });
            });

        // ── Detail view ──
        egui::ScrollArea::vertical()
            .auto_shrink([false, false])
            .show(ui, |ui| {
                let opts = |height: f32, max: Option<f32>, fmt: &'static dyn Fn(f32) -> String| {
                    GraphOpts {
                        height,
                        max,
                        window,
                        grid: true,
                        fmt,
                        interval_ms: interval,
                    }
                };
                match self.perf_sel.clone() {
                    PerfSel::Cpu => {
                        widgets::card(ui, &pal, |ui| {
                            ui.horizontal(|ui| {
                                ui.label(RichText::new("CPU").size(18.0).strong());
                                ui.label(RichText::new(&s.cpu_brand).color(pal.text_dim));
                                ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                                    if ui
                                        .selectable_label(self.perf_cores, "Logical processors")
                                        .clicked()
                                    {
                                        self.perf_cores = true;
                                    }
                                    if ui.selectable_label(!self.perf_cores, "Overall").clicked() {
                                        self.perf_cores = false;
                                    }
                                });
                            });
                            ui.add_space(6.0);
                            if self.perf_cores && !h.per_core.is_empty() {
                                let n = h.per_core.len();
                                let cols = (n as f32).sqrt().ceil().max(2.0) as usize;
                                let cw =
                                    (ui.available_width() - 8.0 * (cols - 1) as f32) / cols as f32;
                                egui::Grid::new("cores")
                                    .spacing(vec2(8.0, 8.0))
                                    .show(ui, |ui| {
                                        for (i, q) in h.per_core.iter().enumerate() {
                                            ui.allocate_ui(vec2(cw, 80.0), |ui| {
                                                ui.vertical(|ui| {
                                                    ui.spacing_mut().item_spacing.y = 2.0;
                                                    ui.label(
                                                        RichText::new(format!(
                                                            "CPU {i}  ·  {:.0}%",
                                                            s.per_core
                                                                .get(i)
                                                                .copied()
                                                                .unwrap_or(0.0)
                                                        ))
                                                        .size(10.5)
                                                        .color(pal.text_dim),
                                                    );
                                                    ui.set_width(cw);
                                                    widgets::graph(
                                                        ui,
                                                        &pal,
                                                        &[Series {
                                                            data: q,
                                                            color: pal.cpu,
                                                            label: "",
                                                        }],
                                                        GraphOpts {
                                                            height: 60.0,
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
                                        data: &h.cpu,
                                        color: pal.cpu,
                                        label: "CPU",
                                    }],
                                    opts(260.0, Some(100.0), &|v| format!("{v:.0}%")),
                                );
                            }
                        });
                        ui.add_space(10.0);
                        widgets::card(ui, &pal, |ui| {
                            ui.set_width(ui.available_width());
                            stat_row(
                                ui,
                                &pal,
                                &[
                                    ("Utilization", format!("{:.0}%", s.cpu_total)),
                                    (
                                        "Speed",
                                        format!("{:.2} GHz", s.cpu_freq_mhz as f32 / 1000.0),
                                    ),
                                    ("Processes", s.procs.len().to_string()),
                                    ("Up time", fmt_dur(s.uptime)),
                                    ("Logical processors", s.logical_cores.to_string()),
                                    (
                                        "Cores",
                                        s.physical_cores
                                            .map(|c| c.to_string())
                                            .unwrap_or("—".into()),
                                    ),
                                    ("Architecture", s.arch.clone()),
                                ],
                            );
                        });
                        ui.add_space(10.0);
                        top_card(ui, &pal, &s, true);
                        if !s.temps.is_empty() {
                            ui.add_space(10.0);
                            widgets::card(ui, &pal, |ui| {
                                ui.set_width(ui.available_width());
                                widgets::section_title(ui, &pal, "🌡 Temperatures");
                                for (label, t, crit) in &s.temps {
                                    ui.horizontal(|ui| {
                                        ui.add_sized(
                                            vec2(220.0, 18.0),
                                            egui::Label::new(
                                                RichText::new(label).color(pal.text_dim),
                                            )
                                            .truncate(),
                                        );
                                        let frac = t / crit.unwrap_or(100.0).max(1.0);
                                        ui.label(
                                            RichText::new(format!("{t:.0} °C"))
                                                .strong()
                                                .color(pal.load(frac)),
                                        );
                                        widgets::bar(ui, &pal, frac, pal.load(frac), 5.0);
                                    });
                                }
                            });
                        }
                    }
                    PerfSel::Memory => {
                        widgets::card(ui, &pal, |ui| {
                            ui.horizontal(|ui| {
                                ui.label(RichText::new("Memory").size(18.0).strong());
                                ui.label(RichText::new(fmt_bytes(s.mem_total)).color(pal.text_dim));
                            });
                            ui.add_space(6.0);
                            widgets::graph(
                                ui,
                                &pal,
                                &[
                                    Series {
                                        data: &h.mem,
                                        color: pal.mem,
                                        label: "Memory",
                                    },
                                    Series {
                                        data: &h.swap,
                                        color: pal.net_tx,
                                        label: "Page file",
                                    },
                                ],
                                opts(240.0, Some(100.0), &|v| format!("{v:.0}%")),
                            );
                            ui.horizontal(|ui| {
                                widgets::legend(ui, pal.mem, "Memory in use");
                                widgets::legend(ui, pal.net_tx, "Page file / swap");
                            });
                            ui.add_space(8.0);
                            widgets::section_title(ui, &pal, "Composition");
                            composition(
                                ui,
                                &pal,
                                &[
                                    (s.mem_used, pal.mem, "In use"),
                                    (
                                        s.mem_total.saturating_sub(s.mem_used),
                                        pal.text_faint,
                                        "Available",
                                    ),
                                ],
                                s.mem_total,
                            );
                        });
                        ui.add_space(10.0);
                        widgets::card(ui, &pal, |ui| {
                            ui.set_width(ui.available_width());
                            stat_row(
                                ui,
                                &pal,
                                &[
                                    ("In use", fmt_bytes(s.mem_used)),
                                    ("Available", fmt_bytes(s.mem_available)),
                                    ("Total", fmt_bytes(s.mem_total)),
                                    ("Page file used", fmt_bytes(s.swap_used)),
                                    ("Page file size", fmt_bytes(s.swap_total)),
                                    ("Load", format!("{:.0}%", s.mem_pct())),
                                ],
                            );
                        });
                        ui.add_space(10.0);
                        top_card(ui, &pal, &s, false);
                    }
                    PerfSel::Disk => {
                        widgets::card(ui, &pal, |ui| {
                            ui.label(RichText::new("Disk activity").size(18.0).strong());
                            ui.add_space(6.0);
                            widgets::graph(
                                ui,
                                &pal,
                                &[
                                    Series {
                                        data: &h.disk_r,
                                        color: pal.disk,
                                        label: "Read",
                                    },
                                    Series {
                                        data: &h.disk_w,
                                        color: pal.net_rx,
                                        label: "Write",
                                    },
                                ],
                                opts(220.0, None, &|v| fmt_rate(v)),
                            );
                            ui.horizontal(|ui| {
                                widgets::legend(
                                    ui,
                                    pal.disk,
                                    &format!("Read {}", fmt_rate(s.disk_read_rate as f32)),
                                );
                                widgets::legend(
                                    ui,
                                    pal.net_rx,
                                    &format!("Write {}", fmt_rate(s.disk_write_rate as f32)),
                                );
                            });
                        });
                        ui.add_space(10.0);
                        widgets::card(ui, &pal, |ui| {
                            ui.set_width(ui.available_width());
                            widgets::section_title(ui, &pal, "Drives");
                            ui.add_space(4.0);
                            for d in &s.disks {
                                let used = d.total.saturating_sub(d.available);
                                let frac = pct(used, d.total) / 100.0;
                                ui.horizontal(|ui| {
                                    ui.label(RichText::new(format!("💾 {}", d.mount)).strong());
                                    ui.label(
                                        RichText::new(format!(
                                            "{} · {} · {}",
                                            if d.name.is_empty() {
                                                "Local disk"
                                            } else {
                                                &d.name
                                            },
                                            d.fs,
                                            d.kind
                                        ))
                                        .color(pal.text_dim),
                                    );
                                    if d.removable {
                                        widgets::pill(ui, "Removable", pal.accent);
                                    }
                                    ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                                        ui.label(format!(
                                            "{} free of {}",
                                            fmt_bytes(d.available),
                                            fmt_bytes(d.total)
                                        ));
                                    });
                                });
                                widgets::bar(ui, &pal, frac, pal.load(frac), 8.0);
                                ui.add_space(8.0);
                            }
                            // Top disk users
                            let mut v: Vec<_> = s
                                .procs
                                .iter()
                                .filter(|p| p.disk_read + p.disk_write > 0)
                                .collect();
                            v.sort_by_key(|p| std::cmp::Reverse(p.disk_read + p.disk_write));
                            if !v.is_empty() {
                                widgets::section_title(ui, &pal, "Busiest processes");
                                for p in v.into_iter().take(6) {
                                    ui.horizontal(|ui| {
                                        widgets::avatar(ui, &p.name, pal.dark);
                                        ui.label(&*p.name);
                                        ui.with_layout(
                                            Layout::right_to_left(Align::Center),
                                            |ui| {
                                                ui.label(
                                                    RichText::new(format!(
                                                        "R {}  W {}",
                                                        fmt_rate(p.disk_read as f32),
                                                        fmt_rate(p.disk_write as f32)
                                                    ))
                                                    .color(pal.text_dim),
                                                );
                                            },
                                        );
                                    });
                                }
                            }
                        });
                    }
                    PerfSel::Gpu(i) => {
                        self.gpu_detail(ui, &s, h.gpus.get(i), i, window, interval);
                    }
                    PerfSel::Net(name) => {
                        let n = s
                            .networks
                            .iter()
                            .find(|n| n.name == name)
                            .cloned()
                            .unwrap_or_default();
                        let empty = std::collections::VecDeque::new();
                        let (rx, tx) = h
                            .per_if
                            .get(&name)
                            .map(|x| (&x.0, &x.1))
                            .unwrap_or((&empty, &empty));
                        widgets::card(ui, &pal, |ui| {
                            ui.horizontal(|ui| {
                                ui.label(RichText::new("Network").size(18.0).strong());
                                ui.label(RichText::new(&name).color(pal.text_dim));
                            });
                            ui.add_space(6.0);
                            widgets::graph(
                                ui,
                                &pal,
                                &[
                                    Series {
                                        data: rx,
                                        color: pal.net_rx,
                                        label: "Receive",
                                    },
                                    Series {
                                        data: tx,
                                        color: pal.net_tx,
                                        label: "Send",
                                    },
                                ],
                                opts(240.0, None, &|v| fmt_bits(v)),
                            );
                            ui.horizontal(|ui| {
                                widgets::legend(
                                    ui,
                                    pal.net_rx,
                                    &format!("Receive {}", fmt_bits(n.rx_rate as f32)),
                                );
                                widgets::legend(
                                    ui,
                                    pal.net_tx,
                                    &format!("Send {}", fmt_bits(n.tx_rate as f32)),
                                );
                            });
                        });
                        ui.add_space(10.0);
                        widgets::card(ui, &pal, |ui| {
                            ui.set_width(ui.available_width());
                            stat_row(
                                ui,
                                &pal,
                                &[
                                    ("Received (total)", fmt_bytes(n.total_rx)),
                                    ("Sent (total)", fmt_bytes(n.total_tx)),
                                    ("MAC address", n.mac.clone()),
                                ],
                            );
                            if !n.ips.is_empty() {
                                ui.add_space(6.0);
                                widgets::section_title(ui, &pal, "Addresses");
                                for ip in &n.ips {
                                    ui.label(RichText::new(ip).monospace().color(pal.text_dim));
                                }
                            }
                        });
                    }
                }
            });
    }

    pub fn page_activity(&mut self, ui: &mut egui::Ui) {
        let pal = self.pal;
        let shared = self.mon.shared.clone();
        let mut clear = false;
        let mut goto = None;
        ui.horizontal(|ui| {
            ui.add(
                egui::TextEdit::singleline(&mut self.log_filter)
                    .hint_text("🔍  Filter by name, PID or text")
                    .desired_width(300.0),
            );
            let labels = ["▶ Started", "■ Exited", "⚠ Alerts", "✔ Actions"];
            for (i, l) in labels.iter().enumerate() {
                ui.checkbox(&mut self.log_kinds[i], *l);
            }
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                if ui.button("🗑 Clear").clicked() {
                    clear = true;
                }
                ui.checkbox(
                    &mut self.settings.log_process_events,
                    "Show launches & exits",
                );
            });
        });
        ui.add_space(6.0);

        let g = shared.lock().unwrap();
        let q = self.log_filter.to_lowercase();
        let kinds = self.log_kinds;
        let log_procs = self.settings.log_process_events;
        let items: Vec<_> = g
            .log
            .iter()
            .rev()
            .filter(|e| {
                let k = match e.kind {
                    EventKind::Started => 0,
                    EventKind::Exited => 1,
                    EventKind::Alert => 2,
                    EventKind::Action => 3,
                };
                kinds[k] && (log_procs || k >= 2)
            })
            .filter(|e| {
                q.is_empty()
                    || crate::ui::processes::contains_ci(&e.name, &q)
                    || e.pid.to_string() == q
                    || crate::ui::processes::contains_ci(&e.detail, &q)
            })
            .cloned()
            .collect();
        drop(g);

        if items.is_empty() {
            widgets::empty_state(
                ui,
                &pal,
                "🕘",
                "Nothing logged yet",
                "New processes, exits, alerts and your actions show up here as they happen.",
            );
        } else {
            egui::Frame::new()
                .fill(pal.card)
                .stroke(egui::Stroke::new(1.0f32, pal.border))
                .corner_radius(12.0)
                .inner_margin(egui::Margin::symmetric(6, 4))
                .show(ui, |ui| {
                    egui_extras::TableBuilder::new(ui)
                        .striped(true)
                        .cell_layout(Layout::left_to_right(Align::Center))
                        .auto_shrink([false, false])
                        .column(egui_extras::Column::exact(175.0))
                        .column(egui_extras::Column::exact(90.0))
                        .column(egui_extras::Column::initial(200.0).clip(true))
                        .column(egui_extras::Column::exact(70.0))
                        .column(egui_extras::Column::remainder().clip(true))
                        .header(26.0, |mut h| {
                            for t in ["Time", "Event", "Process", "PID", "Details"] {
                                h.col(|ui| {
                                    ui.label(RichText::new(t).size(11.0).color(pal.text_dim));
                                });
                            }
                        })
                        .body(|body| {
                            body.rows(26.0, items.len(), |mut row| {
                                let e = &items[row.index()];
                                let (icon, col) = event_style(e.kind, &pal);
                                row.col(|ui| {
                                    ui.label(
                                        RichText::new(
                                            e.time.format("%Y-%m-%d %H:%M:%S").to_string(),
                                        )
                                        .monospace()
                                        .color(pal.text_dim),
                                    );
                                });
                                row.col(|ui| {
                                    ui.label(
                                        RichText::new(format!("{icon} {:?}", e.kind)).color(col),
                                    );
                                });
                                row.col(|ui| {
                                    if e.pid > 0 && e.kind == EventKind::Started {
                                        if ui.link(&*e.name).clicked() {
                                            goto = Some(e.pid);
                                        }
                                    } else {
                                        ui.label(&*e.name);
                                    }
                                });
                                row.col(|ui| {
                                    ui.label(
                                        RichText::new(if e.pid > 0 {
                                            e.pid.to_string()
                                        } else {
                                            String::new()
                                        })
                                        .color(pal.text_dim),
                                    );
                                });
                                row.col(|ui| {
                                    ui.label(RichText::new(&e.detail).color(pal.text_dim))
                                        .on_hover_text(&e.detail);
                                });
                            });
                        });
                });
        }
        if clear {
            shared.lock().unwrap().log.clear();
        }
        if let Some(pid) = goto {
            if self.snap.procs.iter().any(|p| p.pid == pid) {
                self.reveal_process(pid);
            } else {
                self.toast(
                    crate::app::ToastKind::Info,
                    "That process has already exited",
                );
            }
        }
    }
}

pub fn event_style(k: EventKind, pal: &crate::ui::theme::Palette) -> (&'static str, egui::Color32) {
    match k {
        EventKind::Started => ("▶", pal.good),
        EventKind::Exited => ("■", pal.text_faint),
        EventKind::Alert => ("⚠", pal.bad),
        EventKind::Action => ("✔", pal.accent),
    }
}

/// "Who is using it" list under the CPU / Memory graphs.
fn top_card(
    ui: &mut egui::Ui,
    pal: &crate::ui::theme::Palette,
    s: &crate::monitor::Snapshot,
    by_cpu: bool,
) {
    widgets::card(ui, pal, |ui| {
        ui.set_width(ui.available_width());
        widgets::section_title(
            ui,
            pal,
            if by_cpu {
                "Using the most CPU"
            } else {
                "Using the most memory"
            },
        );
        ui.add_space(4.0);
        let mut v: Vec<_> = s.procs.iter().collect();
        if by_cpu {
            v.sort_by(|a, b| b.cpu.total_cmp(&a.cpu));
        } else {
            v.sort_by(|a, b| b.mem.cmp(&a.mem));
        }
        let col = if by_cpu { pal.cpu } else { pal.mem };
        let maxv = v
            .first()
            .map(|p| if by_cpu { p.cpu } else { p.mem as f32 })
            .unwrap_or(1.0)
            .max(0.01);
        for p in v.into_iter().take(8) {
            let (val, txt) = if by_cpu {
                (p.cpu, format!("{:.1}%", p.cpu))
            } else {
                (p.mem as f32, fmt_bytes(p.mem))
            };
            ui.horizontal(|ui| {
                widgets::avatar(ui, &p.name, pal.dark);
                ui.allocate_ui_with_layout(
                    vec2(200.0, 18.0),
                    Layout::left_to_right(Align::Center),
                    |ui| {
                        ui.set_width(200.0);
                        ui.add(egui::Label::new(&*p.name).truncate());
                    },
                );
                ui.allocate_ui_with_layout(
                    vec2(80.0, 18.0),
                    Layout::right_to_left(Align::Center),
                    |ui| {
                        ui.set_width(80.0);
                        ui.label(RichText::new(txt).strong());
                    },
                );
                widgets::bar(ui, pal, val / maxv, col, 6.0);
            });
        }
    });
}

fn stat_row(ui: &mut egui::Ui, pal: &crate::ui::theme::Palette, items: &[(&str, String)]) {
    ui.horizontal_wrapped(|ui| {
        ui.spacing_mut().item_spacing = vec2(28.0, 10.0);
        for (k, v) in items {
            ui.vertical(|ui| {
                ui.spacing_mut().item_spacing.y = 1.0;
                ui.label(RichText::new(*k).size(11.0).color(pal.text_dim));
                ui.label(RichText::new(v).size(17.0).strong());
            });
        }
    });
}

fn composition(
    ui: &mut egui::Ui,
    pal: &crate::ui::theme::Palette,
    parts: &[(u64, egui::Color32, &str)],
    total: u64,
) {
    let w = ui.available_width();
    let (rect, _) = ui.allocate_exact_size(vec2(w, 18.0), egui::Sense::hover());
    let p = ui.painter();
    p.rect_filled(rect, 6.0, pal.track);
    let mut x = rect.left();
    for (v, col, _) in parts {
        let pw = rect.width() * (*v as f32 / total.max(1) as f32);
        let r = egui::Rect::from_min_size(egui::pos2(x, rect.top()), vec2(pw, rect.height()));
        p.rect_filled(r, 6.0, *col);
        x += pw;
    }
    ui.horizontal(|ui| {
        for (v, col, label) in parts {
            widgets::legend(ui, *col, &format!("{label} {}", fmt_bytes(*v)));
        }
    });
}
