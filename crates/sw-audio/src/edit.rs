//! Sample edits: fades, gain, normalising, seamless loop crossfades and
//! zero-crossing search.

/// Fade shape.
fn curve(t: f32) -> f32 {
    // equal-power (quarter sine): no dip in loudness half way through a crossfade
    (t.clamp(0.0, 1.0) * std::f32::consts::FRAC_PI_2).sin()
}

/// Fades the first `fade_in` and last `fade_out` frames.
pub fn fade(channels: &mut [Vec<f32>], fade_in: usize, fade_out: usize) {
    for c in channels.iter_mut() {
        let n = c.len();
        let fi = fade_in.min(n);
        for (i, v) in c.iter_mut().enumerate().take(fi) {
            *v *= curve(i as f32 / fi as f32);
        }
        let fo = fade_out.min(n);
        for i in 0..fo {
            c[n - 1 - i] *= curve(i as f32 / fo as f32);
        }
    }
}

pub fn gain(channels: &mut [Vec<f32>], g: f32) {
    if g != 1.0 {
        channels.iter_mut().flatten().for_each(|v| *v *= g);
    }
}

/// Scales so the loudest sample is at `peak_db` dBFS (silence stays silent).
pub fn normalize(channels: &mut [Vec<f32>], peak_db: f32) {
    let peak = channels.iter().flatten().fold(0f32, |m, v| m.max(v.abs()));
    if peak > 1e-6 {
        gain(channels, crate::db_to_gain(peak_db) / peak);
    }
}

/// Makes `x[start..end]` loop without a click while keeping its exact length
/// (so a bar-long loop stays in time in a DAW): the last `xfade` frames are
/// blended into the audio that led into `start`, so the end flows into the
/// beginning exactly as the song did. Returns the loop.
///
/// With less than `xfade` frames of audio before `start`, the crossfade is
/// shortened to what is there.
pub fn seamless_loop(x: &[f32], start: usize, end: usize, xfade: usize) -> Vec<f32> {
    let end = end.min(x.len());
    let start = start.min(end);
    let mut out = x[start..end].to_vec();
    let xf = xfade.min(start).min(out.len() / 2);
    let n = out.len();
    for i in 0..xf {
        let t = (i + 1) as f32 / (xf + 1) as f32;
        let tail = x[end - xf + i];
        let lead = x[start - xf + i];
        out[n - xf + i] = tail * curve(1.0 - t) + lead * curve(t);
    }
    out
}

/// The zero crossing nearest to `at` within `radius` frames (rising or
/// falling, in the summed channels), or `at` if there is none.
pub fn nearest_zero_crossing(channels: &[Vec<f32>], at: usize, radius: usize) -> usize {
    let n = channels.first().map_or(0, Vec::len);
    if n < 2 {
        return at;
    }
    let sum = |i: usize| channels.iter().map(|c| c[i]).sum::<f32>();
    let crosses = |i: usize| i + 1 < n && (sum(i) <= 0.0) != (sum(i + 1) <= 0.0);
    for d in 0..=radius {
        if at >= d && crosses(at - d) {
            return at - d + 1;
        }
        if at + d < n && crosses(at + d) {
            return at + d + 1;
        }
    }
    at.min(n)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn loop_keeps_length_and_joins_smoothly() {
        // a sine whose period does not divide the loop: a plain cut clicks
        let x: Vec<f32> = (0..20_000).map(|i| (i as f32 * 0.0137).sin()).collect();
        let (a, b) = (5_000, 15_000);
        let plain = &x[a..b];
        let jump_plain = (plain[plain.len() - 1] - plain[0]).abs();
        let l = seamless_loop(&x, a, b, 256);
        assert_eq!(l.len(), b - a);
        let jump = (l[l.len() - 1] - l[0]).abs();
        assert!(jump < 0.02 && jump_plain > 0.1, "{jump} vs {jump_plain}");
        // untouched away from the end
        assert_eq!(&l[..9_000], &x[a..a + 9_000]);
    }

    #[test]
    fn finds_zero_crossings_and_fades() {
        let x: Vec<f32> = (0..1000).map(|i| (i as f32 * std::f32::consts::PI / 100.0).sin()).collect();
        let z = nearest_zero_crossing(std::slice::from_ref(&x), 230, 50);
        assert!((199..=201).contains(&z), "{z}");
        let mut c = vec![vec![1.0f32; 100]];
        fade(&mut c, 10, 10);
        assert_eq!(c[0][0], 0.0);
        assert_eq!(c[0][50], 1.0);
        assert!(c[0][99] < 0.01);
        normalize(&mut c, -6.0);
        assert!((c[0][50] - 0.501).abs() < 1e-3);
    }
}
