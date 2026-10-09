//! Studio design tokens (docs/STUDIO_UI.md §6) applied to egui's style.

use egui::{Color32, CornerRadius, FontFamily, FontId, Stroke, TextStyle, Theme, Vec2};
use astrum_app::studio::view::Tone;

pub const SURFACE_0: Color32 = Color32::from_rgb(0x0b, 0x0e, 0x13);
pub const SURFACE_1: Color32 = Color32::from_rgb(0x12, 0x16, 0x1d);
pub const SURFACE_2: Color32 = Color32::from_rgb(0x1a, 0x1f, 0x28);
pub const SURFACE_3: Color32 = Color32::from_rgb(0x23, 0x2a, 0x35);
pub const BORDER: Color32 = Color32::from_rgb(0x2a, 0x31, 0x3d);
pub const BORDER_STRONG: Color32 = Color32::from_rgb(0x3a, 0x43, 0x52);
pub const TEXT_PRIMARY: Color32 = Color32::from_rgb(0xe6, 0xe9, 0xef);
pub const TEXT_SECONDARY: Color32 = Color32::from_rgb(0x9a, 0xa3, 0xb2);
pub const TEXT_DISABLED: Color32 = Color32::from_rgb(0x5d, 0x65, 0x74);
pub const ACCENT: Color32 = Color32::from_rgb(0x5b, 0x8c, 0xff);
pub const ACCENT_SOFT: Color32 = Color32::from_rgba_premultiplied(0x12, 0x1c, 0x33, 0x33);
pub const OK: Color32 = Color32::from_rgb(0x4c, 0xc3, 0x8a);
pub const WARN: Color32 = Color32::from_rgb(0xf2, 0xb8, 0x4b);
pub const ERROR: Color32 = Color32::from_rgb(0xf2, 0x64, 0x5c);
/// Strip and timeline series: interval, engine CPU, GPU.
pub const SERIES_INTERVAL: Color32 = Color32::from_rgb(0x5b, 0x8c, 0xff);
pub const SERIES_CPU: Color32 = Color32::from_rgb(0x4c, 0xc3, 0x8a);
pub const SERIES_GPU: Color32 = Color32::from_rgb(0xc5, 0x8a, 0xf9);
/// Stable span colours, indexed by `profiler_view::color_slot`.
pub const SPAN_COLORS: [Color32; 8] = [
    Color32::from_rgb(0x5b, 0x8c, 0xff),
    Color32::from_rgb(0x4c, 0xc3, 0x8a),
    Color32::from_rgb(0xf2, 0xb8, 0x4b),
    Color32::from_rgb(0xc5, 0x8a, 0xf9),
    Color32::from_rgb(0x63, 0xb3, 0xed),
    Color32::from_rgb(0xf2, 0x8c, 0x6b),
    Color32::from_rgb(0x9b, 0xd1, 0x6b),
    Color32::from_rgb(0xe0, 0x7a, 0xb8),
];

pub const SPACE_1: f32 = 4.0;
pub const SPACE_2: f32 = 8.0;
pub const SPACE_3: f32 = 12.0;
pub const RADIUS_SMALL: u8 = 4;
pub const RADIUS: u8 = 6;
pub const FONT_SMALL: f32 = 11.0;
pub const FONT_UI: f32 = 13.0;
pub const FONT_HEADING: f32 = 15.0;
pub const FONT_MONO: f32 = 12.0;

pub fn tone_color(tone: Tone) -> Color32 {
    match tone {
        Tone::Normal => TEXT_PRIMARY,
        Tone::Ok => OK,
        Tone::Warn => WARN,
        Tone::Error => ERROR,
    }
}

pub fn small() -> FontId {
    FontId::proportional(FONT_SMALL)
}

pub fn mono() -> FontId {
    FontId::monospace(FONT_MONO)
}

/// Dark Studio theme for every widget.
pub fn apply(ctx: &egui::Context) {
    ctx.set_theme(Theme::Dark);
    ctx.style_mut_of(Theme::Dark, |style| {
        style.text_styles = [
            (
                TextStyle::Small,
                FontId::new(FONT_SMALL, FontFamily::Proportional),
            ),
            (
                TextStyle::Body,
                FontId::new(FONT_UI, FontFamily::Proportional),
            ),
            (
                TextStyle::Button,
                FontId::new(FONT_UI, FontFamily::Proportional),
            ),
            (
                TextStyle::Heading,
                FontId::new(FONT_HEADING, FontFamily::Proportional),
            ),
            (
                TextStyle::Monospace,
                FontId::new(FONT_MONO, FontFamily::Monospace),
            ),
        ]
        .into();
        style.spacing.item_spacing = Vec2::new(SPACE_2, SPACE_1);
        style.spacing.button_padding = Vec2::new(SPACE_2, 3.0);
        style.spacing.interact_size.y = 24.0;
        let visuals = &mut style.visuals;
        visuals.dark_mode = true;
        visuals.override_text_color = None;
        visuals.panel_fill = SURFACE_1;
        visuals.window_fill = SURFACE_1;
        visuals.extreme_bg_color = SURFACE_0;
        visuals.faint_bg_color = SURFACE_2;
        visuals.code_bg_color = SURFACE_2;
        visuals.window_stroke = Stroke::new(1.0, BORDER);
        visuals.window_corner_radius = CornerRadius::same(RADIUS);
        visuals.menu_corner_radius = CornerRadius::same(RADIUS);
        visuals.selection.bg_fill = ACCENT_SOFT;
        visuals.selection.stroke = Stroke::new(1.0, ACCENT);
        visuals.hyperlink_color = ACCENT;
        let widgets = &mut visuals.widgets;
        for (state, fill, stroke, text) in [
            (
                &mut widgets.noninteractive,
                SURFACE_1,
                BORDER,
                TEXT_SECONDARY,
            ),
            (&mut widgets.inactive, SURFACE_2, BORDER, TEXT_PRIMARY),
            (&mut widgets.hovered, SURFACE_3, BORDER_STRONG, TEXT_PRIMARY),
            (&mut widgets.active, SURFACE_3, ACCENT, TEXT_PRIMARY),
            (&mut widgets.open, SURFACE_3, BORDER_STRONG, TEXT_PRIMARY),
        ] {
            state.bg_fill = fill;
            state.weak_bg_fill = fill;
            state.bg_stroke = Stroke::new(1.0, stroke);
            state.fg_stroke = Stroke::new(1.0, text);
            state.corner_radius = CornerRadius::same(RADIUS_SMALL);
            state.expansion = 0.0;
        }
    });
}
