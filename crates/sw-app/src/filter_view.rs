//! The filter graph: the region's average spectrum with the filter chain's
//! response over it, and a handle per active filter. Drag a handle to move
//! its frequency (and a band's gain); scroll over a band to change its width.

use eframe::egui::{self, Align2, CornerRadius, FontId, Pos2, Rect, Sense, Stroke, StrokeKind, Ui, pos2, vec2};
use sw_audio::filter::{BandKind, Filters};

use crate::theme::{ACCENT, palette};

const F_MIN: f64 = 20.0;
const F_MAX: f64 = 20_000.0;
const DB_TOP: f32 = 12.0;
const DB_BOTTOM: f32 = -48.0;

#[derive(Clone, Copy, PartialEq)]
enum Handle {
    High,
    Low,
    Band(usize),
}

/// Spectrum of the region (dB per FFT bin of [`sw_audio::spectrum::SIZE`]) and its sample rate.
pub type Spectrum = (Vec<f32>, u32);

pub fn ui(ui: &mut Ui, f: &mut Filters, spectrum: Option<&Spectrum>, sr: u32) -> bool {
    let pal = palette(ui.ctx());
    let w = ui.available_width();
    let (rect, resp) = ui.allocate_exact_size(vec2(w, 170.0), Sense::click_and_drag());
    let painter = ui.painter_at(rect);
    painter.rect_filled(rect, CornerRadius::same(4), pal.lane);
    let plot = rect.shrink2(vec2(6.0, 6.0));
    let f2x = |hz: f64| plot.left() + ((hz.max(F_MIN) / F_MIN).ln() / (F_MAX / F_MIN).ln()) as f32 * plot.width();
    let x2f = |x: f32| F_MIN * (F_MAX / F_MIN).powf(((x - plot.left()) / plot.width()).clamp(0.0, 1.0) as f64);
    let db2y = |db: f32| plot.top() + (DB_TOP - db.clamp(DB_BOTTOM, DB_TOP)) / (DB_TOP - DB_BOTTOM) * plot.height();
    let y2db = |y: f32| DB_TOP - (y - plot.top()) / plot.height() * (DB_TOP - DB_BOTTOM);

    // grid
    for hz in [50.0, 100.0, 200.0, 500.0, 1000.0, 2000.0, 5000.0, 10_000.0] {
        let x = f2x(hz);
        painter.line_segment([pos2(x, plot.top()), pos2(x, plot.bottom())], Stroke::new(1.0, pal.beat_line));
        let label = if hz >= 1000.0 { format!("{}k", hz / 1000.0) } else { format!("{hz}") };
        painter.text(pos2(x + 2.0, plot.bottom() - 2.0), Align2::LEFT_BOTTOM, label, FontId::proportional(9.5), pal.dim);
    }
    for db in [0.0, -24.0] {
        let y = db2y(db);
        painter.line_segment([pos2(plot.left(), y), pos2(plot.right(), y)], Stroke::new(1.0, pal.bar_line));
        painter.text(pos2(plot.left() + 2.0, y - 1.0), Align2::LEFT_BOTTOM, format!("{db} dB"), FontId::proportional(9.5), pal.dim);
    }

    // spectrum, shifted so its loudest band sits near the top
    if let Some((spec, ssr)) = spectrum {
        let bin_hz = *ssr as f64 / sw_audio::spectrum::SIZE as f64;
        let peak = spec.iter().copied().fold(f32::MIN, f32::max);
        let shift = -6.0 - peak;
        let mut pts = Vec::new();
        let n = plot.width() as usize;
        for i in 0..=n {
            let x = plot.left() + i as f32;
            let (fa, fb) = (x2f(x), x2f(x + 1.0));
            let (ka, kb) = ((fa / bin_hz) as usize, ((fb / bin_hz) as usize).max((fa / bin_hz) as usize + 1));
            let v = spec[ka.min(spec.len() - 1)..kb.min(spec.len())].iter().copied().fold(f32::MIN, f32::max);
            if v > f32::MIN {
                pts.push(pos2(x, db2y(v + shift)));
            }
        }
        for w in pts.windows(2) {
            painter.add(egui::Shape::convex_polygon(
                vec![w[0], w[1], pos2(w[1].x, plot.bottom()), pos2(w[0].x, plot.bottom())],
                pal.dim.gamma_multiply(0.18),
                Stroke::NONE,
            ));
        }
        painter.line(pts, Stroke::new(1.0, pal.dim.gamma_multiply(0.7)));
    }

    // response
    let active = f.is_active();
    let curve: Vec<Pos2> = (0..=plot.width() as usize)
        .map(|i| {
            let x = plot.left() + i as f32;
            pos2(x, db2y(f.response_db(x2f(x), sr as f64) as f32))
        })
        .collect();
    painter.line(curve, Stroke::new(2.0, if active { ACCENT } else { pal.dim }));

    // handles
    let handles: Vec<(Handle, Pos2)> = {
        let mut v = Vec::new();
        if f.highpass.on {
            v.push((Handle::High, pos2(f2x(f.highpass.freq as f64), db2y(-3.0))));
        }
        if f.lowpass.on {
            v.push((Handle::Low, pos2(f2x(f.lowpass.freq as f64), db2y(-3.0))));
        }
        for (i, b) in f.bands.iter().enumerate().filter(|(_, b)| b.on) {
            let db = if b.kind == BandKind::Notch { -30.0 } else { b.gain_db };
            v.push((Handle::Band(i), pos2(f2x(b.freq as f64), db2y(db))));
        }
        v
    };
    let id = resp.id.with("handle");
    let mut grabbed: Option<Handle> = ui.memory(|m| m.data.get_temp::<usize>(id)).and_then(|k| match k {
        0 => Some(Handle::High),
        1 => Some(Handle::Low),
        k if k >= 2 => Some(Handle::Band(k - 2)),
        _ => None,
    });
    let hover = resp.hover_pos();
    let nearest = |p: Pos2| handles.iter().map(|(h, at)| (*h, at.distance(p))).filter(|(_, d)| *d < 14.0).min_by(|a, b| a.1.total_cmp(&b.1)).map(|h| h.0);
    if resp.drag_started()
        && let Some(p) = resp.interact_pointer_pos()
    {
        grabbed = nearest(p);
    }
    let mut changed = false;
    if resp.dragged()
        && let (Some(h), Some(p)) = (grabbed, resp.interact_pointer_pos())
    {
        let hz = x2f(p.x).clamp(F_MIN, F_MAX) as f32;
        match h {
            Handle::High => f.highpass.freq = hz.round(),
            Handle::Low => f.lowpass.freq = hz.round(),
            Handle::Band(i) => {
                f.bands[i].freq = hz.round();
                if f.bands[i].kind == BandKind::Bell {
                    f.bands[i].gain_db = (y2db(p.y).clamp(-24.0, 12.0) * 2.0).round() / 2.0;
                }
            }
        }
        changed = true;
    }
    if let Some(p) = hover
        && let Some(Handle::Band(i)) = nearest(p)
    {
        let scroll = ui.input(|i| i.smooth_scroll_delta.y);
        if scroll != 0.0 {
            f.bands[i].q = (f.bands[i].q * (1.0 + scroll * 0.005)).clamp(0.2, 30.0);
            changed = true;
        }
    }
    if resp.drag_stopped() {
        grabbed = None;
    }
    ui.memory_mut(|m| match grabbed {
        Some(h) => {
            let k: usize = match h {
                Handle::High => 0,
                Handle::Low => 1,
                Handle::Band(i) => 2 + i,
            };
            m.data.insert_temp(id, k);
        }
        None => m.data.remove::<usize>(id),
    });
    for (h, at) in &handles {
        let hot = hover.is_some_and(|p| nearest(p) == Some(*h)) || grabbed == Some(*h);
        let r = Rect::from_center_size(*at, vec2(10.0, 10.0));
        painter.rect_filled(r, CornerRadius::same(5), if hot { ACCENT } else { pal.lane });
        painter.rect_stroke(r, CornerRadius::same(5), Stroke::new(2.0, ACCENT), StrokeKind::Middle);
        let label = match h {
            Handle::High => "HP".to_string(),
            Handle::Low => "LP".to_string(),
            Handle::Band(i) => format!("{}", i + 1),
        };
        painter.text(*at + vec2(0.0, -9.0), Align2::CENTER_BOTTOM, label, FontId::proportional(10.0), pal.text);
    }
    if let Some(p) = hover
        && plot.contains(p)
    {
        let hz = x2f(p.x);
        let txt = if hz >= 1000.0 { format!("{:.2} kHz", hz / 1000.0) } else { format!("{hz:.0} Hz") };
        painter.text(pos2(plot.right() - 2.0, plot.top() + 2.0), Align2::RIGHT_TOP, txt, FontId::proportional(10.0), pal.dim);
    }
    changed
}
