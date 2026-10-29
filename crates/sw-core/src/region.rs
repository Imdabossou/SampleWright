//! A region: a stretch of time on some stems, what kind of sample it makes,
//! and the processing chain that cleans it up.

use serde::{Deserialize, Serialize};
use sw_audio::filter::Filters;
use sw_audio::gate::Gate;
use sw_audio::{Audio, edit};

use crate::{SAMPLE_RATE, Stems};

/// What a region exports as.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub enum Kind {
    /// Exact length, crossfaded at the end so it repeats seamlessly.
    #[default]
    Loop,
    /// One sample with short fades, starting on a zero crossing.
    OneShot,
    /// Cut at every detected note or hit, one file each.
    Slices,
}

impl Kind {
    pub const ALL: [Kind; 3] = [Kind::Loop, Kind::OneShot, Kind::Slices];

    pub fn label(self) -> &'static str {
        match self {
            Kind::Loop => "Loop",
            Kind::OneShot => "One-shot",
            Kind::Slices => "Slices",
        }
    }
}

/// The processing applied to a region, in this order: filters, gate, gain,
/// loop crossfade or fades, normalise.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Chain {
    pub filters: Filters,
    pub gate: Gate,
    pub gain_db: f32,
    pub normalize: bool,
    pub normalize_db: f32,
    pub fade_in_ms: f32,
    pub fade_out_ms: f32,
    pub loop_xfade_ms: f32,
}

impl Default for Chain {
    fn default() -> Chain {
        Chain {
            filters: Filters::default(),
            gate: Gate::default(),
            gain_db: 0.0,
            normalize: false,
            normalize_db: -1.0,
            fade_in_ms: 2.0,
            fade_out_ms: 20.0,
            loop_xfade_ms: 10.0,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Region {
    pub id: u64,
    pub name: String,
    /// Seconds.
    pub start: f64,
    pub end: f64,
    /// Lanes it takes audio from (stems, or the mix).
    pub stems: Vec<String>,
    pub kind: Kind,
    pub chain: Chain,
    #[serde(default)]
    pub slicing: crate::slice::Settings,
}

/// Audio kept before a region so the filters have settled by its start, and
/// the loop crossfade has something to blend in.
const PRE_ROLL: f64 = 0.5;
/// Audio kept after it for the gate's lookahead.
const POST_ROLL: f64 = 0.01;

/// A region's audio processed by the filters, gate and gain but not yet cut:
/// `audio` starts `pre` frames before the region and runs `len` frames
/// through it, plus a little more.
pub struct Processed {
    pub audio: Audio,
    pub pre: usize,
    pub len: usize,
}

impl Region {
    pub fn new(id: u64, start: f64, end: f64, stems: Vec<String>) -> Region {
        let name = if stems.len() == 1 { stems[0].clone() } else { stems.join("+") };
        Region { id, name, start: start.min(end), end: start.max(end), stems, kind: Kind::Loop, chain: Chain::default(), slicing: Default::default() }
    }

    pub fn duration(&self) -> f64 {
        self.end - self.start
    }

    fn frames(&self) -> (usize, usize) {
        let sr = SAMPLE_RATE as f64;
        ((self.start.max(0.0) * sr).round() as usize, (self.end.max(0.0) * sr).round() as usize)
    }

    /// Sum of `lanes` around the region, through the filters, gate and gain.
    pub fn process(&self, stems: &Stems, lanes: &[String]) -> Processed {
        let sr = SAMPLE_RATE;
        let (a, b) = self.frames();
        let total = lanes.iter().filter_map(|l| stems.get(l)).map(|s| s.frames()).max().unwrap_or(0);
        let (a, b) = (a.min(total), b.min(total));
        let pre = a.min((PRE_ROLL * sr as f64) as usize);
        let post = (total - b).min((POST_ROLL * sr as f64) as usize);
        let mut audio = Audio::new(sr, vec![vec![0.0; b - a + pre + post]; 2]);
        for l in lanes {
            if let Some(s) = stems.get(l) {
                audio.mix_in(&s.slice(a - pre, b + post), 1.0);
            }
        }
        let c = &self.chain;
        if c.filters.is_active() {
            for ch in audio.channels.iter_mut() {
                c.filters.process(ch, sr);
            }
        }
        if c.gate.on {
            c.gate.process(&mut audio.channels, sr);
        }
        edit::gain(&mut audio.channels, sw_audio::db_to_gain(c.gain_db));
        Processed { audio, pre, len: b - a }
    }

    /// The finished sample of `lanes` (summed) for a loop or one-shot region.
    /// For slices, the whole region with fades: see [`crate::slice`] for the cuts.
    pub fn render(&self, stems: &Stems, lanes: &[String]) -> Audio {
        let p = self.process(stems, lanes);
        let sr = SAMPLE_RATE as f32;
        let c = &self.chain;
        let ms = |v: f32| (v.max(0.0) * sr / 1000.0) as usize;
        let mut out = match self.kind {
            Kind::Loop => {
                let xf = ms(c.loop_xfade_ms);
                let channels = p.audio.channels.iter().map(|ch| edit::seamless_loop(ch, p.pre, p.pre + p.len, xf)).collect();
                Audio::new(SAMPLE_RATE, channels)
            }
            Kind::OneShot | Kind::Slices => {
                let start = if self.kind == Kind::OneShot { edit::nearest_zero_crossing(&p.audio.channels, p.pre, ms(1.0)) } else { p.pre };
                let mut a = p.audio.slice(start, p.pre + p.len);
                edit::fade(&mut a.channels, ms(c.fade_in_ms), ms(c.fade_out_ms));
                a
            }
        };
        if c.normalize {
            edit::normalize(&mut out.channels, c.normalize_db);
        }
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;

    pub fn stems_with(name: &str, x: Vec<f32>) -> Stems {
        let mut s = Stems::default();
        s.0.insert(name.into(), Arc::new(Audio::new(SAMPLE_RATE, vec![x.clone(), x])));
        s
    }

    #[test]
    fn loop_is_exactly_the_region_and_filters_apply() {
        let sr = SAMPLE_RATE as usize;
        // a 30 Hz rumble (kick bleed) over a 220 Hz note
        let x: Vec<f32> = (0..sr * 4)
            .map(|i| {
                let t = i as f32 / sr as f32;
                0.5 * (2.0 * std::f32::consts::PI * 30.0 * t).sin() + 0.3 * (2.0 * std::f32::consts::PI * 220.0 * t).sin()
            })
            .collect();
        let stems = stems_with("bass", x);
        let mut r = Region::new(1, 1.0, 3.0, vec!["bass".into()]);
        let plain = r.render(&stems, &r.stems.clone());
        assert_eq!(plain.frames(), 2 * sr);
        r.chain.filters.highpass = sw_audio::filter::Pass { on: true, freq: 100.0, slope: sw_audio::filter::Slope::Db48 };
        let filtered = r.render(&stems, &r.stems.clone());
        assert_eq!(filtered.frames(), 2 * sr);
        // the pre-roll let the filter settle: the rumble is gone from the very start
        let head = filtered.channels[0][..2000].iter().fold(0f32, |m, v| m.max(v.abs()));
        assert!(head < 0.33 && head > 0.25, "{head}");
        r.kind = Kind::OneShot;
        r.chain.normalize = true;
        let shot = r.render(&stems, &r.stems.clone());
        assert!((shot.peak() - sw_audio::db_to_gain(-1.0)).abs() < 1e-3);
        assert!(shot.channels[0][0].abs() < 0.01, "starts on a zero crossing");
    }
}
