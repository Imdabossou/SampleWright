//! Colours for the light and dark themes, and one colour per stem that reads
//! on both.

use eframe::egui::{self, Color32, Theme};

pub const ACCENT: Color32 = Color32::from_rgb(0xf0, 0xa0, 0x30);

/// The accent for text and thin lines: darker on light backgrounds so it stays readable.
pub fn accent_text(ctx: &egui::Context) -> Color32 {
    match ctx.theme() {
        Theme::Dark => ACCENT,
        Theme::Light => Color32::from_rgb(0xa8, 0x5c, 0x00),
    }
}

/// Colours of the timeline and graphs for the current theme.
#[derive(Clone, Copy)]
pub struct Palette {
    pub bg: Color32,
    pub lane: Color32,
    pub lane_alt: Color32,
    pub header: Color32,
    pub ruler: Color32,
    pub bar_line: Color32,
    pub beat_line: Color32,
    pub text: Color32,
    pub dim: Color32,
    pub selection: Color32,
    pub playhead: Color32,
}

pub fn palette(ctx: &egui::Context) -> Palette {
    match ctx.theme() {
        Theme::Dark => Palette {
            bg: Color32::from_rgb(0x17, 0x19, 0x1e),
            lane: Color32::from_rgb(0x1e, 0x21, 0x27),
            lane_alt: Color32::from_rgb(0x22, 0x25, 0x2c),
            header: Color32::from_rgb(0x26, 0x29, 0x31),
            ruler: Color32::from_rgb(0x1b, 0x1d, 0x23),
            bar_line: Color32::from_rgba_unmultiplied(255, 255, 255, 46),
            beat_line: Color32::from_rgba_unmultiplied(255, 255, 255, 14),
            text: Color32::from_rgb(0xdd, 0xdf, 0xe4),
            dim: Color32::from_rgb(0x8a, 0x8f, 0x9a),
            selection: Color32::from_rgba_unmultiplied(0xf0, 0xa0, 0x30, 40),
            playhead: Color32::from_rgb(0xff, 0xff, 0xff),
        },
        Theme::Light => Palette {
            bg: Color32::from_rgb(0xf3, 0xf4, 0xf6),
            lane: Color32::from_rgb(0xfb, 0xfb, 0xfc),
            lane_alt: Color32::from_rgb(0xf1, 0xf2, 0xf5),
            header: Color32::from_rgb(0xe6, 0xe8, 0xec),
            ruler: Color32::from_rgb(0xea, 0xec, 0xef),
            bar_line: Color32::from_rgba_unmultiplied(0, 0, 0, 56),
            beat_line: Color32::from_rgba_unmultiplied(0, 0, 0, 16),
            text: Color32::from_rgb(0x22, 0x25, 0x2b),
            dim: Color32::from_rgb(0x6b, 0x70, 0x7a),
            selection: Color32::from_rgba_unmultiplied(0xe0, 0x8a, 0x10, 46),
            playhead: Color32::from_rgb(0x10, 0x10, 0x10),
        },
    }
}

/// The colour of a stem's lane, waveform and regions.
pub fn stem_color(name: &str) -> Color32 {
    match name {
        "mix" => Color32::from_rgb(0x8c, 0x96, 0xa8),
        "drums" => Color32::from_rgb(0xe0, 0x5f, 0x52),
        "bass" => Color32::from_rgb(0x4d, 0x8f, 0xe6),
        "vocals" => Color32::from_rgb(0xd0, 0x68, 0xc4),
        "guitar" => Color32::from_rgb(0xe8, 0xa0, 0x38),
        "piano" => Color32::from_rgb(0x3f, 0xbf, 0x96),
        _ => Color32::from_rgb(0x95, 0x7d, 0xe0),
    }
}

/// Both themes' widget styles, with the accent colour for selections.
pub fn install(ctx: &egui::Context) {
    let mut dark = egui::Visuals::dark();
    dark.panel_fill = Color32::from_rgb(0x1b, 0x1d, 0x23);
    dark.window_fill = Color32::from_rgb(0x1f, 0x22, 0x29);
    dark.extreme_bg_color = Color32::from_rgb(0x23, 0x26, 0x2e);
    dark.selection.bg_fill = Color32::from_rgb(0xa8, 0x6a, 0x14);
    dark.hyperlink_color = ACCENT;
    ctx.set_visuals_of(Theme::Dark, dark);
    let mut light = egui::Visuals::light();
    light.selection.bg_fill = Color32::from_rgb(0xf6, 0xc8, 0x82);
    light.selection.stroke.color = Color32::from_rgb(0x40, 0x28, 0x00);
    light.hyperlink_color = Color32::from_rgb(0xa8, 0x5c, 0x00);
    ctx.set_visuals_of(Theme::Light, light);
}
