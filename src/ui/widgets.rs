//! Custom drawing: themed graphs, gauges, cards and formatting helpers.

use std::collections::VecDeque;

use eframe::egui::{
    self, epaint::PathStroke, pos2, vec2, Align2, Color32, CornerRadius, FontId, Mesh, Pos2,
    Response, Sense, Shape, Stroke, StrokeKind, Ui,
};

use super::theme::Palette;

// ───────────────────────────── Formatting ─────────────────────────────

pub fn fmt_bytes(b: u64) -> String {
    const U: [&str; 6] = ["B", "KB", "MB", "GB", "TB", "PB"];
    let mut v = b as f64;
    let mut i = 0;
    while v >= 1024.0 && i < U.len() - 1 {
        v /= 1024.0;
        i += 1;
    }
    if i == 0 {
        format!("{b} B")
    } else if v >= 100.0 {
        format!("{v:.0} {}", U[i])
    } else {
        format!("{v:.1} {}", U[i])
    }
}

pub fn fmt_rate(b: f32) -> String {
    if b < 1.0 {
        "0 B/s".into()
    } else {
        format!("{}/s", fmt_bytes(b as u64))
    }
}

pub fn fmt_bits(bytes_per_sec: f32) -> String {
    let bits = bytes_per_sec as f64 * 8.0;
    if bits >= 1e9 {
        format!("{:.1} Gbps", bits / 1e9)
    } else if bits >= 1e6 {
        format!("{:.1} Mbps", bits / 1e6)
    } else if bits >= 1e3 {
        format!("{:.0} Kbps", bits / 1e3)
    } else {
        format!("{bits:.0} bps")
    }
}

pub fn fmt_dur(secs: u64) -> String {
    let (d, h, m, s) = (secs / 86400, secs / 3600 % 24, secs / 60 % 60, secs % 60);
    if d > 0 {
        format!("{d}d {h}h {m}m")
    } else if h > 0 {
        format!("{h}h {m}m")
    } else if m > 0 {
        format!("{m}m {s}s")
    } else {
        format!("{s}s")
    }
}

pub fn fmt_epoch(secs: u64) -> String {
    use chrono::TimeZone;
    chrono::Local
        .timestamp_opt(secs as i64, 0)
        .single()
        .map(|t| t.format("%Y-%m-%d %H:%M:%S").to_string())
        .unwrap_or_default()
}

// ───────────────────────────── Colors ─────────────────────────────

/// Task-Manager-style heat background for a 0..1 load value.
pub fn heat(v: f32, pal: &Palette) -> Color32 {
    let v = v.clamp(0.0, 1.0);
    if v < 0.005 {
        return Color32::TRANSPARENT;
    }
    // Amber → orange → red as load climbs.
    let (r, g, b) = if v < 0.5 {
        let t = v / 0.5;
        (255.0, 200.0 - 40.0 * t, 60.0)
    } else {
        let t = (v - 0.5) / 0.5;
        (255.0, 160.0 - 100.0 * t, 60.0 - 20.0 * t)
    };
    let a = if pal.dark {
        22.0 + 160.0 * v.powf(0.75)
    } else {
        30.0 + 170.0 * v.powf(0.75)
    };
    Color32::from_rgba_unmultiplied(r as u8, g as u8, b as u8, a as u8)
}

/// Stable, pleasant colour derived from a string (for process avatars).
pub fn hash_color(s: &str, dark: bool) -> Color32 {
    let mut h: u32 = 2166136261;
    for b in s.to_lowercase().bytes() {
        h ^= b as u32;
        h = h.wrapping_mul(16777619);
    }
    let hue = (h % 360) as f32 / 360.0;
    let c = egui::ecolor::Hsva::new(hue, 0.55, if dark { 0.80 } else { 0.70 }, 1.0);
    c.into()
}

pub fn with_alpha(c: Color32, a: f32) -> Color32 {
    let [r, g, b, _] = c.to_srgba_unmultiplied();
    Color32::from_rgba_unmultiplied(r, g, b, (a.clamp(0.0, 1.0) * 255.0) as u8)
}

// ───────────────────────────── Graphs ─────────────────────────────

pub struct Series<'a> {
    pub data: &'a VecDeque<f32>,
    pub color: Color32,
    pub label: &'a str,
}

pub struct GraphOpts<'a> {
    pub height: f32,
    /// Fixed y-maximum; `None` = auto-scale to the visible data.
    pub max: Option<f32>,
    /// Number of samples visible across the width.
    pub window: usize,
    pub grid: bool,
    pub fmt: &'a dyn Fn(f32) -> String,
    pub interval_ms: u64,
}

/// Area graph with gradient-like fill, grid, auto-scaling and hover read-out.
pub fn graph(ui: &mut Ui, pal: &Palette, series: &[Series], o: GraphOpts) -> Response {
    let width = ui.available_width().max(60.0);
    let (rect, resp) = ui.allocate_exact_size(vec2(width, o.height), Sense::hover());
    if !ui.is_rect_visible(rect) {
        return resp;
    }
    let p = ui.painter_at(rect.expand(1.0));
    p.rect_filled(rect, CornerRadius::same(8), pal.graph_bg);

    let window = o.window.max(2);
    let max = o
        .max
        .unwrap_or_else(|| {
            series
                .iter()
                .flat_map(|s| s.data.iter().rev().take(window))
                .fold(0.0f32, |a, &b| a.max(b))
                .max(1.0)
                * 1.15
        })
        .max(f32::EPSILON);

    let inner = rect.shrink2(vec2(0.0, 2.0));
    if o.grid {
        for i in 1..4 {
            let y = inner.top() + inner.height() * i as f32 / 4.0;
            p.line_segment(
                [pos2(rect.left(), y), pos2(rect.right(), y)],
                Stroke::new(1.0f32, pal.grid),
            );
        }
        for i in 1..6 {
            let x = rect.left() + rect.width() * i as f32 / 6.0;
            p.line_segment(
                [pos2(x, rect.top()), pos2(x, rect.bottom())],
                Stroke::new(1.0f32, pal.grid),
            );
        }
    }

    let step = inner.width() / (window - 1) as f32;
    let to_pos = |i: usize, n: usize, v: f32| -> Pos2 {
        let x = inner.right() - (n - 1 - i) as f32 * step;
        let y = inner.bottom() - (v / max).clamp(0.0, 1.0) * inner.height();
        pos2(x, y)
    };

    for s in series {
        let n = s.data.len().min(window);
        if n < 2 {
            continue;
        }
        let start = s.data.len() - n;
        // Never draw more than ~1 point per pixel: when there are more samples
        // than pixels, keep each bucket's peak so spikes stay visible.
        let max_pts = (inner.width().max(2.0) as usize).max(2);
        let bucket = n.div_ceil(max_pts).max(1);
        let mut pts: Vec<Pos2> = Vec::with_capacity(n / bucket + 1);
        let mut i = 0;
        while i < n {
            let end = (i + bucket).min(n);
            let peak = (i..end).map(|k| s.data[start + k]).fold(f32::MIN, f32::max);
            pts.push(to_pos(end - 1, n, peak));
            i = end;
        }
        if pts.len() < 2 {
            continue;
        }

        // Filled area: a vertical gradient from the line down to the baseline.
        let mut mesh = Mesh::default();
        let top_c = with_alpha(s.color, if pal.dark { 0.35 } else { 0.28 });
        let bot_c = with_alpha(s.color, 0.03);
        for pt in &pts {
            mesh.colored_vertex(*pt, top_c);
            mesh.colored_vertex(pos2(pt.x, inner.bottom()), bot_c);
        }
        for i in 0..(pts.len() as u32 - 1) {
            let a = i * 2;
            mesh.add_triangle(a, a + 1, a + 2);
            mesh.add_triangle(a + 1, a + 3, a + 2);
        }
        p.add(Shape::mesh(mesh));
        p.add(Shape::line(pts, PathStroke::new(1.6f32, s.color)));
    }

    p.rect_stroke(
        rect,
        CornerRadius::same(8),
        Stroke::new(1.0f32, pal.border),
        StrokeKind::Inside,
    );

    if o.grid {
        p.text(
            rect.left_top() + vec2(6.0, 4.0),
            Align2::LEFT_TOP,
            (o.fmt)(max),
            FontId::proportional(10.0),
            pal.text_faint,
        );
        let secs = (window as u64 * o.interval_ms) / 1000;
        p.text(
            rect.left_bottom() + vec2(6.0, -4.0),
            Align2::LEFT_BOTTOM,
            format!("{} ago", fmt_dur(secs)),
            FontId::proportional(10.0),
            pal.text_faint,
        );
    }

    // Hover crosshair + value read-out.
    if let Some(hp) = resp.hover_pos() {
        let back = ((inner.right() - hp.x) / step).round().max(0.0) as usize;
        p.line_segment(
            [pos2(hp.x, rect.top()), pos2(hp.x, rect.bottom())],
            Stroke::new(1.0f32, pal.text_faint),
        );
        let mut lines = Vec::new();
        for s in series {
            if back < s.data.len() {
                let v = s.data[s.data.len() - 1 - back];
                lines.push(if s.label.is_empty() {
                    (o.fmt)(v)
                } else {
                    format!("{}: {}", s.label, (o.fmt)(v))
                });
                let pt = to_pos(window - 1 - back.min(window - 1), window, v);
                p.circle_filled(pos2(hp.x, pt.y), 3.0, s.color);
            }
        }
        if !lines.is_empty() {
            let secs = back as u64 * o.interval_ms / 1000;
            lines.push(if secs == 0 {
                "now".into()
            } else {
                format!("{} ago", fmt_dur(secs))
            });
            resp.clone().on_hover_text(lines.join("\n"));
        }
    }
    resp
}

/// Tiny graph for cards and table rows.
pub fn sparkline(
    ui: &mut Ui,
    pal: &Palette,
    data: &VecDeque<f32>,
    color: Color32,
    max: Option<f32>,
    size: egui::Vec2,
) {
    ui.allocate_ui(size, |ui| {
        graph(
            ui,
            pal,
            &[Series {
                data,
                color,
                label: "",
            }],
            GraphOpts {
                height: size.y,
                max,
                window: 60,
                grid: false,
                fmt: &|v| format!("{v:.1}"),
                interval_ms: 1000,
            },
        );
    });
}

/// Circular gauge used on the overview page.
pub fn ring(
    ui: &mut Ui,
    pal: &Palette,
    frac: f32,
    color: Color32,
    center_text: &str,
    sub: &str,
    size: f32,
) -> Response {
    let (rect, resp) = ui.allocate_exact_size(vec2(size, size), Sense::hover());
    let p = ui.painter();
    let c = rect.center();
    let r = size * 0.5 - 7.0;
    let thick = (size * 0.085).max(6.0);
    let arc = |from: f32, to: f32| -> Vec<Pos2> {
        let n = 64;
        (0..=n)
            .map(|i| {
                let t = from + (to - from) * i as f32 / n as f32;
                let a = -std::f32::consts::FRAC_PI_2 + t * std::f32::consts::TAU;
                c + vec2(a.cos(), a.sin()) * r
            })
            .collect()
    };
    p.add(Shape::line(
        arc(0.0, 1.0),
        PathStroke::new(thick, pal.track),
    ));
    let f = frac.clamp(0.0, 1.0);
    if f > 0.002 {
        p.add(Shape::line(arc(0.0, f), PathStroke::new(thick, color)));
        // Rounded end caps.
        let pts = arc(0.0, f);
        p.circle_filled(pts[0], thick / 2.0, color);
        p.circle_filled(*pts.last().unwrap(), thick / 2.0, color);
    }
    p.text(
        c - vec2(0.0, 6.0),
        Align2::CENTER_CENTER,
        center_text,
        FontId::proportional(size * 0.19),
        pal.text,
    );
    p.text(
        c + vec2(0.0, size * 0.13),
        Align2::CENTER_CENTER,
        sub,
        FontId::proportional(11.0),
        pal.text_dim,
    );
    resp
}

/// Horizontal usage bar (disks, per-core lists).
pub fn bar(ui: &mut Ui, pal: &Palette, frac: f32, color: Color32, height: f32) {
    let w = ui.available_width();
    let (rect, _) = ui.allocate_exact_size(vec2(w, height), Sense::hover());
    let p = ui.painter();
    let rr = CornerRadius::same((height / 2.0) as u8);
    p.rect_filled(rect, rr, pal.track);
    let mut fill = rect;
    fill.set_width(rect.width() * frac.clamp(0.0, 1.0));
    if fill.width() > 1.0 {
        p.rect_filled(fill, rr, color);
    }
}

/// Rounded card container.
pub fn card<R>(ui: &mut Ui, pal: &Palette, add: impl FnOnce(&mut Ui) -> R) -> R {
    egui::Frame::new()
        .fill(pal.card)
        .stroke(Stroke::new(1.0f32, pal.border))
        .corner_radius(CornerRadius::same(12))
        .inner_margin(egui::Margin::same(14))
        .show(ui, |ui| ui.vertical(add).inner)
        .inner
}

/// Coloured initial badge used as a process "icon".
pub fn avatar(ui: &mut Ui, name: &str, dark: bool) {
    let (rect, _) = ui.allocate_exact_size(vec2(18.0, 18.0), Sense::hover());
    let col = hash_color(name, dark);
    ui.painter().rect_filled(rect, CornerRadius::same(5), col);
    let ch = name
        .chars()
        .find(|c| c.is_alphanumeric())
        .unwrap_or('?')
        .to_ascii_uppercase();
    ui.painter().text(
        rect.center(),
        Align2::CENTER_CENTER,
        ch,
        FontId::proportional(11.0),
        Color32::WHITE,
    );
}

/// Small coloured pill label.
pub fn pill(ui: &mut Ui, text: &str, color: Color32) -> Response {
    let galley = ui
        .painter()
        .layout_no_wrap(text.to_string(), FontId::proportional(11.0), color);
    let size = galley.size() + vec2(14.0, 6.0);
    let (rect, resp) = ui.allocate_exact_size(size, Sense::hover());
    ui.painter()
        .rect_filled(rect, CornerRadius::same(255), with_alpha(color, 0.16));
    ui.painter()
        .galley(rect.center() - galley.size() / 2.0, galley, color);
    resp
}

/// Legend dot + label.
pub fn legend(ui: &mut Ui, color: Color32, text: &str) {
    let (r, _) = ui.allocate_exact_size(vec2(10.0, 10.0), Sense::hover());
    ui.painter().circle_filled(r.center(), 4.0, color);
    ui.label(text);
}

pub fn section_title(ui: &mut Ui, pal: &Palette, text: &str) {
    ui.label(
        egui::RichText::new(text)
            .size(13.0)
            .strong()
            .color(pal.text),
    );
}

pub fn faint(ui: &mut Ui, pal: &Palette, text: impl Into<String>) -> Response {
    ui.label(
        egui::RichText::new(text.into())
            .size(11.5)
            .color(pal.text_dim),
    )
}

/// Empty-state placeholder.
pub fn empty_state(ui: &mut Ui, pal: &Palette, icon: &str, title: &str, sub: &str) {
    ui.vertical_centered(|ui| {
        ui.add_space(40.0);
        ui.label(egui::RichText::new(icon).size(34.0).color(pal.text_faint));
        ui.add_space(6.0);
        ui.label(
            egui::RichText::new(title)
                .size(15.0)
                .strong()
                .color(pal.text_dim),
        );
        ui.label(egui::RichText::new(sub).color(pal.text_faint));
    });
}

/// Time-based line graph for long-term history. `pts` are (unix seconds, value)
/// in time order; gaps (app closed) are left empty. Hovering shows the time,
/// the value and `detail(i)` for the nearest point.
#[allow(clippy::too_many_arguments)]
pub fn timeline(
    ui: &mut Ui,
    pal: &Palette,
    pts: &[(i64, f32)],
    t0: i64,
    t1: i64,
    color: Color32,
    max: Option<f32>,
    height: f32,
    fmt: &dyn Fn(f32) -> String,
    detail: &dyn Fn(usize) -> String,
) -> Response {
    let width = ui.available_width().max(60.0);
    let (rect, resp) = ui.allocate_exact_size(vec2(width, height), Sense::hover());
    if !ui.is_rect_visible(rect) {
        return resp;
    }
    let p = ui.painter_at(rect.expand(1.0));
    p.rect_filled(rect, CornerRadius::same(8), pal.graph_bg);
    for i in 1..4 {
        let y = rect.top() + rect.height() * i as f32 / 4.0;
        p.line_segment(
            [pos2(rect.left(), y), pos2(rect.right(), y)],
            Stroke::new(1.0f32, pal.grid),
        );
    }
    let span = (t1 - t0).max(1) as f32;
    let max = max
        .unwrap_or_else(|| pts.iter().map(|x| x.1).fold(0.0, f32::max).max(1.0) * 1.15)
        .max(f32::EPSILON);
    let inner = rect.shrink2(vec2(0.0, 2.0));
    let to_pos = |t: i64, v: f32| {
        pos2(
            inner.left() + (t - t0) as f32 / span * inner.width(),
            inner.bottom() - (v / max).clamp(0.0, 1.0) * inner.height(),
        )
    };
    // Split into runs where consecutive points are close in time.
    let gap = ((t1 - t0) / 150).max(30);
    let mut run: Vec<Pos2> = Vec::new();
    let flush = |run: &mut Vec<Pos2>| {
        if run.len() >= 2 {
            let mut mesh = Mesh::default();
            let top_c = with_alpha(color, if pal.dark { 0.32 } else { 0.25 });
            let bot_c = with_alpha(color, 0.03);
            for pt in run.iter() {
                mesh.colored_vertex(*pt, top_c);
                mesh.colored_vertex(pos2(pt.x, inner.bottom()), bot_c);
            }
            for i in 0..(run.len() as u32 - 1) {
                let a = i * 2;
                mesh.add_triangle(a, a + 1, a + 2);
                mesh.add_triangle(a + 1, a + 3, a + 2);
            }
            p.add(Shape::mesh(mesh));
            p.add(Shape::line(run.clone(), PathStroke::new(1.4f32, color)));
        }
        run.clear();
    };
    let mut last_t: Option<i64> = None;
    for &(t, v) in pts {
        if last_t.is_some_and(|lt| t - lt > gap) {
            flush(&mut run);
        }
        let pos = to_pos(t, v);
        // Merge points that land on the same pixel column, keeping the peak.
        match run.last_mut() {
            Some(prev) if (pos.x - prev.x).abs() < 1.0 => prev.y = prev.y.min(pos.y),
            _ => run.push(pos),
        }
        last_t = Some(t);
    }
    flush(&mut run);
    p.rect_stroke(
        rect,
        CornerRadius::same(8),
        Stroke::new(1.0f32, pal.border),
        StrokeKind::Inside,
    );
    p.text(
        rect.left_top() + vec2(6.0, 4.0),
        Align2::LEFT_TOP,
        fmt(max),
        FontId::proportional(10.0),
        pal.text_faint,
    );

    if let Some(hp) = resp.hover_pos() {
        let t = t0 + ((hp.x - inner.left()) / inner.width() * span) as i64;
        if let Some((i, &(pt, v))) = pts.iter().enumerate().min_by_key(|(_, x)| (x.0 - t).abs()) {
            if (pt - t).abs() <= gap {
                let pos = to_pos(pt, v);
                p.line_segment(
                    [pos2(pos.x, rect.top()), pos2(pos.x, rect.bottom())],
                    Stroke::new(1.0f32, pal.text_faint),
                );
                p.circle_filled(pos, 3.5, color);
                let when = fmt_epoch(pt as u64);
                let extra = detail(i);
                resp.clone().on_hover_text(if extra.is_empty() {
                    format!("{when}\n{}", fmt(v))
                } else {
                    format!("{when}\n{}\n{extra}", fmt(v))
                });
            }
        }
    }
    resp
}
