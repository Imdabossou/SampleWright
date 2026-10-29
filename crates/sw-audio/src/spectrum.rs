//! Average spectrum of a region, drawn behind the filter curve so you can see
//! where the bleed sits.

use realfft::RealFftPlanner;

pub const SIZE: usize = 4096;

/// Average magnitude (dB, 0 dB = full-scale sine) per FFT bin of up to
/// `max_frames` frames spread over `x`. Bin `k` is at `k * sr / SIZE` Hz.
pub fn average_db(x: &[f32], max_frames: usize) -> Vec<f32> {
    let fft = RealFftPlanner::<f32>::new().plan_fft_forward(SIZE);
    let window: Vec<f32> = (0..SIZE).map(|i| (0.5 - 0.5 * (2.0 * std::f64::consts::PI * i as f64 / SIZE as f64).cos()) as f32).collect();
    let mut input = fft.make_input_vec();
    let mut out = fft.make_output_vec();
    let mut acc = vec![0f64; SIZE / 2 + 1];
    let n = if x.len() > SIZE { ((x.len() - SIZE) / (SIZE / 2) + 1).min(max_frames.max(1)) } else { 1 };
    for f in 0..n {
        let at = if n > 1 { (x.len() - SIZE) * f / (n - 1) } else { 0 };
        for (i, v) in input.iter_mut().enumerate() {
            *v = x.get(at + i).copied().unwrap_or(0.0) * window[i];
        }
        if fft.process(&mut input, &mut out).is_ok() {
            for (a, c) in acc.iter_mut().zip(&out) {
                *a += c.norm() as f64;
            }
        }
    }
    // a Hann-windowed full-scale sine peaks at SIZE / 4
    let scale = 4.0 / SIZE as f64 / n as f64;
    acc.iter().map(|a| (20.0 * (a * scale).max(1e-9).log10()) as f32).collect()
}
