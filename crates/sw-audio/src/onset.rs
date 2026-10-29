//! Onset detection: where the notes and hits of a stem start, and how long
//! each one rings.
//!
//! Spectral flux of a log-compressed spectrum (each frame's rise in energy,
//! summed over frequency), peak-picked against a moving average. Each peak is
//! moved back to the quiet point just before the attack, so slices start
//! before the transient rather than in it.

use realfft::RealFftPlanner;

const FRAME: usize = 2048;
const HOP: usize = 256;

/// A detected note or hit, in frames of the analysed audio.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Onset {
    pub start: usize,
    pub end: usize,
    /// Strength of the attack, 0..1 relative to the strongest in the audio.
    pub strength: f32,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Settings {
    /// 0 (only the strongest attacks) .. 1 (every small bump).
    pub sensitivity: f32,
    /// Two onsets are at least this far apart.
    pub min_gap_ms: f32,
    /// A note ends when it decays this far below its peak (dB, positive)...
    pub decay_db: f32,
    /// ...or at the next onset, or after this long.
    pub max_len_ms: f32,
}

impl Default for Settings {
    fn default() -> Settings {
        Settings { sensitivity: 0.5, min_gap_ms: 60.0, decay_db: 40.0, max_len_ms: 4_000.0 }
    }
}

/// Onset strength per hop (spectral flux, normalised to 0..1).
pub fn novelty(x: &[f32]) -> Vec<f32> {
    let window: Vec<f32> = (0..FRAME).map(|i| (0.5 - 0.5 * (2.0 * std::f64::consts::PI * i as f64 / FRAME as f64).cos()) as f32).collect();
    let fft = RealFftPlanner::<f32>::new().plan_fft_forward(FRAME);
    let mut input = fft.make_input_vec();
    let mut spec = fft.make_output_vec();
    let frames = x.len() / HOP + 1;
    let mut prev = vec![0f32; FRAME / 2 + 1];
    let mut flux = Vec::with_capacity(frames);
    for f in 0..frames {
        // frames are centred on their hop position
        let centre = (f * HOP) as isize;
        for (i, v) in input.iter_mut().enumerate() {
            let j = centre - (FRAME / 2) as isize + i as isize;
            *v = if j >= 0 && (j as usize) < x.len() { x[j as usize] * window[i] } else { 0.0 };
        }
        if fft.process(&mut input, &mut spec).is_err() {
            flux.push(0.0);
            continue;
        }
        let mut sum = 0f32;
        for (k, c) in spec.iter().enumerate() {
            let m = (1.0 + 100.0 * c.norm()).ln();
            sum += (m - prev[k]).max(0.0);
            prev[k] = m;
        }
        flux.push(sum);
    }
    // the first frame rises from silence by definition
    if let Some(f) = flux.first_mut() {
        *f = 0.0;
    }
    let max = flux.iter().copied().fold(0f32, f32::max).max(1e-9);
    flux.iter().map(|v| v / max).collect()
}

/// RMS envelope over `HOP`-sized blocks.
fn envelope(x: &[f32]) -> Vec<f32> {
    x.chunks(HOP).map(|c| (c.iter().map(|v| v * v).sum::<f32>() / c.len() as f32).sqrt()).collect()
}

/// Finds the onsets of mono audio at `sr`.
pub fn detect(x: &[f32], sr: u32, s: &Settings) -> Vec<Onset> {
    if x.len() < HOP * 4 {
        return Vec::new();
    }
    let nov = novelty(x);
    let fps = sr as f32 / HOP as f32;
    let avg_w = (0.1 * fps) as usize; // 100 ms either side
    let gap = ((s.min_gap_ms / 1000.0 * fps) as usize).max(1);
    // sensitivity 1 -> threshold 0.02 above the local mean, 0 -> 0.4
    let delta = 0.4 * (1.0 - s.sensitivity.clamp(0.0, 1.0)).powi(2) + 0.02;
    let mut peaks: Vec<usize> = Vec::new();
    for i in 1..nov.len().saturating_sub(1) {
        let (a, b) = (i.saturating_sub(avg_w), (i + avg_w + 1).min(nov.len()));
        let mean = nov[a..b].iter().sum::<f32>() / (b - a) as f32;
        let local_max = nov[i.saturating_sub(3)..(i + 4).min(nov.len())].iter().all(|&v| v <= nov[i]);
        if local_max && nov[i] >= mean + delta {
            match peaks.last() {
                Some(&p) if i - p < gap => {
                    if nov[i] > nov[p] {
                        *peaks.last_mut().expect("non-empty") = i;
                    }
                }
                _ => peaks.push(i),
            }
        }
    }

    let env = envelope(x);
    let max_len = (s.max_len_ms / 1000.0 * sr as f32) as usize;
    let decay = crate::db_to_gain(-s.decay_db.abs());
    let starts: Vec<usize> = peaks
        .iter()
        .map(|&p| {
            // back to the quiet point before the attack: the last block, in the
            // 50 ms before the flux peak, that is near that stretch's minimum
            let hi = p.min(env.len().saturating_sub(1));
            let lo = p.saturating_sub((0.05 * fps) as usize).min(hi);
            let w = &env[lo..=hi];
            let (min, max) = (w.iter().copied().fold(f32::MAX, f32::min), w.iter().copied().fold(0f32, f32::max));
            let quiet = min + 0.05 * (max - min);
            let hop = (lo..=hi).rev().find(|&h| env[h] <= quiet).unwrap_or(lo);
            hop * HOP
        })
        .collect();
    let mut out = Vec::with_capacity(starts.len());
    for (k, &start) in starts.iter().enumerate() {
        let next = starts.get(k + 1).copied().unwrap_or(x.len()).min(x.len());
        let limit = next.min(start + max_len);
        let (h0, h1) = (start / HOP, limit.div_ceil(HOP).min(env.len()));
        let peak = env[h0..h1].iter().copied().fold(0f32, f32::max);
        let peak_at = env[h0..h1].iter().position(|&v| v == peak).unwrap_or(0) + h0;
        let end = (peak_at..h1).find(|&h| env[h] < peak * decay).map_or(limit, |h| (h * HOP).min(limit));
        if end > start {
            out.push(Onset { start, end, strength: nov[peaks[k]] });
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Plucked notes: decaying sines at the given times.
    fn plucks(times: &[f32], sr: u32, len: f32) -> Vec<f32> {
        let mut x = vec![0f32; (len * sr as f32) as usize];
        for (k, &t) in times.iter().enumerate() {
            let a = (t * sr as f32) as usize;
            let f = 110.0 * (1.0 + k as f32 * 0.25);
            for i in 0..(sr as usize).min(x.len() - a) {
                let tt = i as f32 / sr as f32;
                x[a + i] += 0.6 * (2.0 * std::f32::consts::PI * f * tt).sin() * (-tt * 6.0).exp();
            }
        }
        x
    }

    #[test]
    fn finds_each_pluck_at_the_right_time() {
        let sr = 44_100;
        let times = [0.25, 0.75, 1.25, 1.5, 2.2];
        let x = plucks(&times, sr, 3.0);
        let found = detect(&x, sr, &Settings::default());
        assert_eq!(found.len(), times.len(), "{found:?}");
        for (o, t) in found.iter().zip(times) {
            let at = o.start as f32 / sr as f32;
            assert!(at <= t + 0.003 && at > t - 0.03, "onset at {at}, want {t}");
            assert!(o.end > o.start);
        }
        // each note stops at the next one
        assert!(found[2].end <= found[3].start);
    }
}
