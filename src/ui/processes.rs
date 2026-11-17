//! Processes page: flat / tree / grouped views, sorting, filtering, heat-map
//! cells, context menus and the details side panel.

use std::collections::{HashMap, HashSet};

use eframe::egui::{self, vec2, Align, Layout, RichText, Sense, Stroke};
use egui_extras::{Column, TableBuilder};

use crate::app::{App, Col, ViewMode};
use crate::monitor::{ProcInfo, Snapshot};
use crate::platform::{self, Priority};
use crate::ui::theme;
use crate::ui::widgets::{self, fmt_bytes, fmt_dur, fmt_epoch, fmt_rate};

/// One visible line in the process table.
pub struct Row {
    pub pid: Option<u32>,
    pub idx: Option<usize>,
    pub group: Option<String>,
    pub depth: usize,
    pub has_children: bool,
    pub expanded: bool,
    pub name: crate::monitor::Text,
    pub count: usize,
    pub cpu: f32,
    pub mem: u64,
    pub disk: u64,
    pub user: crate::monitor::Text,
    pub status: &'static str,
    pub started: u64,
    pub exe: crate::monitor::Text,
    pub cmd: crate::monitor::Text,
    pub pinned: bool,
    pub gpu: f32,
    pub gpu_mem: u64,
    pub net: u64,
}

impl Row {
    fn from_proc(i: usize, p: &ProcInfo, depth: usize, pinned: bool) -> Self {
        Row {
            pid: Some(p.pid),
            idx: Some(i),
            group: None,
            depth,
            has_children: false,
            expanded: false,
            name: p.name.clone(),
            count: 1,
            cpu: p.cpu,
            mem: p.mem,
            disk: p.disk_read + p.disk_write,
            user: p.user.clone(),
            status: p.status,
            started: p.start_time,
            exe: p.exe.clone(),
            cmd: p.cmd.clone(),
            pinned,
            gpu: p.gpu,
            gpu_mem: p.gpu_mem,
            net: p.net_rx + p.net_tx,
        }
    }
}

/// Case-insensitive `contains`; `needle` must already be lower-case.
/// Allocation-free for ASCII text (process names and paths almost always are).
pub fn contains_ci(hay: &str, needle: &str) -> bool {
    if needle.is_empty() {
        return true;
    }
    if hay.is_ascii() && needle.is_ascii() {
        let (h, n) = (hay.as_bytes(), needle.as_bytes());
        return h.len() >= n.len()
            && h.windows(n.len())
                .any(|w| w.iter().zip(n).all(|(a, b)| a.to_ascii_lowercase() == *b));
    }
    hay.to_lowercase().contains(needle)
}

/// Case-insensitive ordering without allocating.
pub fn cmp_ci(a: &str, b: &str) -> std::cmp::Ordering {
    a.chars()
        .flat_map(char::to_lowercase)
        .cmp(b.chars().flat_map(char::to_lowercase))
}

/// `terms` are the lower-cased, space-separated search words; all must match
/// the name, PID, user, path or command line.
fn matches(p: &ProcInfo, terms: &[String]) -> bool {
    terms.iter().all(|t| {
        contains_ci(&p.name, t)
            || t.parse::<u32>().is_ok_and(|n| n == p.pid)
            || contains_ci(&p.user, t)
            || contains_ci(&p.exe, t)
            || contains_ci(&p.cmd, t)
    })
}

/// Everything the visible process list depends on; rows are only rebuilt when it changes.
#[derive(Clone, PartialEq)]
pub struct RowsKey {
    seq: u64,
    search: String,
    view: ViewMode,
    col: Col,
    desc: bool,
    only_mine: bool,
    gen: u64,
}

fn cmp_rows(a: &Row, b: &Row, col: Col, publisher: &dyn Fn(&str) -> String) -> std::cmp::Ordering {
    use std::cmp::Ordering::*;
    let o = match col {
        Col::Gpu => a.gpu.total_cmp(&b.gpu),
        Col::GpuMem => a.gpu_mem.cmp(&b.gpu_mem),
        Col::Net => a.net.cmp(&b.net),
        Col::Publisher => cmp_ci(&publisher(&a.exe), &publisher(&b.exe)),
        Col::Name => cmp_ci(&a.name, &b.name),
        Col::Pid => a.pid.unwrap_or(0).cmp(&b.pid.unwrap_or(0)),
        Col::Status => a.status.cmp(b.status),
        Col::User => cmp_ci(&a.user, &b.user),
        Col::Cpu => a.cpu.total_cmp(&b.cpu),
        Col::Mem => a.mem.cmp(&b.mem),
        Col::Disk => a.disk.cmp(&b.disk),
        Col::Started => a.started.cmp(&b.started),
        Col::Path => cmp_ci(&a.exe, &b.exe),
        Col::Cmd => cmp_ci(&a.cmd, &b.cmd),
    };
    if o == Equal {
        a.pid.unwrap_or(0).cmp(&b.pid.unwrap_or(0))
    } else {
        o
    }
}

impl App {
    /// Signer name, or a status like "Not signed", once signatures were checked.
    pub fn publisher(&self, exe: &str) -> String {
        match self.sig_for(exe) {
            Some(s) if s.verified() => s.signer.clone(),
            Some(s) => s.status.clone(),
            None => String::new(),
        }
    }

    fn sort_rows(&self, rows: &mut [Row]) {
        let (col, desc) = (self.settings.sort_col, self.settings.sort_desc);
        rows.sort_by(|a, b| {
            b.pinned.cmp(&a.pinned).then_with(|| {
                let o = cmp_rows(a, b, col, &|e| self.publisher(e));
                if desc {
                    o.reverse()
                } else {
                    o
                }
            })
        });
    }

    fn visible(&self, s: &Snapshot) -> Vec<usize> {
        let terms: Vec<String> = self
            .search
            .split_whitespace()
            .map(|t| t.to_lowercase())
            .collect();
        let me_suffix = format!("\\{}", self.me.to_lowercase());
        s.procs
            .iter()
            .enumerate()
            .filter(|(_, p)| matches(p, &terms))
            .filter(|(_, p)| {
                !self.settings.only_mine
                    || self.me.is_empty()
                    || p.user.eq_ignore_ascii_case(&self.me)
                    || p.user.to_lowercase().ends_with(&me_suffix)
            })
            .map(|(i, _)| i)
            .collect()
    }

    /// Rows for the table, rebuilt only when the snapshot or view settings change
    /// (instead of filtering and sorting every frame).
    pub fn cached_rows(&mut self, s: &Snapshot) -> std::rc::Rc<Vec<Row>> {
        let key = RowsKey {
            seq: s.seq,
            search: self.search.clone(),
            view: self.settings.view_mode,
            col: self.settings.sort_col,
            desc: self.settings.sort_desc,
            only_mine: self.settings.only_mine,
            gen: self.rows_gen,
        };
        if let Some((k, rows)) = &self.rows_cache {
            if *k == key {
                return rows.clone();
            }
        }
        let rows = std::rc::Rc::new(self.build_rows(s));
        self.rows_cache = Some((key, rows.clone()));
        rows
    }

    pub fn build_rows(&self, s: &Snapshot) -> Vec<Row> {
        let vis = self.visible(s);
        match self.settings.view_mode {
            ViewMode::Flat => {
                let mut rows: Vec<Row> = vis
                    .iter()
                    .map(|&i| Row::from_proc(i, &s.procs[i], 0, self.is_pinned(&s.procs[i].name)))
                    .collect();
                self.sort_rows(&mut rows);
                rows
            }
            ViewMode::Grouped => {
                let mut groups: HashMap<String, Vec<usize>> = HashMap::new();
                for &i in &vis {
                    groups
                        .entry(s.procs[i].name.to_lowercase())
                        .or_default()
                        .push(i);
                }
                let mut heads: Vec<(Row, Vec<usize>)> = groups
                    .into_iter()
                    .map(|(key, idxs)| {
                        let first = &s.procs[idxs[0]];
                        let pinned = self.is_pinned(&first.name);
                        let mut r = Row::from_proc(idxs[0], first, 0, pinned);
                        if idxs.len() > 1 {
                            r.pid = None;
                            r.idx = None;
                            r.group = Some(key.clone());
                            r.count = idxs.len();
                            r.has_children = true;
                            r.expanded =
                                self.expanded_groups.contains(&key) || !self.search.is_empty();
                            r.cpu = idxs.iter().map(|&i| s.procs[i].cpu).sum();
                            r.gpu = idxs.iter().map(|&i| s.procs[i].gpu).sum::<f32>().min(100.0);
                            r.gpu_mem = idxs.iter().map(|&i| s.procs[i].gpu_mem).sum();
                            r.net = idxs
                                .iter()
                                .map(|&i| s.procs[i].net_rx + s.procs[i].net_tx)
                                .sum();
                            r.mem = idxs.iter().map(|&i| s.procs[i].mem).sum();
                            r.disk = idxs
                                .iter()
                                .map(|&i| s.procs[i].disk_read + s.procs[i].disk_write)
                                .sum();
                            r.started = idxs
                                .iter()
                                .map(|&i| s.procs[i].start_time)
                                .min()
                                .unwrap_or(0);
                        }
                        (r, idxs)
                    })
                    .collect();
                let (col, desc) = (self.settings.sort_col, self.settings.sort_desc);
                heads.sort_by(|a, b| {
                    b.0.pinned.cmp(&a.0.pinned).then_with(|| {
                        let o = cmp_rows(&a.0, &b.0, col, &|e| self.publisher(e));
                        if desc {
                            o.reverse()
                        } else {
                            o
                        }
                    })
                });
                let mut rows = Vec::new();
                for (head, idxs) in heads {
                    let expanded = head.expanded && head.has_children;
                    let pinned = head.pinned;
                    rows.push(head);
                    if expanded {
                        let mut kids: Vec<Row> = idxs
                            .iter()
                            .map(|&i| Row::from_proc(i, &s.procs[i], 1, pinned))
                            .collect();
                        self.sort_rows(&mut kids);
                        rows.extend(kids);
                    }
                }
                rows
            }
            ViewMode::Tree => {
                let by_pid: HashMap<u32, usize> = s
                    .procs
                    .iter()
                    .enumerate()
                    .map(|(i, p)| (p.pid, i))
                    .collect();
                // Keep matches plus their ancestors so the hierarchy stays readable.
                let mut keep: HashSet<usize> = vis.iter().copied().collect();
                if !self.search.trim().is_empty() || self.settings.only_mine {
                    for &i in &vis {
                        let mut cur = s.procs[i].parent;
                        let mut guard = 0;
                        while let Some(pp) = cur {
                            let Some(&pi) = by_pid.get(&pp) else { break };
                            if !keep.insert(pi) || guard > 64 {
                                break;
                            }
                            guard += 1;
                            cur = s.procs[pi].parent;
                        }
                    }
                }
                let mut children: HashMap<u32, Vec<usize>> = HashMap::new();
                let mut roots = Vec::new();
                for &i in &keep {
                    let p = &s.procs[i];
                    match p.parent.filter(|pp| {
                        *pp != p.pid && by_pid.get(pp).is_some_and(|pi| keep.contains(pi))
                    }) {
                        Some(pp) => children.entry(pp).or_default().push(i),
                        None => roots.push(i),
                    }
                }
                let mut rows = Vec::new();
                let mut root_rows: Vec<Row> = roots
                    .iter()
                    .map(|&i| Row::from_proc(i, &s.procs[i], 0, self.is_pinned(&s.procs[i].name)))
                    .collect();
                self.sort_rows(&mut root_rows);
                fn walk(
                    app: &App,
                    s: &Snapshot,
                    row: Row,
                    children: &HashMap<u32, Vec<usize>>,
                    out: &mut Vec<Row>,
                    seen: &mut HashSet<u32>,
                ) {
                    let pid = row.pid.unwrap_or(0);
                    if !seen.insert(pid) {
                        return;
                    }
                    let kids = children.get(&pid);
                    let mut row = row;
                    row.has_children = kids.is_some_and(|k| !k.is_empty());
                    row.expanded = row.has_children && !app.collapsed.contains(&pid);
                    let depth = row.depth;
                    let expanded = row.expanded;
                    out.push(row);
                    if expanded {
                        let mut kr: Vec<Row> = kids
                            .unwrap()
                            .iter()
                            .map(|&i| {
                                Row::from_proc(
                                    i,
                                    &s.procs[i],
                                    depth + 1,
                                    app.is_pinned(&s.procs[i].name),
                                )
                            })
                            .collect();
                        app.sort_rows(&mut kr);
                        for k in kr {
                            walk(app, s, k, children, out, seen);
                        }
                    }
                }
                let mut seen = HashSet::new();
                for r in root_rows {
                    walk(self, s, r, &children, &mut rows, &mut seen);
                }
                rows
            }
        }
    }

    pub fn page_processes(&mut self, ui: &mut egui::Ui) {
        let pal = self.pal;
        let s = self.snap.clone();

        // ── Toolbar ──
        ui.horizontal(|ui| {
            let r = ui.add(
                egui::TextEdit::singleline(&mut self.search)
                    .hint_text("🔍  Search name, PID, user, path or command line   (Ctrl+F)")
                    .desired_width(380.0),
            );
            if self.focus_search {
                r.request_focus();
                self.focus_search = false;
            }
            if !self.search.is_empty() && ui.small_button("✖").clicked() {
                self.search.clear();
            }
            ui.add_space(8.0);
            for (mode, label, tip) in [
                (
                    ViewMode::Grouped,
                    "Grouped",
                    "Combine instances of the same app",
                ),
                (ViewMode::Tree, "Tree", "Show parent / child hierarchy"),
                (ViewMode::Flat, "Flat", "Every process on its own row"),
            ] {
                if ui
                    .selectable_label(self.settings.view_mode == mode, label)
                    .on_hover_text(tip)
                    .clicked()
                {
                    self.settings.view_mode = mode;
                }
            }
            ui.add_space(8.0);
            ui.checkbox(&mut self.settings.only_mine, "Only mine");

            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                let sel = self
                    .selected
                    .and_then(|pid| s.procs.iter().find(|p| p.pid == pid).cloned());
                let group = self.selected_group.clone();
                let can = sel.is_some() || group.is_some();
                if theme::danger_button(ui, &pal, "End task", can)
                    .on_hover_text("Del  ·  Shift+Del ends the whole tree")
                    .clicked()
                {
                    if let Some(p) = &sel {
                        self.request_kill(p.pid, &p.name, false);
                    } else if let Some(g) = &group {
                        self.request_kill_all(g);
                    }
                }
                ui.menu_button("Columns ⏷", |ui| {
                    for c in Col::ALL {
                        if c == Col::Name {
                            continue;
                        }
                        let mut on = self.settings.columns.contains(&c);
                        if ui.checkbox(&mut on, c.label()).changed() {
                            if on {
                                self.settings.columns.push(c);
                                self.settings
                                    .columns
                                    .sort_by_key(|x| Col::ALL.iter().position(|y| y == x));
                            } else {
                                self.settings.columns.retain(|x| *x != c);
                            }
                        }
                    }
                    ui.separator();
                    ui.checkbox(&mut self.settings.heatmap, "Heat-map colouring");
                    ui.checkbox(&mut self.settings.compact_rows, "Compact rows");
                    ui.checkbox(&mut self.settings.show_details, "Details panel");
                });
                if ui
                    .button("⬇ Export")
                    .on_hover_text("Save the process list as CSV (Ctrl+E)")
                    .clicked()
                {
                    self.export_csv();
                }
                let verifying = self.sig_rx.is_some();
                if ui
                    .add_enabled(!verifying, egui::Button::new(if verifying { "⏳ Verifying…" } else { "🛡 Verify" }))
                    .on_hover_text("Check which programs are digitally signed by their publisher (adds a Publisher column)")
                    .clicked()
                {
                    self.verify_all_signatures();
                }
            });
        });
        ui.add_space(6.0);

        // ── Details panel ──
        let sel_proc = self
            .selected
            .and_then(|pid| s.procs.iter().find(|p| p.pid == pid).cloned());
        if self.settings.show_details {
            if let Some(p) = sel_proc.clone() {
                egui::SidePanel::right("details")
                    .resizable(true)
                    .default_width(330.0)
                    .width_range(280.0..=520.0)
                    .frame(egui::Frame::new().inner_margin(egui::Margin {
                        left: 12,
                        right: 0,
                        top: 0,
                        bottom: 0,
                    }))
                    .show_inside(ui, |ui| self.details_panel(ui, &s, &p));
            }
        }

        // ── Table ──
        let rows = self.cached_rows(&s);
        if rows.is_empty() {
            widgets::empty_state(
                ui,
                &pal,
                "🔍",
                "No matching processes",
                "Try a different search or turn off “Only mine”.",
            );
            return;
        }

        let cols = self.settings.columns.clone();
        let row_h = if self.settings.compact_rows {
            22.0
        } else {
            28.0
        };
        let mut scroll_to = None;
        if self.scroll_to_selected {
            scroll_to = rows
                .iter()
                .position(|r| r.pid.is_some() && r.pid == self.selected);
            self.scroll_to_selected = false;
        }

        let mut clicked_row: Option<usize> = None;
        let mut toggle: Option<usize> = None;
        let mut actions: Vec<RowAction> = Vec::new();
        let mut header_click: Option<Col> = None;

        // Never let a wide column set paint over the details panel.
        ui.shrink_clip_rect(ui.available_rect_before_wrap());
        egui::Frame::new()
            .fill(pal.card)
            .stroke(Stroke::new(1.0f32, pal.border))
            .corner_radius(12.0)
            .inner_margin(egui::Margin::symmetric(6, 4))
            .show(ui, |ui| {
                let mut tb = TableBuilder::new(ui)
                    .striped(true)
                    .resizable(true)
                    .sense(Sense::click())
                    .cell_layout(Layout::left_to_right(Align::Center))
                    .min_scrolled_height(0.0)
                    .auto_shrink([false, false]);
                if let Some(i) = scroll_to {
                    tb = tb.scroll_to_row(i, Some(Align::Center));
                }
                tb = tb.column(Column::remainder().at_least(170.0).clip(true));
                for c in cols.iter().filter(|c| **c != Col::Name) {
                    tb = tb.column(match c {
                        Col::Name => unreachable!(),
                        Col::Pid => Column::initial(70.0).at_least(50.0),
                        Col::Status => Column::initial(80.0).at_least(50.0),
                        Col::User => Column::initial(110.0).at_least(60.0).clip(true),
                        Col::Cpu => Column::initial(80.0).at_least(60.0),
                        Col::Mem => Column::initial(96.0).at_least(70.0),
                        Col::Disk => Column::initial(96.0).at_least(70.0),
                        Col::Started => Column::initial(140.0).at_least(80.0),
                        Col::Path => Column::initial(220.0).at_least(80.0).clip(true),
                        Col::Cmd => Column::initial(300.0).at_least(120.0).clip(true),
                        Col::Gpu => Column::initial(72.0).at_least(56.0),
                        Col::GpuMem => Column::initial(96.0).at_least(70.0),
                        Col::Net => Column::initial(96.0).at_least(70.0),
                        Col::Publisher => Column::initial(170.0).at_least(80.0).clip(true),
                    });
                }

                let totals = (
                    s.cpu_total,
                    s.mem_used,
                    s.disk_read_rate + s.disk_write_rate,
                );
                let gpu_total = s.gpu_util();
                let gpu_mem_total: u64 = s.gpus.iter().map(|g| g.ded_used).sum();
                let net_total: u64 = s.procs.iter().map(|p| p.net_rx + p.net_tx).sum();
                let net_live = s.net_status == Some(crate::native::NetStatus::Running);
                tb.header(30.0, |mut h| {
                    let mut head = |h: &mut egui_extras::TableRow, c: Col, sub: Option<String>| {
                        h.col(|ui| {
                            let active = self.settings.sort_col == c;
                            let arrow = if active {
                                if self.settings.sort_desc {
                                    " ⏷"
                                } else {
                                    " ⏶"
                                }
                            } else {
                                ""
                            };
                            ui.vertical(|ui| {
                                ui.spacing_mut().item_spacing.y = 0.0;
                                if let Some(sub) = &sub {
                                    ui.label(
                                        RichText::new(sub).size(12.0).strong().color(pal.text),
                                    );
                                }
                                let txt = RichText::new(format!("{}{arrow}", c.label()))
                                    .size(11.0)
                                    .color(if active { pal.accent } else { pal.text_dim });
                                if ui
                                    .add(egui::Label::new(txt).sense(Sense::click()))
                                    .clicked()
                                {
                                    header_click = Some(c);
                                }
                            });
                        });
                    };
                    head(&mut h, Col::Name, None);
                    for c in &cols {
                        let sub = match c {
                            Col::Cpu => Some(format!("{:.0}%", totals.0)),
                            Col::Mem => Some(fmt_bytes(totals.1)),
                            Col::Disk => Some(fmt_rate(totals.2 as f32)),
                            Col::Gpu if !s.gpus.is_empty() => Some(format!("{gpu_total:.0}%")),
                            Col::GpuMem if !s.gpus.is_empty() => Some(fmt_bytes(gpu_mem_total)),
                            Col::Net if net_live => Some(fmt_rate(net_total as f32)),
                            _ => None,
                        };
                        if *c != Col::Name {
                            head(&mut h, *c, sub);
                        }
                    }
                })
                .body(|body| {
                    body.rows(row_h, rows.len(), |mut row| {
                        let i = row.index();
                        let r = &rows[i];
                        let selected = match (&r.group, r.pid) {
                            (Some(g), _) => self.selected_group.as_ref() == Some(g),
                            (None, Some(pid)) => self.selected == Some(pid),
                            _ => false,
                        };
                        row.set_selected(selected);

                        // Name cell
                        row.col(|ui| {
                            ui.add_space(r.depth as f32 * 16.0);
                            if r.has_children {
                                let t = if r.expanded { "⏷" } else { "⏵" };
                                if ui
                                    .add(
                                        egui::Label::new(RichText::new(t).color(pal.text_dim))
                                            .sense(Sense::click()),
                                    )
                                    .clicked()
                                {
                                    toggle = Some(i);
                                }
                            } else if self.settings.view_mode != ViewMode::Flat {
                                ui.add_space(14.0);
                            }
                            widgets::avatar(ui, &r.name, pal.dark);
                            let mut name = RichText::new(&*r.name);
                            if r.depth == 0 {
                                name = name.strong();
                            }
                            ui.label(name);
                            if r.count > 1 {
                                ui.label(
                                    RichText::new(format!("({})", r.count)).color(pal.text_faint),
                                );
                            }
                            if r.pinned {
                                ui.label(RichText::new("★").color(pal.warn).size(11.0));
                            }
                            if r.status == "Stop"
                                || r.pid.is_some_and(|p| self.suspended.contains(&p))
                            {
                                widgets::pill(ui, "⏸ Suspended", pal.warn);
                            }
                        });
                        for c in &cols {
                            match c {
                                Col::Name => {}
                                Col::Pid => {
                                    row.col(|ui| {
                                        ui.label(
                                            RichText::new(
                                                r.pid.map(|p| p.to_string()).unwrap_or_default(),
                                            )
                                            .color(pal.text_dim),
                                        );
                                    });
                                }
                                Col::Status => {
                                    row.col(|ui| {
                                        ui.label(
                                            RichText::new(if r.group.is_some() {
                                                String::new()
                                            } else {
                                                r.status.to_string()
                                            })
                                            .color(pal.text_dim),
                                        );
                                    });
                                }
                                Col::User => {
                                    row.col(|ui| {
                                        ui.label(RichText::new(&*r.user).color(pal.text_dim));
                                    });
                                }
                                Col::Cpu => {
                                    row.col(|ui| {
                                        self.metric_cell(
                                            ui,
                                            selected,
                                            r.cpu / 100.0,
                                            format!("{:.1}%", r.cpu),
                                        )
                                    });
                                }
                                Col::Mem => {
                                    let frac = r.mem as f32 / s.mem_total.max(1) as f32;
                                    row.col(|ui| {
                                        self.metric_cell(
                                            ui,
                                            selected,
                                            (frac * 3.0).min(1.0),
                                            fmt_bytes(r.mem),
                                        )
                                    });
                                }
                                Col::Disk => {
                                    let frac = (r.disk as f32 / (50.0 * 1024.0 * 1024.0)).min(1.0);
                                    row.col(|ui| {
                                        self.metric_cell(
                                            ui,
                                            selected,
                                            frac,
                                            fmt_rate(r.disk as f32),
                                        )
                                    });
                                }
                                Col::Started => {
                                    row.col(|ui| {
                                        ui.label(
                                            RichText::new(fmt_epoch(r.started)).color(pal.text_dim),
                                        );
                                    });
                                }
                                Col::Path => {
                                    row.col(|ui| {
                                        ui.label(RichText::new(&*r.exe).color(pal.text_dim))
                                            .on_hover_text(&*r.exe);
                                    });
                                }
                                Col::Cmd => {
                                    row.col(|ui| {
                                        ui.label(
                                            RichText::new(&*r.cmd).color(pal.text_dim).monospace(),
                                        )
                                        .on_hover_text(&*r.cmd);
                                    });
                                }
                                Col::Gpu => {
                                    row.col(|ui| {
                                        self.metric_cell(
                                            ui,
                                            selected,
                                            r.gpu / 100.0,
                                            format!("{:.1}%", r.gpu),
                                        )
                                    });
                                }
                                Col::GpuMem => {
                                    let total = s
                                        .gpus
                                        .iter()
                                        .map(|g| g.ded_total)
                                        .max()
                                        .unwrap_or(0)
                                        .max(1);
                                    let frac = (r.gpu_mem as f32 / total as f32 * 3.0).min(1.0);
                                    row.col(|ui| {
                                        self.metric_cell(ui, selected, frac, fmt_bytes(r.gpu_mem))
                                    });
                                }
                                Col::Net => {
                                    let frac = (r.net as f32 / (5.0 * 1024.0 * 1024.0)).min(1.0);
                                    row.col(|ui| {
                                        if net_live {
                                            self.metric_cell(
                                                ui,
                                                selected,
                                                frac,
                                                fmt_rate(r.net as f32),
                                            )
                                        } else {
                                            ui.label(RichText::new("—").color(pal.text_faint))
                                                .on_hover_text(crate::native::net_status_hint());
                                        }
                                    });
                                }
                                Col::Publisher => {
                                    row.col(|ui| match self.sig_for(&r.exe) {
                                        Some(sig) if sig.verified() => {
                                            ui.label(RichText::new("✔").color(pal.good));
                                            ui.label(
                                                RichText::new(&sig.signer).color(pal.text_dim),
                                            )
                                            .on_hover_text(format!(
                                                "Verified signature: {}",
                                                sig.signer
                                            ));
                                        }
                                        Some(sig) => {
                                            ui.label(
                                                RichText::new(format!("⚠ {}", sig.status))
                                                    .color(pal.warn),
                                            )
                                            .on_hover_text(
                                                "This file is not signed by a trusted publisher",
                                            );
                                        }
                                        None => {}
                                    });
                                }
                            }
                        }

                        let resp = row.response();
                        if resp.clicked() {
                            clicked_row = Some(i);
                        }
                        if resp.double_clicked() {
                            if r.has_children {
                                toggle = Some(i);
                            } else {
                                actions.push(RowAction::ShowDetails);
                            }
                        }
                        resp.context_menu(|ui| {
                            clicked_row = Some(i);
                            self.row_menu(ui, r, &mut actions);
                        });
                    });
                });
            });

        // ── Apply interactions after drawing ──
        if let Some(c) = header_click {
            if self.settings.sort_col == c {
                self.settings.sort_desc = !self.settings.sort_desc;
            } else {
                self.settings.sort_col = c;
                self.settings.sort_desc =
                    matches!(c, Col::Cpu | Col::Mem | Col::Disk | Col::Started);
            }
        }
        if let Some(i) = clicked_row {
            let r = &rows[i];
            if let Some(g) = &r.group {
                self.selected_group = Some(g.clone());
                self.selected = None;
            } else {
                self.selected = r.pid;
                self.selected_group = None;
                if let Some(pid) = r.pid {
                    if !self.priority_cache.contains_key(&pid) {
                        self.fetch_priority(pid);
                    }
                }
            }
        }
        if let Some(i) = toggle {
            let r = &rows[i];
            if let Some(g) = &r.group {
                if !self.expanded_groups.remove(g) {
                    self.expanded_groups.insert(g.clone());
                }
                self.rows_gen += 1;
            } else if let Some(pid) = r.pid {
                if !self.collapsed.remove(&pid) {
                    self.collapsed.insert(pid);
                }
                self.rows_gen += 1;
            }
        }
        for a in actions {
            self.apply_row_action(a);
        }
    }

    fn metric_cell(&self, ui: &mut egui::Ui, selected: bool, frac: f32, text: String) {
        let pal = self.pal;
        if self.settings.heatmap && !selected {
            let r = ui.max_rect().expand2(vec2(4.0, 1.0));
            ui.painter().rect_filled(r, 0.0, widgets::heat(frac, &pal));
        }
        ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
            ui.add_space(4.0);
            ui.label(RichText::new(text).color(if frac > 0.005 {
                pal.text
            } else {
                pal.text_faint
            }));
        });
    }

    fn row_menu(&self, ui: &mut egui::Ui, r: &Row, actions: &mut Vec<RowAction>) {
        ui.set_min_width(220.0);
        let name = r.name.to_string();
        if let Some(pid) = r.pid {
            if ui.button("✖  End task").clicked() {
                actions.push(RowAction::Kill(pid, name.clone(), false));
                ui.close_menu();
            }
            if ui.button("🌳  End process tree").clicked() {
                actions.push(RowAction::Kill(pid, name.clone(), true));
                ui.close_menu();
            }
        }
        if ui.button(format!("⛔  End all “{}”", r.name)).clicked() {
            actions.push(RowAction::KillAll(name.clone()));
            ui.close_menu();
        }
        ui.separator();
        if let Some(pid) = r.pid {
            ui.menu_button("⚡  Set priority", |ui| {
                for p in Priority::ALL {
                    if ui.button(p.label()).clicked() {
                        actions.push(RowAction::Priority(pid, name.clone(), p));
                        ui.close_menu();
                    }
                }
            });
            if ui.button("🧮  Set affinity…").clicked() {
                actions.push(RowAction::Affinity(pid, name.clone()));
                ui.close_menu();
            }
            let susp = self.suspended.contains(&pid) || r.status == "Stop";
            if ui
                .button(if susp { "▶  Resume" } else { "⏸  Suspend" })
                .clicked()
            {
                actions.push(RowAction::Suspend(pid, name.clone(), !susp));
                ui.close_menu();
            }
            if ui
                .button("⟲  Restart")
                .on_hover_text("End it and start it again with the same command line")
                .clicked()
            {
                actions.push(RowAction::Restart(pid));
                ui.close_menu();
            }
            ui.separator();
            if ui.button("🛡  Check on VirusTotal").clicked() {
                actions.push(RowAction::VirusTotal(pid, name.clone(), r.exe.to_string()));
                ui.close_menu();
            }
            if ui.button("💾  Create dump file…").clicked() {
                actions.push(RowAction::Dump(pid, name.clone()));
                ui.close_menu();
            }
            ui.separator();
        }
        if ui.button("📂  Open file location").clicked() {
            actions.push(RowAction::OpenLocation(r.exe.to_string()));
            ui.close_menu();
        }
        if ui.button("🔎  Search online").clicked() {
            actions.push(RowAction::SearchOnline(r.name.to_string()));
            ui.close_menu();
        }
        ui.menu_button("📋  Copy", |ui| {
            for (label, val) in [
                ("Name", r.name.to_string()),
                ("PID", r.pid.map(|p| p.to_string()).unwrap_or_default()),
                ("Path", r.exe.to_string()),
                ("Command line", r.cmd.to_string()),
            ] {
                if ui
                    .add_enabled(!val.is_empty(), egui::Button::new(label))
                    .clicked()
                {
                    ui.ctx().copy_text(val);
                    ui.close_menu();
                }
            }
        });
        ui.separator();
        let pin = if self.is_pinned(&r.name) {
            "☆  Unpin from top"
        } else {
            "★  Pin to top"
        };
        if ui.button(pin).clicked() {
            actions.push(RowAction::TogglePin(name));
            ui.close_menu();
        }
        if r.pid.is_some() && ui.button("ℹ  Details").clicked() {
            actions.push(RowAction::ShowDetails);
            ui.close_menu();
        }
    }

    fn apply_row_action(&mut self, a: RowAction) {
        match a {
            RowAction::Kill(pid, name, tree) => self.request_kill(pid, &name, tree),
            RowAction::KillAll(name) => self.request_kill_all(&name),
            RowAction::Priority(pid, name, p) => self.set_priority(pid, &name, p),
            RowAction::Affinity(pid, name) => self.open_affinity(pid, &name),
            RowAction::OpenLocation(path) => {
                if let Err(e) = platform::open_location(&path) {
                    self.toast(crate::app::ToastKind::Err, e);
                }
            }
            RowAction::SearchOnline(name) => platform::open_url(&format!(
                "https://www.bing.com/search?q={}",
                platform::url_encode(&format!("{name} process"))
            )),
            RowAction::TogglePin(name) => self.toggle_pin(&name),
            RowAction::ShowDetails => self.settings.show_details = true,
            RowAction::Select(pid) => self.reveal_process(pid),
            RowAction::Suspend(pid, name, on) => self.set_suspended(pid, &name, on),
            RowAction::Restart(pid) => self.restart(pid),
            RowAction::VirusTotal(pid, name, exe) => self.virustotal(pid, &name, &exe),
            RowAction::Dump(pid, name) => self.create_dump(pid, &name),
            RowAction::LoadThreads(pid) => self.load_threads(pid),
            RowAction::Thread(pid, tid, crate::native::ThreadAction::Kill) => {
                self.confirm = Some(crate::app::Confirm {
                    title: format!("End thread {tid}?"),
                    body: "Ending a single thread can crash or corrupt the program. Only do this if you know what the thread is.".into(),
                    warn: None,
                    action: crate::app::ConfirmAction::KillThread { pid, tid },
                });
            }
            RowAction::Thread(pid, tid, a) => self.thread_action(pid, tid, a),
        }
    }

    fn details_panel(&mut self, ui: &mut egui::Ui, s: &Snapshot, p: &ProcInfo) {
        let pal = self.pal;
        let shared = self.mon.shared.clone();
        let mut actions = Vec::new();
        if !self.priority_cache.contains_key(&p.pid) {
            self.fetch_priority(p.pid);
        }
        egui::ScrollArea::vertical()
            .auto_shrink([false, false])
            .show(ui, |ui| {
                widgets::card(ui, &pal, |ui| {
                    ui.set_width(ui.available_width());
                    ui.horizontal(|ui| {
                        widgets::avatar(ui, &p.name, pal.dark);
                        ui.label(RichText::new(&*p.name).size(16.0).strong());
                        ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                            if ui
                                .small_button("✖")
                                .on_hover_text("Close details")
                                .clicked()
                            {
                                self.selected = None;
                            }
                        });
                    });
                    widgets::faint(ui, &pal, format!("PID {}  ·  {}", p.pid, p.status));
                    ui.add_space(8.0);
                    ui.horizontal_wrapped(|ui| {
                        if theme::danger_button(ui, &pal, "End task", true).clicked() {
                            actions.push(RowAction::Kill(p.pid, p.name.to_string(), false));
                        }
                        if ui.button("End tree").clicked() {
                            actions.push(RowAction::Kill(p.pid, p.name.to_string(), true));
                        }
                        if ui
                            .button("📂")
                            .on_hover_text("Open file location")
                            .clicked()
                        {
                            actions.push(RowAction::OpenLocation(p.exe.to_string()));
                        }
                        if ui.button("🔎").on_hover_text("Search online").clicked() {
                            actions.push(RowAction::SearchOnline(p.name.to_string()));
                        }
                        let star = if self.is_pinned(&p.name) {
                            "★"
                        } else {
                            "☆"
                        };
                        if ui.button(star).on_hover_text("Pin to top").clicked() {
                            actions.push(RowAction::TogglePin(p.name.to_string()));
                        }
                    });
                    ui.horizontal_wrapped(|ui| {
                        let susp = self.suspended.contains(&p.pid) || p.status == "Stop";
                        if ui
                            .button(if susp { "▶ Resume" } else { "⏸ Suspend" })
                            .clicked()
                        {
                            actions.push(RowAction::Suspend(p.pid, p.name.to_string(), !susp));
                        }
                        if ui
                            .button("⟲ Restart")
                            .on_hover_text("End and relaunch with the same command line")
                            .clicked()
                        {
                            actions.push(RowAction::Restart(p.pid));
                        }
                        if ui
                            .button("💾 Dump")
                            .on_hover_text("Create a full memory dump file for debugging")
                            .clicked()
                        {
                            actions.push(RowAction::Dump(p.pid, p.name.to_string()));
                        }
                    });
                });
                ui.add_space(8.0);

                // Live mini graphs from per-process history.
                widgets::card(ui, &pal, |ui| {
                    ui.set_width(ui.available_width());
                    let g = shared.lock().unwrap();
                    let hist = g.history.procs.get(&p.pid);
                    let cpu: std::collections::VecDeque<f32> = hist
                        .map(|h| h.1.iter().map(|x| x.0).collect())
                        .unwrap_or_default();
                    let mem: std::collections::VecDeque<f32> = hist
                        .map(|h| h.1.iter().map(|x| x.1).collect())
                        .unwrap_or_default();
                    let gpu: std::collections::VecDeque<f32> = hist
                        .map(|h| h.1.iter().map(|x| x.2).collect())
                        .unwrap_or_default();
                    drop(g);
                    ui.horizontal(|ui| {
                        widgets::legend(ui, pal.cpu, "CPU");
                        ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                            ui.label(
                                RichText::new(format!("{:.1}%", p.cpu))
                                    .strong()
                                    .color(pal.cpu),
                            );
                        });
                    });
                    let w = ui.available_width();
                    widgets::sparkline(ui, &pal, &cpu, pal.cpu, None, vec2(w, 54.0));
                    ui.add_space(6.0);
                    ui.horizontal(|ui| {
                        widgets::legend(ui, pal.mem, "Memory");
                        ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                            ui.label(RichText::new(fmt_bytes(p.mem)).strong().color(pal.mem));
                        });
                    });
                    widgets::sparkline(ui, &pal, &mem, pal.mem, None, vec2(w, 54.0));
                    let (trend, leaking) = {
                        let g = shared.lock().unwrap();
                        (
                            g.leaks.trend(p.pid, p.start_time),
                            g.leak_suspects.iter().any(|l| l.pid == p.pid),
                        )
                    };
                    if let Some(t) = trend {
                        let col = if leaking { pal.warn } else { pal.text_dim };
                        let txt = if leaking {
                            format!("⚠ Possible memory leak: {t:+.0} MB per hour")
                        } else {
                            format!("Memory trend: {t:+.0} MB per hour")
                        };
                        ui.label(RichText::new(txt).size(11.5).color(col));
                    }
                    if !s.gpus.is_empty() {
                        ui.add_space(6.0);
                        ui.horizontal(|ui| {
                            widgets::legend(ui, pal.gpu, "GPU");
                            if !p.gpu_engine.is_empty() {
                                widgets::faint(ui, &pal, &*p.gpu_engine);
                            }
                            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                                ui.label(
                                    RichText::new(format!("{:.1}%", p.gpu))
                                        .strong()
                                        .color(pal.gpu),
                                );
                            });
                        });
                        widgets::sparkline(ui, &pal, &gpu, pal.gpu, Some(100.0), vec2(w, 54.0));
                        widgets::faint(
                            ui,
                            &pal,
                            format!(
                                "GPU memory: {} dedicated · {} shared",
                                fmt_bytes(p.gpu_mem),
                                fmt_bytes(p.gpu_shared)
                            ),
                        );
                    }
                    if s.net_status == Some(crate::native::NetStatus::Running) {
                        ui.add_space(4.0);
                        widgets::faint(
                            ui,
                            &pal,
                            format!(
                                "Network: ⬇ {}  ⬆ {}",
                                fmt_rate(p.net_rx as f32),
                                fmt_rate(p.net_tx as f32)
                            ),
                        );
                    }
                });
                ui.add_space(8.0);

                // Security: signature + VirusTotal
                widgets::card(ui, &pal, |ui| {
                    ui.set_width(ui.available_width());
                    widgets::section_title(ui, &pal, "🛡 Security");
                    match self.sig_for(&p.exe) {
                        Some(sig) if sig.verified() => {
                            ui.label(
                                RichText::new(format!("✔ Signed by {}", sig.signer))
                                    .color(pal.good),
                            );
                        }
                        Some(sig) => {
                            ui.label(RichText::new(format!("⚠ {}", sig.status)).color(pal.warn));
                        }
                        None => {
                            widgets::faint(
                                ui,
                                &pal,
                                "Signature not checked yet — use “🛡 Verify” in the toolbar.",
                            );
                        }
                    }
                    if let Some(h) = self.hashes.get(&p.pid) {
                        ui.horizontal(|ui| {
                            ui.label(
                                RichText::new(format!("SHA-256 {}…", &h[..16]))
                                    .monospace()
                                    .color(pal.text_dim),
                            );
                            if ui.small_button("Copy").clicked() {
                                ui.ctx().copy_text(h.clone());
                            }
                        });
                    }
                    if ui
                        .button("Check on VirusTotal")
                        .on_hover_text(
                            "Looks up this file's SHA-256 hash; the file itself is not uploaded",
                        )
                        .clicked()
                    {
                        actions.push(RowAction::VirusTotal(
                            p.pid,
                            p.name.to_string(),
                            p.exe.to_string(),
                        ));
                    }
                });
                ui.add_space(8.0);

                widgets::card(ui, &pal, |ui| {
                    ui.set_width(ui.available_width());
                    let parent = p.parent.and_then(|pp| s.procs.iter().find(|x| x.pid == pp));
                    let prio = self
                        .priority_cache
                        .get(&p.pid)
                        .cloned()
                        .unwrap_or_else(|| "…".into());
                    egui::Grid::new("pinfo")
                        .num_columns(2)
                        .spacing(vec2(12.0, 6.0))
                        .striped(false)
                        .show(ui, |ui| {
                            let kv = |ui: &mut egui::Ui, k: &str, v: String| {
                                ui.label(RichText::new(k).color(pal.text_dim));
                                ui.add(egui::Label::new(v).truncate());
                                ui.end_row();
                            };
                            kv(
                                ui,
                                "User",
                                if p.user.is_empty() {
                                    "—".into()
                                } else {
                                    p.user.to_string()
                                },
                            );
                            kv(ui, "Priority", prio);
                            kv(ui, "Started", fmt_epoch(p.start_time));
                            kv(ui, "Running for", fmt_dur(p.run_time));
                            kv(ui, "Working set", fmt_bytes(p.mem));
                            kv(ui, "Virtual memory", fmt_bytes(p.vmem));
                            kv(ui, "Disk read", fmt_rate(p.disk_read as f32));
                            kv(ui, "Disk write", fmt_rate(p.disk_write as f32));
                            kv(
                                ui,
                                "Parent",
                                parent
                                    .map(|pp| format!("{} ({})", pp.name, pp.pid))
                                    .unwrap_or_else(|| {
                                        p.parent
                                            .map(|x| format!("PID {x} (exited)"))
                                            .unwrap_or("—".into())
                                    }),
                            );
                        });
                    ui.add_space(6.0);
                    ui.horizontal(|ui| {
                        ui.menu_button("⚡ Priority ⏷", |ui| {
                            for pr in Priority::ALL {
                                if ui.button(pr.label()).clicked() {
                                    actions.push(RowAction::Priority(
                                        p.pid,
                                        p.name.to_string(),
                                        pr,
                                    ));
                                    ui.close_menu();
                                }
                            }
                        });
                        if ui.button("🧮 Affinity…").clicked() {
                            actions.push(RowAction::Affinity(p.pid, p.name.to_string()));
                        }
                    });
                });
                ui.add_space(8.0);

                widgets::card(ui, &pal, |ui| {
                    ui.set_width(ui.available_width());
                    widgets::section_title(ui, &pal, "Path");
                    let mut exe = if p.exe.is_empty() {
                        "Unavailable — run as administrator to see protected processes".to_string()
                    } else {
                        p.exe.to_string()
                    };
                    ui.add(
                        egui::TextEdit::multiline(&mut exe)
                            .desired_rows(1)
                            .desired_width(f32::INFINITY)
                            .font(egui::TextStyle::Monospace)
                            .interactive(true),
                    );
                    ui.add_space(4.0);
                    widgets::section_title(ui, &pal, "Command line");
                    let mut cmd = if p.cmd.is_empty() {
                        "—".to_string()
                    } else {
                        p.cmd.to_string()
                    };
                    ui.add(
                        egui::TextEdit::multiline(&mut cmd)
                            .desired_rows(2)
                            .desired_width(f32::INFINITY)
                            .font(egui::TextStyle::Monospace),
                    );
                });

                // Threads
                widgets::card(ui, &pal, |ui| {
                    ui.set_width(ui.available_width());
                    let loaded = self
                        .threads
                        .as_ref()
                        .filter(|(pid, _)| *pid == p.pid)
                        .map(|(_, t)| t.clone());
                    ui.horizontal(|ui| {
                        widgets::section_title(
                            ui,
                            &pal,
                            &match &loaded {
                                Some(t) => format!("☰ Threads ({})", t.len()),
                                None => "☰ Threads".to_string(),
                            },
                        );
                        ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                            let label = if loaded.is_some() {
                                "⟳"
                            } else {
                                "Show threads"
                            };
                            if ui.small_button(label).clicked() {
                                actions.push(RowAction::LoadThreads(p.pid));
                            }
                            if self.threads_rx.is_some() {
                                ui.spinner();
                            }
                        });
                    });
                    if let Some(threads) = loaded {
                        egui::Grid::new("threads")
                            .num_columns(5)
                            .striped(true)
                            .spacing(vec2(10.0, 3.0))
                            .show(ui, |ui| {
                                for h in ["TID", "Name", "Pri", "CPU time", ""] {
                                    ui.label(RichText::new(h).size(11.0).color(pal.text_dim));
                                }
                                ui.end_row();
                                for t in threads.iter().take(60) {
                                    ui.label(RichText::new(t.tid.to_string()).monospace());
                                    ui.add(
                                        egui::Label::new(
                                            RichText::new(&*t.name).color(pal.text_dim),
                                        )
                                        .truncate(),
                                    );
                                    ui.label(t.base_priority.to_string());
                                    ui.label(fmt_dur(t.cpu_ms / 1000));
                                    ui.menu_button("⋯", |ui| {
                                        use crate::native::ThreadAction as TA;
                                        for (l, a) in [
                                            ("⏸ Suspend thread", TA::Suspend),
                                            ("▶ Resume thread", TA::Resume),
                                            ("✖ End thread", TA::Kill),
                                        ] {
                                            if ui.button(l).clicked() {
                                                actions.push(RowAction::Thread(p.pid, t.tid, a));
                                                ui.close_menu();
                                            }
                                        }
                                    });
                                    ui.end_row();
                                }
                            });
                        if threads.len() > 60 {
                            widgets::faint(
                                ui,
                                &pal,
                                format!("…and {} more (sorted by CPU time)", threads.len() - 60),
                            );
                        }
                    }
                });

                // Child processes
                let kids: Vec<&ProcInfo> = s
                    .procs
                    .iter()
                    .filter(|x| x.parent == Some(p.pid) && x.pid != p.pid)
                    .collect();
                if !kids.is_empty() {
                    ui.add_space(8.0);
                    widgets::card(ui, &pal, |ui| {
                        ui.set_width(ui.available_width());
                        widgets::section_title(
                            ui,
                            &pal,
                            &format!("Child processes ({})", kids.len()),
                        );
                        for k in kids.iter().take(30) {
                            ui.horizontal(|ui| {
                                widgets::avatar(ui, &k.name, pal.dark);
                                if ui.link(&*k.name).clicked() {
                                    actions.push(RowAction::Select(k.pid));
                                }
                                ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                                    ui.label(RichText::new(fmt_bytes(k.mem)).color(pal.text_dim));
                                });
                            });
                        }
                    });
                }

                // Services hosted by this process (if the services list is loaded)
                let svcs: Vec<String> = self
                    .services
                    .data
                    .iter()
                    .filter(|x| x.pid == p.pid && x.pid != 0)
                    .map(|x| x.display.clone())
                    .collect();
                if !svcs.is_empty() {
                    ui.add_space(8.0);
                    widgets::card(ui, &pal, |ui| {
                        ui.set_width(ui.available_width());
                        widgets::section_title(
                            ui,
                            &pal,
                            &format!("Hosted services ({})", svcs.len()),
                        );
                        for sv in svcs.iter().take(30) {
                            ui.label(RichText::new(format!("⚙ {sv}")).color(pal.text_dim));
                        }
                    });
                }
            });
        for a in actions {
            match a {
                RowAction::Select(pid) => {
                    self.reveal_process(pid);
                    self.fetch_priority(pid);
                }
                other => self.apply_row_action(other),
            }
        }
    }
}

pub enum RowAction {
    Kill(u32, String, bool),
    KillAll(String),
    Priority(u32, String, Priority),
    Affinity(u32, String),
    OpenLocation(String),
    SearchOnline(String),
    TogglePin(String),
    ShowDetails,
    Select(u32),
    Suspend(u32, String, bool),
    Restart(u32),
    VirusTotal(u32, String, String),
    Dump(u32, String),
    LoadThreads(u32),
    Thread(u32, u32, crate::native::ThreadAction),
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn case_insensitive_helpers() {
        assert!(contains_ci("Chrome.EXE", "chrome"));
        assert!(contains_ci(
            "C:\\Program Files\\App\\app.exe",
            "program files"
        ));
        assert!(!contains_ci("abc", "abcd"));
        assert!(contains_ci("Überwachung.exe", "überwachung"));
        assert_eq!(cmp_ci("apple", "Banana"), std::cmp::Ordering::Less);
        assert_eq!(cmp_ci("ZED", "zed"), std::cmp::Ordering::Equal);
    }

    #[test]
    fn search_terms_all_must_match() {
        let p = ProcInfo {
            pid: 4242,
            name: "Code.exe".into(),
            cmd: "code --type=renderer".into(),
            ..Default::default()
        };
        assert!(matches(&p, &["code".into(), "renderer".into()]));
        assert!(matches(&p, &["4242".into()]));
        assert!(!matches(&p, &["code".into(), "gpu-process".into()]));
    }
}
