//! Cutting a region into one-shots: one sample per detected note or hit,
//! named by its pitch (bass and melodic stems) or its drum type.

use serde::{Deserialize, Serialize};
use sw_audio::{Audio, edit, onset, pitch};

use crate::region::Processed;
use crate::{Region, SAMPLE_RATE, Stems};

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct Settings {
    pub sensitivity: f32,
    pub min_gap_ms: f32,
    pub decay_db: f32,
    pub max_len_ms: f32,
}

impl Default for Settings {
    fn default() -> Settings {
        let d = onset::Settings::default();
        Settings { sensitivity: d.sensitivity, min_gap_ms: d.min_gap_ms, decay_db: d.decay_db, max_len_ms: d.max_len_ms }
    }
}

/// One cut, in seconds of the song.
#[derive(Debug, Clone, PartialEq)]
pub struct Slice {
    pub start: f64,
    pub end: f64,
    /// Pitch name ("E1") or drum type ("kick"), if recognised.
    pub label: Option<String>,
    pub strength: f32,
}

/// Rough drum type from where the hit's energy sits: kick (lows), hat or
/// cymbal (highs), otherwise snare/tom.
fn drum_type(x: &[f32], sr: u32) -> &'static str {
    use sw_audio::filter::Biquad;
    let n = x.len().min((0.12 * sr as f32) as usize);
    let x = &x[..n];
    let energy = |y: &[f32]| y.iter().map(|v| v * v).sum::<f32>();
    let mut low = x.to_vec();
    Biquad::lowpass(150.0, 0.707, sr as f64).process(&mut low);
    let mut high = x.to_vec();
    Biquad::highpass(5000.0, 0.707, sr as f64).process(&mut high);
    let (all, lo, hi) = (energy(x).max(1e-12), energy(&low), energy(&high));
    if lo / all > 0.5 {
        "kick"
    } else if hi / all > 0.4 {
        "hat"
    } else {
        "snare"
    }
}

/// The slices of a region (after its filters and gate, which helps: a
/// high-pass on the drums finds the hats alone).
pub fn detect(region: &Region, stems: &Stems) -> Vec<Slice> {
    let p = region.process(stems, &region.stems);
    detect_in(region, &p)
}

pub fn detect_in(region: &Region, p: &Processed) -> Vec<Slice> {
    let sr = SAMPLE_RATE;
    let body = p.audio.slice(p.pre, p.pre + p.len);
    let x = body.mono();
    let s = &region.slicing;
    let found = onset::detect(&x, sr, &onset::Settings { sensitivity: s.sensitivity, min_gap_ms: s.min_gap_ms, decay_db: s.decay_db, max_len_ms: s.max_len_ms });
    let drums = region.stems.iter().all(|l| l == "drums");
    found
        .iter()
        .map(|o| {
            let seg = &x[o.start..o.end];
            let label = if drums {
                Some(drum_type(seg, sr).to_string())
            } else {
                pitch::note_pitch(seg, sr).map(|p| p.name())
            };
            Slice {
                start: region.start + o.start as f64 / sr as f64,
                end: region.start + o.end as f64 / sr as f64,
                label,
                strength: o.strength,
            }
        })
        .collect()
}

/// The finished audio of one slice, cut from the processed region.
pub fn render(region: &Region, p: &Processed, slice: &Slice) -> Audio {
    let sr = SAMPLE_RATE as f64;
    let ms = |v: f32| (v.max(0.0) as f64 * sr / 1000.0) as usize;
    let to_frame = |t: f64| (p.pre as f64 + (t - region.start) * sr).round().max(0.0) as usize;
    // start just before the attack, on a zero crossing
    let a = to_frame(slice.start).saturating_sub(ms(2.0));
    let a = edit::nearest_zero_crossing(&p.audio.channels, a, ms(1.0));
    let b = to_frame(slice.end).min(p.pre + p.len);
    let mut out = p.audio.slice(a, b.max(a + 1));
    let c = &region.chain;
    let len = out.frames();
    edit::fade(&mut out.channels, ms(c.fade_in_ms.min(5.0)), ms(c.fade_out_ms).min(len / 4));
    if c.normalize {
        edit::normalize(&mut out.channels, c.normalize_db);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::Kind;
    use std::sync::Arc;

    #[test]
    fn slices_a_bassline_into_named_notes() {
        let sr = SAMPLE_RATE as usize;
        let notes = [(0.5, 41.2), (1.0, 55.0), (1.5, 73.42), (2.0, 41.2)];
        let mut x = vec![0f32; sr * 3];
        for (t, hz) in notes {
            let a = (t * sr as f64) as usize;
            let len = sr / 2 - 200;
            for i in 0..len {
                let tt = i as f32 / sr as f32;
                // players damp the string: a 20 ms release, not a hard stop
                let release = ((len - i) as f32 / (0.02 * sr as f32)).min(1.0);
                let ph = 2.0 * std::f32::consts::PI * hz as f32 * tt;
                x[a + i] += 0.5 * (ph.sin() + 0.5 * (2.0 * ph).sin()) * (-tt * 3.0).exp() * (tt / 0.002).min(1.0) * release;
            }
        }
        let mut stems = Stems::default();
        stems.0.insert("bass".into(), Arc::new(Audio::new(SAMPLE_RATE, vec![x.clone(), x])));
        let mut r = Region::new(1, 0.25, 2.75, vec!["bass".into()]);
        r.kind = Kind::Slices;
        let slices = detect(&r, &stems);
        let labels: Vec<_> = slices.iter().map(|s| s.label.clone().unwrap_or_default()).collect();
        assert_eq!(labels, ["E1", "A1", "D2", "E1"], "{slices:?}");
        let p = r.process(&stems, &r.stems);
        let shot = render(&r, &p, &slices[1]);
        let len = shot.frames() as f64 / sr as f64;
        assert!(len > 0.4 && len < 0.52, "{len}");
    }
}
