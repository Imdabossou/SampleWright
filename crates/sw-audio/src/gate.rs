//! A noise gate for the bleed left between notes: it closes when the signal
//! stays below the threshold and opens just before it comes back (lookahead),
//! so note attacks are never clipped.

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct Gate {
    pub on: bool,
    pub threshold_db: f32,
    /// How far the gate turns the signal down when closed (dB, positive).
    pub range_db: f32,
    pub attack_ms: f32,
    pub hold_ms: f32,
    pub release_ms: f32,
}

impl Default for Gate {
    fn default() -> Gate {
        Gate { on: false, threshold_db: -40.0, range_db: 60.0, attack_ms: 1.0, hold_ms: 30.0, release_ms: 80.0 }
    }
}

const LOOKAHEAD_MS: f32 = 3.0;

impl Gate {
    /// Gain curve for audio whose channels are `channels` (the gate is linked:
    /// all channels get the same gain so the stereo image does not wander).
    pub fn gains(&self, channels: &[Vec<f32>], sr: u32) -> Vec<f32> {
        let n = channels.first().map_or(0, Vec::len);
        let ms = |v: f32| (v.max(0.0) * sr as f32 / 1000.0) as usize;
        let coef = |m: f32| if m <= 0.0 { 0.0 } else { (-1.0 / (m * sr as f32 / 1000.0)).exp() };
        let thr = crate::db_to_gain(self.threshold_db);
        let floor = crate::db_to_gain(-self.range_db.abs());
        let (look, hold) = (ms(LOOKAHEAD_MS), ms(self.hold_ms));
        let (att, rel) = (coef(self.attack_ms), coef(self.release_ms));

        // a sample is "open" if any channel within the lookahead is above the threshold
        let mut level = vec![0f32; n];
        for c in channels {
            for (l, v) in level.iter_mut().zip(c) {
                *l = l.max(v.abs());
            }
        }
        let mut open = vec![false; n];
        let mut last_loud: Option<usize> = None;
        for i in (0..n).rev() {
            if level[i] >= thr {
                last_loud = Some(i);
            }
            open[i] = last_loud.is_some_and(|j| j - i <= look);
        }
        let mut out = vec![floor; n];
        let (mut g, mut since_open) = (floor, usize::MAX);
        for i in 0..n {
            since_open = if open[i] { 0 } else { since_open.saturating_add(1) };
            let target = if since_open <= hold { 1.0 } else { floor };
            let c = if target > g { att } else { rel };
            g = target + (g - target) * c;
            out[i] = g;
        }
        out
    }

    pub fn process(&self, channels: &mut [Vec<f32>], sr: u32) {
        let g = self.gains(channels, sr);
        for c in channels.iter_mut() {
            for (v, g) in c.iter_mut().zip(&g) {
                *v *= g;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn silences_bleed_between_notes_and_keeps_the_attack() {
        let sr = 48_000;
        let mut x = vec![0.002f32; sr as usize]; // -54 dB of bleed
        for (i, v) in x.iter_mut().enumerate().skip(24_000).take(4_800) {
            *v = 0.5 * (i as f32 * 0.1).sin();
        }
        let mut ch = vec![x];
        Gate { on: true, ..Default::default() }.process(&mut ch, sr);
        let x = &ch[0];
        assert!(x[..20_000].iter().all(|v| v.abs() < 1e-5), "bleed gated");
        // the note's first cycle comes through at full level thanks to the lookahead
        let first = x[24_000..24_100].iter().fold(0f32, |m, v| m.max(v.abs()));
        assert!(first > 0.45, "attack kept: {first}");
        assert!(x[44_000..].iter().all(|v| v.abs() < 1e-4), "closed again after the note");
    }
}
