//! Colours for the light and dark themes: neutral greys plus the system accent.
//! Stem colours stay fixed because they tell the stems apart, and read on both.

use crate::system_accent;
use eframe::egui::{self, Color32, Theme};

/// The accent for selections and highlights: the desktop's own, or monochrome if it has none.
pub fn accent(ctx: &egui::Context) -> Color32 {
    let dark = ctx.theme() == Theme::Dark;
    match system_accent::get() {
        Some(c) => system_accent::readable(c, dark),
        None if dark => Color32::from_gray(0xe6),
        None => Color32::from_gray(0x1f),
    }
}

/// The accent for text and thin lines, always readable on the theme's background.
pub fn accent_text(ctx: &egui::Context) -> Color32 {
    accent(ctx)
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

fn selection_fill(ctx: &egui::Context, alpha: u8) -> Color32 {
    let c = accent(ctx);
    Color32::from_rgba_unmultiplied(c.r(), c.g(), c.b(), alpha)
}

pub fn palette(ctx: &egui::Context) -> Palette {
    match ctx.theme() {
        Theme::Dark => Palette {
            bg: Color32::from_gray(0x19),
            lane: Color32::from_gray(0x20),
            lane_alt: Color32::from_gray(0x24),
            header: Color32::from_gray(0x29),
            ruler: Color32::from_gray(0x1c),
            bar_line: Color32::from_rgba_unmultiplied(255, 255, 255, 46),
            beat_line: Color32::from_rgba_unmultiplied(255, 255, 255, 14),
            text: Color32::from_gray(0xe0),
            dim: Color32::from_gray(0x90),
            selection: selection_fill(ctx, 40),
            playhead: Color32::from_rgb(0xff, 0xff, 0xff),
        },
        Theme::Light => Palette {
            bg: Color32::from_gray(0xf4),
            lane: Color32::from_gray(0xfb),
            lane_alt: Color32::from_gray(0xf2),
            header: Color32::from_gray(0xe8),
            ruler: Color32::from_gray(0xec),
            bar_line: Color32::from_rgba_unmultiplied(0, 0, 0, 56),
            beat_line: Color32::from_rgba_unmultiplied(0, 0, 0, 16),
            text: Color32::from_gray(0x24),
            dim: Color32::from_gray(0x6e),
            selection: selection_fill(ctx, 46),
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

/// Both themes' widget styles. Called again whenever the system accent changes.
pub fn install(ctx: &egui::Context) {
    let accent = system_accent::get();
    let mut dark = egui::Visuals::dark();
    dark.panel_fill = Color32::from_gray(0x1c);
    dark.window_fill = Color32::from_gray(0x21);
    dark.extreme_bg_color = Color32::from_gray(0x26);
    dark.selection.bg_fill = accent.map_or(Color32::from_gray(0x58), |c| c.gamma_multiply(0.55));
    dark.selection.stroke.color = Color32::WHITE;
    dark.hyperlink_color = accent.map_or(Color32::from_gray(0xe6), |c| system_accent::readable(c, true));
    ctx.set_visuals_of(Theme::Dark, dark);
    let mut light = egui::Visuals::light();
    light.selection.bg_fill = accent.map_or(Color32::from_gray(0xc8), |c| c.gamma_multiply(0.45));
    light.selection.stroke.color = Color32::BLACK;
    light.hyperlink_color = accent.map_or(Color32::from_gray(0x1f), |c| system_accent::readable(c, false));
    ctx.set_visuals_of(Theme::Light, light);
}

/// Restyles the widgets if the system accent has changed since the last frame.
pub fn refresh(ctx: &egui::Context, last: &mut Option<Color32>) {
    let now = system_accent::get();
    if now != *last {
        *last = now;
        install(ctx);
    }
}
