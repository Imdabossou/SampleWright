//! Band-limited resampling with a Kaiser-windowed sinc.
//!
//! Output sample `n` is the band-limited input evaluated at exactly
//! `n · from / to` input samples (linear phase, zero delay), so timing is
//! preserved to a small fraction of a sample. The filter follows libsoxr's
//! "HQ" recipe (what librosa and Beat This! resample with): passband to 91.3 %
//! of the lower Nyquist frequency, stopband from 100 %, ~120 dB rejection.
//! Matching it keeps the neural models' input as they were trained on.

use rayon::prelude::*;

const PASSBAND_END: f64 = 0.913;
const STOPBAND_BEGIN: f64 = 1.0;
const ATTENUATION_DB: f64 = 120.4; // 20 bits
/// Above this many polyphase rows the kernel is interpolated between rows.
const MAX_PHASES: usize = 4096;

fn gcd(a: u64, b: u64) -> u64 {
    if b == 0 { a } else { gcd(b, a % b) }
}

/// Zeroth-order modified Bessel function of the first kind.
fn bessel_i0(x: f64) -> f64 {
    let (mut sum, mut term, mut k) = (1.0, 1.0, 1.0);
    loop {
        term *= (x / (2.0 * k)).powi(2);
        sum += term;
        if term < 1e-21 * sum {
            return sum;
        }
        k += 1.0;
    }
}

fn sinc(x: f64) -> f64 {
    if x == 0.0 { 1.0 } else { (std::f64::consts::PI * x).sin() / (std::f64::consts::PI * x) }
}

struct Kernel {
    /// taps per phase: `2 * half`, for input offsets `-half + 1 ..= half`
    taps: usize,
    half: i64,
    /// rows of taps, phase `p / rows` for `p` in `0 ..= rows`
    rows: usize,
    table: Vec<f64>,
}

impl Kernel {
    fn new(from: u32, to: u32, rows: usize) -> Kernel {
        let nyquist = from.min(to) as f64 / 2.0;
        let cutoff = nyquist * (PASSBAND_END + STOPBAND_BEGIN) / 2.0 / from as f64; // cycles per input sample
        let transition = nyquist * (STOPBAND_BEGIN - PASSBAND_END) / from as f64;
        let beta = 0.1102 * (ATTENUATION_DB - 8.7);
        let length = (ATTENUATION_DB - 7.95) / (14.36 * transition);
        let half = (length / 2.0).ceil() as i64;
        let taps = 2 * half as usize;
        let i0_beta = bessel_i0(beta);
        let mut table = Vec::with_capacity((rows + 1) * taps);
        for p in 0..=rows {
            let frac = p as f64 / rows as f64;
            for j in -half + 1..=half {
                let t = frac - j as f64; // distance from output time to input sample
                let u = t / half as f64;
                let w = if u.abs() >= 1.0 { 0.0 } else { bessel_i0(beta * (1.0 - u * u).sqrt()) / i0_beta };
                table.push(2.0 * cutoff * sinc(2.0 * cutoff * t) * w);
            }
        }
        Kernel { taps, half, rows, table }
    }

    fn row(&self, p: usize) -> &[f64] {
        &self.table[p * self.taps..(p + 1) * self.taps]
    }
}

/// Resamples one channel; the output has `round(len · to / from)` samples.
pub fn resample(x: &[f32], from: u32, to: u32) -> Vec<f32> {
    let want = (x.len() as f64 * to as f64 / from as f64).round() as usize;
    if from == to {
        return x.to_vec();
    }
    let g = gcd(from as u64, to as u64);
    let (step, phases) = (from as u64 / g, to as u64 / g); // t_n = n · step / phases
    let exact = phases as usize <= MAX_PHASES;
    let kernel = Kernel::new(from, to, if exact { phases as usize } else { MAX_PHASES });
    let len = x.len() as i64;
    let mut out = vec![0f32; want];
    out.par_chunks_mut(1 << 14).enumerate().for_each(|(c, chunk)| {
        let mut mixed = vec![0f64; kernel.taps];
        for (o, y) in chunk.iter_mut().enumerate() {
            let n = (c << 14) + o;
            let pos = n as u64 * step;
            let i = (pos / phases) as i64;
            let phase = pos % phases;
            let taps: &[f64] = if exact {
                kernel.row(phase as usize)
            } else {
                let r = phase as f64 / phases as f64 * kernel.rows as f64;
                let (r0, f) = (r.floor() as usize, r.fract());
                let (a, b) = (kernel.row(r0), kernel.row(r0 + 1));
                for ((m, &a), &b) in mixed.iter_mut().zip(a).zip(b) {
                    *m = a + (b - a) * f;
                }
                &mixed
            };
            let first = i - kernel.half + 1;
            let (lo, hi) = ((-first).max(0) as usize, ((len - first).min(kernel.taps as i64)).max(0) as usize);
            let mut acc = 0.0;
            for k in lo..hi {
                acc += taps[k] * x[(first + k as i64) as usize] as f64;
            }
            *y = acc as f32;
        }
    });
    out
}
