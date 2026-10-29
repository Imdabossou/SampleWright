//! The multitrack timeline: a bar ruler, one lane per stem (the mix on top)
//! with mute/solo, waveforms, regions and the selection.
//!
//! Drag on the lanes to select (snapped to the grid; hold Alt for free
//! selection), across several lanes to take several stems. Drag a region to
//! move it, its edges to resize it; double-click it to preview it.

use eframe::egui::{self, Align2, Color32, CornerRadius, FontId, Pos2, Rect, Sense, Stroke, StrokeKind, Ui, pos2, vec2};
use sw_audio::peaks::Peaks;
use sw_core::grid::Snap;
use sw_core::slice::Slice;
use sw_core::{SAMPLE_RATE, Session};

use crate::theme::{self, ACCENT, palette, stem_color};

const LEFT: f32 = 150.0;
const RULER_H: f32 = 28.0;
const SCROLL_H: f32 = 14.0;
const LANE_MIN: f32 = 44.0;
const LANE_MAX: f32 = 150.0;
const EDGE_PX: f32 = 6.0;

/// A time selection over a range of lanes (inclusive, display order).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Selection {
    pub start: f64,
    pub end: f64,
    pub lanes: (usize, usize),
}

#[derive(Debug, Clone, Copy)]
enum Drag {
    Select { anchor: f64, lane: usize },
    Seek,
    Edge { id: u64, start: bool },
    Move { id: u64, grab: f64, moved: bool },
    Scrollbar { grab: f64 },
}

/// Per-lane mixer state shown in the header.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct LaneState {
    pub mute: bool,
    pub solo: bool,
    pub gain: f32,
}

impl Default for LaneState {
    fn default() -> LaneState {
        LaneState { mute: false, solo: false, gain: 1.0 }
    }
}

#[derive(Default)]
pub struct Events {
    pub seek: Option<f64>,
    /// Some(None) clears the region selection.
    pub select_region: Option<Option<u64>>,
    /// A region was moved or resized.
    pub region_changed: bool,
    pub toggle_mute: Option<usize>,
    pub toggle_solo: Option<usize>,
    pub preview_region: Option<u64>,
}

pub struct Timeline {
    pub scroll_t: f64,
    /// Pixels per second.
    pub pps: f32,
    width: f32,
    pub snap: Snap,
    pub selection: Option<Selection>,
    drag: Option<Drag>,
    pub peaks: Vec<Peaks>,
    /// Fit the song to the view once the view's width is known.
    fit_pending: bool,
}

impl Default for Timeline {
    fn default() -> Timeline {
        Timeline { scroll_t: 0.0, pps: 40.0, width: 1000.0, snap: Snap::Bar, selection: None, drag: None, peaks: Vec::new(), fit_pending: false }
    }
}

fn fmt_time(t: f64) -> String {
    let t = t.max(0.0);
    format!("{}:{:04.1}", (t / 60.0) as u64, t % 60.0)
}

impl Timeline {
    /// A selection, move or resize is in progress (analysis waits for it to end).
    pub fn dragging(&self) -> bool {
        matches!(self.drag, Some(Drag::Select { .. } | Drag::Edge { .. } | Drag::Move { .. }))
    }

    /// Seconds visible.
    pub fn span(&self) -> f64 {
        ((self.width - LEFT).max(10.0) / self.pps) as f64
    }

    pub fn reset(&mut self, duration: f64) {
        self.fit_pending = true;
        self.scroll_t = 0.0;
        self.selection = None;
        self.drag = None;
        // the whole song across the view, but never so tight that bars blur
        self.pps = (((self.width - LEFT).max(400.0) as f64 / duration.max(1.0)) as f32).max(4.0);
    }

    fn clamp_scroll(&mut self, duration: f64) {
        let max = (duration - self.span() * 0.5).max(0.0);
        self.scroll_t = self.scroll_t.clamp(0.0, max);
    }

    pub fn zoom(&mut self, f: f32, anchor_x: Option<f32>, duration: f64) {
        let ax = anchor_x.unwrap_or((self.width - LEFT) / 2.0).max(0.0);
        let t_at = self.scroll_t + (ax / self.pps) as f64;
        self.pps = (self.pps * f).clamp(2.0, 4000.0);
        self.scroll_t = t_at - (ax / self.pps) as f64;
        self.clamp_scroll(duration);
    }

    /// Scrolls so `t` is in view (paging, as DAWs follow the playhead).
    pub fn follow(&mut self, t: f64, duration: f64) {
        let span = self.span();
        if t < self.scroll_t || t > self.scroll_t + span * 0.95 {
            self.scroll_t = t - span * 0.05;
            self.clamp_scroll(duration);
        }
    }

    /// Zooms to show `a..b` with a margin.
    pub fn show_range(&mut self, a: f64, b: f64, duration: f64) {
        let len = (b - a).max(0.5);
        self.pps = (((self.width - LEFT).max(100.0) as f64 / (len * 1.3)) as f32).clamp(2.0, 4000.0);
        self.scroll_t = a - len * 0.15;
        self.clamp_scroll(duration);
    }

    #[allow(clippy::too_many_arguments)]
    pub fn ui(
        &mut self,
        ui: &mut Ui,
        session: Option<&mut Session>,
        lanes: &[LaneState],
        selected: Option<u64>,
        playhead: f64,
        slices: Option<(u64, &[Slice])>,
        busy: Option<&str>,
    ) -> Events {
        let mut ev = Events::default();
        let pal = palette(ui.ctx());
        let full = ui.available_rect_before_wrap();
        let (rect, scroll_rect) = full.split_top_bottom_at_y(full.bottom() - SCROLL_H);
        let resp = ui.allocate_rect(full, Sense::click_and_drag());
        let painter = ui.painter_at(full);
        painter.rect_filled(full, 0.0, pal.bg);
        self.width = rect.width();
        let Some(s) = session else {
            let text = busy.unwrap_or("Open a song to start: File > Open Song, or drop an audio file here");
            painter.text(rect.center(), Align2::CENTER_CENTER, text, FontId::proportional(16.0), pal.dim);
            return ev;
        };
        if self.fit_pending {
            self.fit_pending = false;
            self.pps = (((self.width - LEFT).max(100.0) as f64 / s.duration.max(1.0)) as f32).max(2.0);
        }
        let names = s.lanes();
        let n_lanes = names.len();
        let lanes_top = rect.top() + RULER_H;
        let lane_h = ((rect.bottom() - lanes_top) / n_lanes as f32).clamp(LANE_MIN, LANE_MAX);
        let x0 = rect.left() + LEFT;
        let t2x = |t: f64, tl: &Timeline| x0 + ((t - tl.scroll_t) as f32) * tl.pps;
        let x2t = |x: f32, tl: &Timeline| tl.scroll_t + ((x - x0) / tl.pps) as f64;
        let lane_rect = |i: usize| Rect::from_min_size(pos2(x0, lanes_top + i as f32 * lane_h), vec2(rect.right() - x0, lane_h));
        let lane_at = |y: f32| -> usize { (((y - lanes_top) / lane_h).floor().max(0.0) as usize).min(n_lanes - 1) };
        let lane_of = |name: &str| names.iter().position(|n| n == name);
        let duration = s.duration;

        // ---------------------------------------------------------- input
        let (pressed, down, released, mods, hover, scroll, zoom, double) = ui.input(|i| {
            (
                i.pointer.primary_pressed(),
                i.pointer.primary_down(),
                i.pointer.primary_released(),
                i.modifiers,
                i.pointer.interact_pos(),
                i.smooth_scroll_delta,
                i.zoom_delta(),
                i.pointer.button_double_clicked(egui::PointerButton::Primary),
            )
        });
        let covered = hover.is_some_and(|h| ui.ctx().layer_id_at(h).is_some_and(|l| l != ui.layer_id()));
        let inside = hover.is_some_and(|h| full.contains(h)) && !covered;
        let pressed = pressed && inside;
        if inside && (zoom - 1.0).abs() > 1e-3 {
            self.zoom(zoom, hover.map(|h| h.x - x0), duration);
        } else if inside && (scroll.x != 0.0 || scroll.y != 0.0) {
            let d = if scroll.x.abs() > scroll.y.abs() { scroll.x } else { scroll.y };
            self.scroll_t -= (d / self.pps) as f64;
            self.clamp_scroll(duration);
        }
        let snap = |t: f64, tl: &Timeline| if mods.alt { t } else { s.grid.snap(t, tl.snap) };

        // region hit-testing: (id, is it on an edge, which edge)
        let region_at = |pos: Pos2, tl: &Timeline, regions: &[sw_core::Region]| -> Option<(u64, Option<bool>)> {
            if pos.x < x0 || pos.y < lanes_top {
                return None;
            }
            let lane = lane_at(pos.y);
            let mut hit = None;
            for r in regions.iter().rev() {
                if !r.stems.iter().any(|st| lane_of(st) == Some(lane)) {
                    continue;
                }
                let (a, b) = (t2x(r.start, tl), t2x(r.end, tl));
                if (pos.x - a).abs() <= EDGE_PX {
                    return Some((r.id, Some(true)));
                }
                if (pos.x - b).abs() <= EDGE_PX {
                    return Some((r.id, Some(false)));
                }
                if pos.x > a && pos.x < b && hit.is_none() {
                    hit = Some((r.id, None));
                }
            }
            hit
        };

        let thumb = |tl: &Timeline| {
            let total = duration.max(tl.span());
            let w = (tl.span() / total) as f32 * scroll_rect.width();
            let x = scroll_rect.left() + (tl.scroll_t / total) as f32 * scroll_rect.width();
            Rect::from_min_size(pos2(x, scroll_rect.top() + 3.0), vec2(w.max(24.0), SCROLL_H - 6.0))
        };

        if pressed && let Some(pos) = hover {
            if scroll_rect.contains(pos) {
                let total = duration.max(self.span());
                if !thumb(self).contains(pos) {
                    self.scroll_t = (pos.x - scroll_rect.left()) as f64 / scroll_rect.width() as f64 * total - self.span() / 2.0;
                    self.clamp_scroll(duration);
                }
                let grab = (pos.x - thumb(self).left()) as f64 / scroll_rect.width() as f64 * total;
                self.drag = Some(Drag::Scrollbar { grab });
            } else if pos.x < x0 {
                // lane header: mute / solo buttons
                if pos.y >= lanes_top {
                    let i = lane_at(pos.y);
                    let top = lanes_top + i as f32 * lane_h;
                    let (m, so) = header_buttons(rect.left(), top);
                    if m.contains(pos) {
                        ev.toggle_mute = Some(i);
                    } else if so.contains(pos) {
                        ev.toggle_solo = Some(i);
                    }
                }
            } else if pos.y < lanes_top {
                ev.seek = Some(x2t(pos.x, self).clamp(0.0, duration));
                self.drag = Some(Drag::Seek);
            } else if let Some((id, edge)) = region_at(pos, self, &s.regions) {
                ev.select_region = Some(Some(id));
                if double {
                    ev.preview_region = Some(id);
                } else if let Some(start) = edge {
                    self.drag = Some(Drag::Edge { id, start });
                } else if let Some(r) = s.regions.iter().find(|r| r.id == id) {
                    self.drag = Some(Drag::Move { id, grab: x2t(pos.x, self) - r.start, moved: false });
                }
            } else {
                let t = snap(x2t(pos.x, self), self).clamp(0.0, duration);
                let lane = lane_at(pos.y);
                self.drag = Some(Drag::Select { anchor: t, lane });
                self.selection = None;
                ev.select_region = Some(None);
            }
        }

        if down && let (Some(drag), Some(pos)) = (self.drag, hover) {
            let t = x2t(pos.x, self);
            match drag {
                Drag::Scrollbar { grab } => {
                    let total = duration.max(self.span());
                    self.scroll_t = (pos.x - scroll_rect.left()) as f64 / scroll_rect.width() as f64 * total - grab;
                    self.clamp_scroll(duration);
                }
                Drag::Seek => ev.seek = Some(t.clamp(0.0, duration)),
                Drag::Select { anchor, lane } => {
                    let t = snap(t, self).clamp(0.0, duration);
                    let l = lane_at(pos.y);
                    self.selection = Some(Selection { start: anchor.min(t), end: anchor.max(t), lanes: (lane.min(l), lane.max(l)) });
                }
                Drag::Edge { id, start } => {
                    let t = snap(t, self).clamp(0.0, duration);
                    if let Some(r) = s.regions.iter_mut().find(|r| r.id == id) {
                        let min = 0.01;
                        if start && t < r.end - min {
                            r.start = t;
                            ev.region_changed = true;
                        } else if !start && t > r.start + min {
                            r.end = t;
                            ev.region_changed = true;
                        }
                    }
                }
                Drag::Move { id, grab, moved } => {
                    if let Some(r) = s.regions.iter_mut().find(|r| r.id == id) {
                        let len = r.duration();
                        let start = snap(t - grab, self).clamp(0.0, (duration - len).max(0.0));
                        if (start - r.start).abs() > 1e-9 && (moved || (t - grab - r.start).abs() * self.pps as f64 > 3.0) {
                            r.start = start;
                            r.end = start + len;
                            ev.region_changed = true;
                            self.drag = Some(Drag::Move { id, grab, moved: true });
                        }
                    }
                }
            }
            // keep dragging past the edges
            if matches!(drag, Drag::Select { .. } | Drag::Edge { .. } | Drag::Move { .. }) {
                if pos.x > rect.right() - 20.0 {
                    self.scroll_t += (6.0 / self.pps) as f64;
                } else if pos.x < x0 + 20.0 {
                    self.scroll_t -= (6.0 / self.pps) as f64;
                }
                self.clamp_scroll(duration);
                ui.ctx().request_repaint();
            }
        }
        if released {
            if let Some(Drag::Select { anchor, .. }) = self.drag
                && let Some(pos) = hover
            {
                let width = self.selection.map_or(0.0, |sel| ((sel.end - sel.start) as f32) * self.pps);
                if width < 3.0 {
                    // a click: move the playhead there
                    self.selection = None;
                    ev.seek = Some(if mods.alt { x2t(pos.x, self) } else { anchor }.clamp(0.0, duration));
                }
            }
            self.drag = None;
        }
        if resp.hovered()
            && let Some(pos) = hover
        {
            let icon = match region_at(pos, self, &s.regions) {
                Some((_, Some(_))) => egui::CursorIcon::ResizeHorizontal,
                Some((_, None)) => egui::CursorIcon::Grab,
                None if pos.x >= x0 && pos.y >= lanes_top => egui::CursorIcon::Text,
                None => egui::CursorIcon::Default,
            };
            ui.ctx().set_cursor_icon(icon);
        }

        // ---------------------------------------------------------- drawing
        let (t_from, t_to) = (self.scroll_t, self.scroll_t + self.span());
        let content = Rect::from_min_max(pos2(x0, rect.top()), rect.max);
        let clip = painter.with_clip_rect(content);

        for i in 0..n_lanes {
            let lr = lane_rect(i);
            clip.rect_filled(lr, 0.0, if i % 2 == 0 { pal.lane } else { pal.lane_alt });
        }
        // grid
        let beat_px = s.grid.bpm().map_or(0.0, |b| 60.0 / b as f32 * self.pps);
        if beat_px > 6.0 {
            for &b in s.grid.beats.iter().filter(|&&b| b >= t_from && b <= t_to) {
                let x = t2x(b, self);
                clip.line_segment([pos2(x, lanes_top), pos2(x, rect.bottom())], Stroke::new(1.0, pal.beat_line));
            }
        }
        let bar_px = beat_px * s.grid.beats_per_bar().unwrap_or(4) as f32;
        let label_every = (1..).map(|k| 1usize << k >> 1).find(|&k| bar_px * k as f32 > 34.0).unwrap_or(64);
        clip.rect_filled(Rect::from_min_max(pos2(x0, rect.top()), pos2(rect.right(), lanes_top)), 0.0, pal.ruler);
        for (k, &d) in s.grid.downbeats.iter().enumerate() {
            if d < t_from - 10.0 || d > t_to {
                continue;
            }
            let x = t2x(d, self);
            let labelled = k % label_every == 0;
            if bar_px > 3.0 || labelled {
                clip.line_segment([pos2(x, if labelled { rect.top() + 4.0 } else { lanes_top - 8.0 }), pos2(x, rect.bottom())], Stroke::new(1.0, pal.bar_line));
            }
            if labelled {
                clip.text(pos2(x + 4.0, rect.top() + 4.0), Align2::LEFT_TOP, format!("{}", k + 1), FontId::monospace(11.0), pal.text);
            }
        }
        if s.grid.downbeats.is_empty() {
            // no beat grid: seconds
            let step = [1.0, 2.0, 5.0, 10.0, 15.0, 30.0, 60.0].into_iter().find(|st| st * self.pps as f64 > 60.0).unwrap_or(120.0);
            let mut t = (t_from / step).floor() * step;
            while t <= t_to {
                let x = t2x(t, self);
                clip.line_segment([pos2(x, rect.top() + 14.0), pos2(x, rect.bottom())], Stroke::new(1.0, pal.bar_line));
                clip.text(pos2(x + 3.0, rect.top() + 3.0), Align2::LEFT_TOP, fmt_time(t), FontId::monospace(11.0), pal.dim);
                t += step;
            }
        }

        // region fills sit under the waveforms, so the audio stays readable
        for r in &s.regions {
            let (a, b) = (t2x(r.start, self), t2x(r.end, self));
            if b < x0 || a > rect.right() {
                continue;
            }
            let is_sel = selected == Some(r.id);
            for st in &r.stems {
                let Some(i) = lane_of(st) else { continue };
                let lr = lane_rect(i);
                let box_ = Rect::from_min_max(pos2(a, lr.top() + 2.0), pos2(b, lr.bottom() - 2.0));
                let col = if is_sel { ACCENT } else { stem_color(st) };
                clip.rect_filled(box_, CornerRadius::same(4), col.gamma_multiply(if is_sel { 0.22 } else { 0.12 }));
            }
        }

        // waveforms
        let sr = SAMPLE_RATE as f64;
        for (i, name) in names.iter().enumerate() {
            let Some(p) = self.peaks.get(i) else { continue };
            let lr = lane_rect(i);
            let st = lanes.get(i).copied().unwrap_or_default();
            let audible = !st.mute && (st.solo || !lanes.iter().any(|l| l.solo));
            let col = stem_color(name).gamma_multiply(if audible { 0.9 } else { 0.3 });
            let mid = lr.center().y;
            // quiet stems are scaled up (up to 4x) so their shape is visible
            let (lo_all, hi_all) = p.range(0, p.frames());
            let lane_peak = lo_all.abs().max(hi_all.abs()).max(1e-6);
            let half = lr.height() * 0.45 * (1.0 / lane_peak).clamp(1.0, 4.0);
            let (px0, px1) = (x0.max(t2x(0.0, self)) as i32, rect.right().min(t2x(duration, self)) as i32);
            for px in px0..px1 {
                let ta = x2t(px as f32, self);
                let tb = x2t(px as f32 + 1.0, self);
                let (lo, hi) = p.range((ta * sr).max(0.0) as usize, (tb * sr).max(0.0) as usize);
                if hi < lo {
                    continue;
                }
                let lim = lr.height() * 0.47;
                let (y0, y1) = (mid - (hi * half).min(lim), (mid - (lo * half).max(-lim)).max(mid - (hi * half).min(lim) + 1.0));
                clip.line_segment([pos2(px as f32 + 0.5, y0), pos2(px as f32 + 0.5, y1)], Stroke::new(1.0, col));
            }
        }

        // region outlines, labels and slice markers
        for r in &s.regions {
            let is_sel = selected == Some(r.id);
            let (a, b) = (t2x(r.start, self), t2x(r.end, self));
            if b < x0 || a > rect.right() {
                continue;
            }
            let mut top_lane = usize::MAX;
            for st in &r.stems {
                let Some(i) = lane_of(st) else { continue };
                top_lane = top_lane.min(i);
                let lr = lane_rect(i);
                let box_ = Rect::from_min_max(pos2(a, lr.top() + 2.0), pos2(b, lr.bottom() - 2.0));
                let col = if is_sel { ACCENT } else { stem_color(st) };
                clip.rect_stroke(box_, CornerRadius::same(4), Stroke::new(if is_sel { 2.0 } else { 1.0 }, col), StrokeKind::Inside);
            }
            if top_lane != usize::MAX {
                let lr = lane_rect(top_lane);
                let label = format!("{}  ·  {}", r.name, r.kind.label());
                let galley = clip.layout_no_wrap(label, FontId::proportional(11.5), pal.text);
                let tag = Rect::from_min_size(pos2(a.max(x0) + 4.0, lr.top() + 5.0), galley.size() + vec2(8.0, 2.0));
                if tag.width() < (b - a.max(x0)) - 4.0 {
                    clip.rect_filled(tag, CornerRadius::same(3), pal.lane.gamma_multiply(0.85));
                    clip.galley(tag.min + vec2(4.0, 1.0), galley, pal.text);
                }
            }
            if let Some((id, cuts)) = slices
                && id == r.id
            {
                let lanes_of: Vec<usize> = r.stems.iter().filter_map(|st| lane_of(st)).collect();
                let (lo, hi) = (lanes_of.iter().min().copied().unwrap_or(0), lanes_of.iter().max().copied().unwrap_or(0));
                let (ya, yb) = (lane_rect(lo).top() + 2.0, lane_rect(hi).bottom() - 2.0);
                for c in cuts {
                    let x = t2x(c.start, self);
                    clip.line_segment([pos2(x, ya), pos2(x, yb)], Stroke::new(1.5, ACCENT));
                    if let Some(l) = &c.label
                        && (t2x(c.end, self) - x) > 22.0
                    {
                        clip.text(pos2(x + 3.0, yb - 3.0), Align2::LEFT_BOTTOM, l, FontId::proportional(10.5), pal.text);
                    }
                }
            }
        }

        // selection
        if let Some(sel) = self.selection {
            let (a, b) = (t2x(sel.start, self), t2x(sel.end, self));
            let (ta, tb) = (lane_rect(sel.lanes.0).top(), lane_rect(sel.lanes.1).bottom());
            let r = Rect::from_min_max(pos2(a, ta), pos2(b.max(a + 1.0), tb));
            clip.rect_filled(r, 0.0, pal.selection);
            clip.rect_stroke(r, 0.0, Stroke::new(1.0, ACCENT), StrokeKind::Inside);
            let len = sel.end - sel.start;
            let bars = s.grid.bars_of(sel.start, sel.end).map(|(x, y)| format!("  ·  {} bar{}", y + 1 - x, if y > x { "s" } else { "" })).unwrap_or_default();
            clip.text(pos2(a + 4.0, tb - 4.0), Align2::LEFT_BOTTOM, format!("{len:.2} s{bars}"), FontId::proportional(11.0), pal.text);
        }

        // playhead
        let px = t2x(playhead, self);
        if px >= x0 && px <= rect.right() {
            clip.line_segment([pos2(px, rect.top()), pos2(px, rect.bottom())], Stroke::new(1.5, pal.playhead));
            clip.add(egui::Shape::convex_polygon(
                vec![pos2(px - 5.0, rect.top()), pos2(px + 5.0, rect.top()), pos2(px, rect.top() + 7.0)],
                pal.playhead,
                Stroke::NONE,
            ));
        }

        // lane headers
        painter.rect_filled(Rect::from_min_max(rect.left_top(), pos2(x0, rect.bottom())), 0.0, pal.header);
        painter.text(pos2(rect.left() + 10.0, rect.top() + 8.0), Align2::LEFT_TOP, "BAR", FontId::proportional(10.5), pal.dim);
        for (i, name) in names.iter().enumerate() {
            let top = lanes_top + i as f32 * lane_h;
            let st = lanes.get(i).copied().unwrap_or_default();
            let col = stem_color(name);
            painter.rect_filled(Rect::from_min_size(pos2(rect.left(), top + 1.0), vec2(4.0, lane_h - 2.0)), 0.0, col);
            painter.line_segment([pos2(rect.left(), top), pos2(x0, top)], Stroke::new(1.0, pal.beat_line));
            let label = if name == sw_core::MIX { "Full mix".to_string() } else { capitalise(name) };
            painter.text(pos2(rect.left() + 12.0, top + 8.0), Align2::LEFT_TOP, label, FontId::proportional(13.5), pal.text);
            let (m, so) = header_buttons(rect.left(), top);
            for (r, on, txt, on_col) in [(m, st.mute, "M", Color32::from_rgb(0xd0, 0x50, 0x50)), (so, st.solo, "S", Color32::from_rgb(0xe0, 0xb0, 0x30))] {
                if r.bottom() > top + lane_h {
                    continue;
                }
                painter.rect_filled(r, CornerRadius::same(3), if on { on_col } else { pal.lane_alt });
                painter.rect_stroke(r, CornerRadius::same(3), Stroke::new(1.0, pal.beat_line.gamma_multiply(3.0)), StrokeKind::Inside);
                painter.text(r.center(), Align2::CENTER_CENTER, txt, FontId::proportional(11.0), if on { Color32::BLACK } else { pal.dim });
            }
            if st.gain != 1.0 {
                painter.text(pos2(x0 - 8.0, top + 8.0), Align2::RIGHT_TOP, format!("{:+.0} dB", sw_audio::gain_to_db(st.gain)), FontId::proportional(10.5), pal.dim);
            }
        }
        painter.line_segment([pos2(x0, rect.top()), pos2(x0, rect.bottom())], Stroke::new(1.0, pal.bar_line));
        painter.text(pos2(x0 - 8.0, rect.top() + 8.0), Align2::RIGHT_TOP, fmt_time(playhead), FontId::monospace(11.0), theme::accent_text(ui.ctx()));

        // scrollbar
        painter.rect_filled(scroll_rect, 0.0, pal.ruler);
        painter.rect_filled(thumb(self), CornerRadius::same(4), pal.dim.gamma_multiply(0.6));
        ev
    }
}

/// The mute and solo buttons of the lane header whose top is at `top`.
fn header_buttons(left: f32, top: f32) -> (Rect, Rect) {
    let m = Rect::from_min_size(pos2(left + 12.0, top + 27.0), vec2(22.0, 16.0));
    (m, m.translate(vec2(26.0, 0.0)))
}

pub fn capitalise(s: &str) -> String {
    let mut c = s.chars();
    c.next().map(|f| f.to_uppercase().chain(c).collect()).unwrap_or_default()
}
