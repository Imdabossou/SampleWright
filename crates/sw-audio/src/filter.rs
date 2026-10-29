//! The filters of a region's chain: Butterworth high- and low-pass at 12, 24
//! or 48 dB per octave, and bell and notch bands.
//!
//! All are biquads (Robert Bristow-Johnson's "Audio EQ Cookbook" formulas);
//! steeper Butterworth slopes are cascades of second-order sections with the
//! Butterworth pole Qs, so the passband stays flat.

use serde::{Deserialize, Serialize};

/// Coefficients of one second-order section, normalised so a0 = 1.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Biquad {
    b0: f64,
    b1: f64,
    b2: f64,
    a1: f64,
    a2: f64,
}

impl Biquad {
    fn new(b0: f64, b1: f64, b2: f64, a0: f64, a1: f64, a2: f64) -> Biquad {
        Biquad { b0: b0 / a0, b1: b1 / a0, b2: b2 / a0, a1: a1 / a0, a2: a2 / a0 }
    }

    fn w0(freq: f64, sr: f64) -> f64 {
        // keep the design stable at the extremes
        2.0 * std::f64::consts::PI * freq.clamp(1.0, sr * 0.49) / sr
    }

    pub fn lowpass(freq: f64, q: f64, sr: f64) -> Biquad {
        let w = Self::w0(freq, sr);
        let (c, alpha) = (w.cos(), w.sin() / (2.0 * q));
        Biquad::new((1.0 - c) / 2.0, 1.0 - c, (1.0 - c) / 2.0, 1.0 + alpha, -2.0 * c, 1.0 - alpha)
    }

    pub fn highpass(freq: f64, q: f64, sr: f64) -> Biquad {
        let w = Self::w0(freq, sr);
        let (c, alpha) = (w.cos(), w.sin() / (2.0 * q));
        Biquad::new((1.0 + c) / 2.0, -(1.0 + c), (1.0 + c) / 2.0, 1.0 + alpha, -2.0 * c, 1.0 - alpha)
    }

    pub fn bell(freq: f64, q: f64, gain_db: f64, sr: f64) -> Biquad {
        let w = Self::w0(freq, sr);
        let a = 10f64.powf(gain_db / 40.0);
        let (c, alpha) = (w.cos(), w.sin() / (2.0 * q));
        Biquad::new(1.0 + alpha * a, -2.0 * c, 1.0 - alpha * a, 1.0 + alpha / a, -2.0 * c, 1.0 - alpha / a)
    }

    pub fn notch(freq: f64, q: f64, sr: f64) -> Biquad {
        let w = Self::w0(freq, sr);
        let (c, alpha) = (w.cos(), w.sin() / (2.0 * q));
        Biquad::new(1.0, -2.0 * c, 1.0, 1.0 + alpha, -2.0 * c, 1.0 - alpha)
    }

    /// Magnitude response at `freq`.
    pub fn magnitude(&self, freq: f64, sr: f64) -> f64 {
        let w = 2.0 * std::f64::consts::PI * freq / sr;
        let (c1, s1, c2, s2) = (w.cos(), w.sin(), (2.0 * w).cos(), (2.0 * w).sin());
        let num_re = self.b0 + self.b1 * c1 + self.b2 * c2;
        let num_im = -(self.b1 * s1 + self.b2 * s2);
        let den_re = 1.0 + self.a1 * c1 + self.a2 * c2;
        let den_im = -(self.a1 * s1 + self.a2 * s2);
        ((num_re * num_re + num_im * num_im) / (den_re * den_re + den_im * den_im)).sqrt()
    }

    /// Filters `x` in place (transposed direct form II, f64 state).
    pub fn process(&self, x: &mut [f32]) {
        let (mut z1, mut z2) = (0f64, 0f64);
        for v in x.iter_mut() {
            let i = *v as f64;
            let o = self.b0 * i + z1;
            z1 = self.b1 * i - self.a1 * o + z2;
            z2 = self.b2 * i - self.a2 * o;
            *v = o as f32;
        }
    }
}

/// Steepness of a high- or low-pass.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub enum Slope {
    Db12,
    #[default]
    Db24,
    Db48,
}

impl Slope {
    pub const ALL: [Slope; 3] = [Slope::Db12, Slope::Db24, Slope::Db48];

    pub fn label(self) -> &'static str {
        match self {
            Slope::Db12 => "12 dB/oct",
            Slope::Db24 => "24 dB/oct",
            Slope::Db48 => "48 dB/oct",
        }
    }

    fn order(self) -> usize {
        match self {
            Slope::Db12 => 2,
            Slope::Db24 => 4,
            Slope::Db48 => 8,
        }
    }

    /// Qs of the second-order sections of a Butterworth filter of this order.
    fn qs(self) -> Vec<f64> {
        let n = self.order();
        (1..=n / 2).map(|k| 1.0 / (2.0 * ((2 * k - 1) as f64 * std::f64::consts::PI / (2 * n) as f64).cos())).collect()
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct Pass {
    pub on: bool,
    pub freq: f32,
    pub slope: Slope,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub enum BandKind {
    #[default]
    Bell,
    Notch,
}

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct Band {
    pub on: bool,
    pub kind: BandKind,
    pub freq: f32,
    pub gain_db: f32,
    pub q: f32,
}

/// The filter part of a region's chain.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Filters {
    pub highpass: Pass,
    pub lowpass: Pass,
    pub bands: [Band; 2],
}

impl Default for Filters {
    fn default() -> Filters {
        let band = |freq| Band { on: false, kind: BandKind::Bell, freq, gain_db: -6.0, q: 2.0 };
        Filters {
            highpass: Pass { on: false, freq: 40.0, slope: Slope::Db24 },
            lowpass: Pass { on: false, freq: 12_000.0, slope: Slope::Db24 },
            bands: [band(250.0), band(3_000.0)],
        }
    }
}

impl Filters {
    pub fn is_active(&self) -> bool {
        self.highpass.on || self.lowpass.on || self.bands.iter().any(|b| b.on)
    }

    /// The sections this setting runs, in order.
    pub fn sections(&self, sr: f64) -> Vec<Biquad> {
        let mut v = Vec::new();
        if self.highpass.on {
            v.extend(self.highpass.slope.qs().into_iter().map(|q| Biquad::highpass(self.highpass.freq as f64, q, sr)));
        }
        if self.lowpass.on {
            v.extend(self.lowpass.slope.qs().into_iter().map(|q| Biquad::lowpass(self.lowpass.freq as f64, q, sr)));
        }
        for b in self.bands.iter().filter(|b| b.on) {
            let q = (b.q as f64).clamp(0.1, 40.0);
            v.push(match b.kind {
                BandKind::Bell => Biquad::bell(b.freq as f64, q, b.gain_db as f64, sr),
                BandKind::Notch => Biquad::notch(b.freq as f64, q, sr),
            });
        }
        v
    }

    /// Combined magnitude response in dB at `freq`.
    pub fn response_db(&self, freq: f64, sr: f64) -> f64 {
        let m: f64 = self.sections(sr).iter().map(|s| s.magnitude(freq, sr)).product();
        20.0 * m.max(1e-9).log10()
    }

    /// Filters one channel in place.
    pub fn process(&self, x: &mut [f32], sr: u32) {
        for s in self.sections(sr as f64) {
            s.process(x);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rms(x: &[f32]) -> f64 {
        (x.iter().map(|v| (*v as f64).powi(2)).sum::<f64>() / x.len() as f64).sqrt()
    }

    fn sine(f: f64, sr: u32) -> Vec<f32> {
        (0..sr).map(|i| (2.0 * std::f64::consts::PI * f * i as f64 / sr as f64).sin() as f32).collect()
    }

    #[test]
    fn butterworth_is_3db_down_at_cutoff_and_slopes_as_promised() {
        let sr = 48_000.0;
        for slope in Slope::ALL {
            let f = Filters { highpass: Pass { on: true, freq: 1000.0, slope }, ..Default::default() };
            assert!((f.response_db(1000.0, sr) + 3.01).abs() < 0.05, "{slope:?} at cutoff");
            assert!(f.response_db(10_000.0, sr).abs() < 0.05, "{slope:?} passband");
            // one octave further down the stopband loses (order * 6) dB more
            let per_octave = f.response_db(125.0, sr) - f.response_db(250.0, sr);
            let want = -(slope.order() as f64) * 6.02;
            assert!((per_octave - want).abs() < 0.5, "{slope:?}: {per_octave} dB/oct");
        }
    }

    #[test]
    fn highpass_removes_kick_and_keeps_bass_note() {
        let sr = 44_100;
        let f = Filters { highpass: Pass { on: true, freq: 70.0, slope: Slope::Db48 }, ..Default::default() };
        let mut kick = sine(35.0, sr);
        let mut note = sine(196.0, sr);
        f.process(&mut kick, sr);
        f.process(&mut note, sr);
        let tail = (sr / 2) as usize; // past the filter's start-up
        assert!(rms(&kick[tail..]) < 0.01);
        assert!((rms(&note[tail..]) - 0.707).abs() < 0.02);
    }

    #[test]
    fn notch_kills_its_frequency_only() {
        let sr = 48_000.0;
        let b = Biquad::notch(3000.0, 4.0, sr);
        assert!(b.magnitude(3000.0, sr) < 1e-6);
        assert!((b.magnitude(300.0, sr) - 1.0).abs() < 0.01);
        let bell = Biquad::bell(500.0, 1.0, -9.0, sr);
        assert!((20.0 * bell.magnitude(500.0, sr).log10() + 9.0).abs() < 0.01);
    }
}
