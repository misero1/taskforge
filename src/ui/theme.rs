//! Colour palettes and egui style setup.

use eframe::egui::{self, Color32, CornerRadius, FontFamily, FontId, Stroke, TextStyle, Visuals};

pub const ACCENTS: [(&str, Color32); 6] = [
    ("Azure", Color32::from_rgb(76, 141, 255)),
    ("Teal", Color32::from_rgb(32, 201, 176)),
    ("Violet", Color32::from_rgb(150, 110, 255)),
    ("Rose", Color32::from_rgb(244, 91, 134)),
    ("Amber", Color32::from_rgb(245, 166, 35)),
    ("Lime", Color32::from_rgb(120, 200, 80)),
];

#[derive(Clone, Copy)]
pub struct Palette {
    pub dark: bool,
    pub accent: Color32,
    pub bg: Color32,
    pub sidebar: Color32,
    pub card: Color32,
    pub graph_bg: Color32,
    pub border: Color32,
    pub grid: Color32,
    pub track: Color32,
    pub text: Color32,
    pub text_dim: Color32,
    pub text_faint: Color32,
    pub cpu: Color32,
    pub mem: Color32,
    pub disk: Color32,
    pub net_rx: Color32,
    pub net_tx: Color32,
    pub gpu: Color32,
    pub good: Color32,
    pub warn: Color32,
    pub bad: Color32,
}

impl Palette {
    pub fn new(dark: bool, accent_idx: usize) -> Self {
        let accent = ACCENTS[accent_idx % ACCENTS.len()].1;
        if dark {
            Palette {
                dark,
                accent,
                bg: Color32::from_rgb(15, 17, 22),
                sidebar: Color32::from_rgb(19, 22, 28),
                card: Color32::from_rgb(24, 27, 34),
                graph_bg: Color32::from_rgb(18, 21, 27),
                border: Color32::from_rgb(38, 43, 53),
                grid: Color32::from_rgb(32, 36, 45),
                track: Color32::from_rgb(40, 45, 56),
                text: Color32::from_rgb(232, 235, 241),
                text_dim: Color32::from_rgb(160, 167, 180),
                text_faint: Color32::from_rgb(105, 112, 126),
                cpu: Color32::from_rgb(76, 141, 255),
                mem: Color32::from_rgb(170, 120, 255),
                disk: Color32::from_rgb(32, 201, 150),
                net_rx: Color32::from_rgb(245, 166, 35),
                net_tx: Color32::from_rgb(244, 91, 134),
                gpu: Color32::from_rgb(140, 214, 80),
                good: Color32::from_rgb(70, 200, 120),
                warn: Color32::from_rgb(245, 180, 50),
                bad: Color32::from_rgb(240, 85, 85),
            }
        } else {
            Palette {
                dark,
                accent,
                bg: Color32::from_rgb(244, 246, 250),
                sidebar: Color32::from_rgb(236, 239, 245),
                card: Color32::WHITE,
                graph_bg: Color32::from_rgb(248, 249, 252),
                border: Color32::from_rgb(220, 225, 233),
                grid: Color32::from_rgb(232, 236, 242),
                track: Color32::from_rgb(226, 230, 238),
                text: Color32::from_rgb(24, 28, 36),
                text_dim: Color32::from_rgb(90, 98, 112),
                text_faint: Color32::from_rgb(140, 148, 162),
                cpu: Color32::from_rgb(40, 110, 235),
                mem: Color32::from_rgb(130, 80, 220),
                disk: Color32::from_rgb(10, 160, 115),
                net_rx: Color32::from_rgb(220, 135, 10),
                net_tx: Color32::from_rgb(220, 60, 110),
                gpu: Color32::from_rgb(70, 150, 30),
                good: Color32::from_rgb(30, 160, 90),
                warn: Color32::from_rgb(210, 140, 20),
                bad: Color32::from_rgb(215, 50, 50),
            }
        }
    }

    /// Colour for a load fraction: green → amber → red.
    pub fn load(&self, frac: f32) -> Color32 {
        if frac < 0.6 {
            self.good
        } else if frac < 0.85 {
            self.warn
        } else {
            self.bad
        }
    }
}

pub fn apply(ctx: &egui::Context, pal: &Palette) {
    let mut v = if pal.dark {
        Visuals::dark()
    } else {
        Visuals::light()
    };
    v.panel_fill = pal.bg;
    v.window_fill = pal.card;
    v.extreme_bg_color = pal.graph_bg;
    v.faint_bg_color = if pal.dark {
        Color32::from_rgb(21, 24, 30)
    } else {
        Color32::from_rgb(247, 248, 251)
    };
    v.window_stroke = Stroke::new(1.0f32, pal.border);
    v.window_corner_radius = CornerRadius::same(12);
    v.menu_corner_radius = CornerRadius::same(10);
    v.override_text_color = Some(pal.text);
    v.selection.bg_fill =
        super::widgets::with_alpha(pal.accent, if pal.dark { 0.32 } else { 0.22 });
    v.selection.stroke = Stroke::new(1.0f32, pal.accent);
    v.hyperlink_color = pal.accent;
    v.striped = true;

    let r = CornerRadius::same(7);
    for w in [
        &mut v.widgets.noninteractive,
        &mut v.widgets.inactive,
        &mut v.widgets.hovered,
        &mut v.widgets.active,
        &mut v.widgets.open,
    ] {
        w.corner_radius = r;
    }
    v.widgets.noninteractive.bg_stroke = Stroke::new(1.0f32, pal.border);
    v.widgets.inactive.weak_bg_fill = if pal.dark {
        Color32::from_rgb(32, 36, 45)
    } else {
        Color32::from_rgb(233, 236, 242)
    };
    v.widgets.inactive.bg_fill = v.widgets.inactive.weak_bg_fill;
    v.widgets.hovered.weak_bg_fill = if pal.dark {
        Color32::from_rgb(42, 47, 58)
    } else {
        Color32::from_rgb(222, 227, 236)
    };
    v.widgets.hovered.bg_stroke = Stroke::new(1.0f32, super::widgets::with_alpha(pal.accent, 0.6));
    v.widgets.active.weak_bg_fill = super::widgets::with_alpha(pal.accent, 0.35);
    // Pin egui's theme so an OS light/dark switch never swaps our styling out.
    ctx.set_theme(if pal.dark {
        egui::Theme::Dark
    } else {
        egui::Theme::Light
    });
    ctx.set_visuals(v);

    ctx.all_styles_mut(|s| {
        s.spacing.item_spacing = egui::vec2(8.0, 6.0);
        s.spacing.button_padding = egui::vec2(10.0, 5.0);
        s.spacing.interact_size.y = 26.0;
        s.text_styles = [
            (
                TextStyle::Small,
                FontId::new(11.0, FontFamily::Proportional),
            ),
            (TextStyle::Body, FontId::new(13.5, FontFamily::Proportional)),
            (
                TextStyle::Button,
                FontId::new(13.5, FontFamily::Proportional),
            ),
            (
                TextStyle::Heading,
                FontId::new(20.0, FontFamily::Proportional),
            ),
            (
                TextStyle::Monospace,
                FontId::new(12.5, FontFamily::Monospace),
            ),
        ]
        .into();
    });
}

/// Primary (filled accent) button.
pub fn primary_button(ui: &mut egui::Ui, pal: &Palette, text: &str) -> egui::Response {
    ui.add(
        egui::Button::new(egui::RichText::new(text).color(Color32::WHITE).strong())
            .fill(pal.accent)
            .corner_radius(CornerRadius::same(7)),
    )
}

/// Destructive (red) button.
pub fn danger_button(
    ui: &mut egui::Ui,
    pal: &Palette,
    text: &str,
    enabled: bool,
) -> egui::Response {
    ui.add_enabled(
        enabled,
        egui::Button::new(egui::RichText::new(text).color(Color32::WHITE).strong())
            .fill(pal.bad)
            .corner_radius(CornerRadius::same(7)),
    )
}
