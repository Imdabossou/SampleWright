//! Pitch of a single note, for naming one-shots (`bass_E1_003.wav`).
//!
//! YIN (de Cheveigné and Kawahara, 2002): the cumulative-mean-normalised
//! difference function, absolute threshold and parabolic interpolation.
//! It is run on a few frames of the note's body and the median taken.

const NAMES: [&str; 12] = ["C", "C#", "D", "D#", "E", "F", "F#", "G", "G#", "A", "A#", "B"];

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Pitch {
    pub hz: f32,
    /// 0..1, how periodic the frame was (1 - the YIN dip).
    pub confidence: f32,
}

impl Pitch {
    /// Nearest MIDI note number.
    pub fn midi(&self) -> i32 {
        (69.0 + 12.0 * (self.hz / 440.0).log2()).round() as i32
    }

    /// Note name with octave, e.g. "E1", "C#3" (middle C is C4).
    pub fn name(&self) -> String {
        note_name(self.midi())
    }
}

pub fn note_name(midi: i32) -> String {
    format!("{}{}", NAMES[midi.rem_euclid(12) as usize], midi.div_euclid(12) - 1)
}

/// YIN on one frame. `fmin`/`fmax` bound the search.
pub fn yin(frame: &[f32], sr: u32, fmin: f32, fmax: f32) -> Option<Pitch> {
    let tau_max = ((sr as f32 / fmin) as usize).min(frame.len() / 2);
    let tau_min = ((sr as f32 / fmax) as usize).max(2);
    if tau_max <= tau_min + 2 {
        return None;
    }
    let w = frame.len() - tau_max;
    let mut d = vec![0f32; tau_max + 1];
    for (tau, dv) in d.iter_mut().enumerate().skip(1) {
        *dv = (0..w).map(|j| (frame[j] - frame[j + tau]).powi(2)).sum();
    }
    // cumulative mean normalisation
    let mut cmnd = vec![1f32; tau_max + 1];
    let mut running = 0f32;
    for tau in 1..=tau_max {
        running += d[tau];
        cmnd[tau] = if running > 0.0 { d[tau] * tau as f32 / running } else { 1.0 };
    }
    const THRESHOLD: f32 = 0.15;
    let mut best = None;
    let mut tau = tau_min;
    while tau < tau_max {
        if cmnd[tau] < THRESHOLD {
            while tau + 1 < tau_max && cmnd[tau + 1] < cmnd[tau] {
                tau += 1;
            }
            best = Some(tau);
            break;
        }
        tau += 1;
    }
    // no dip below the threshold: take the global minimum, with low confidence
    let tau = best.unwrap_or_else(|| (tau_min..tau_max).min_by(|&a, &b| cmnd[a].total_cmp(&cmnd[b])).unwrap_or(tau_min));
    let (a, b, c) = (cmnd[tau - 1], cmnd[tau], cmnd[(tau + 1).min(tau_max)]);
    let denom = a - 2.0 * b + c;
    let shift = if denom.abs() > 1e-9 { (0.5 * (a - c) / denom).clamp(-1.0, 1.0) } else { 0.0 };
    let period = tau as f32 + shift;
    Some(Pitch { hz: sr as f32 / period, confidence: (1.0 - b).clamp(0.0, 1.0) })
}

/// The pitch of a note (mono samples from its start), or None if it is not
/// clearly pitched (drum hits, noise).
pub fn note_pitch(x: &[f32], sr: u32) -> Option<Pitch> {
    // 4096 frames reach down to ~22 Hz at 44.1 kHz (low B on a 5-string bass is 31 Hz)
    let frame = 4096.min(x.len());
    if frame < 512 {
        return None;
    }
    // skip the attack, then look at up to five frames of the body
    let skip = ((0.03 * sr as f32) as usize).min(x.len() - frame);
    let span = x.len() - frame - skip;
    let mut found: Vec<Pitch> = (0..5)
        .map(|k| skip + span * k / 4)
        .filter_map(|at| yin(&x[at..at + frame], sr, 25.0, 4200.0))
        .filter(|p| p.confidence > 0.8)
        .collect();
    if found.len() < 2 && !(found.len() == 1 && span == 0) {
        return None;
    }
    found.sort_by(|a, b| a.hz.total_cmp(&b.hz));
    Some(found[found.len() / 2])
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tone(hz: f32, sr: u32, secs: f32) -> Vec<f32> {
        (0..(sr as f32 * secs) as usize)
            .map(|i| {
                let t = i as f32 / sr as f32;
                let p = 2.0 * std::f32::consts::PI * hz * t;
                // bass-like: strong harmonics
                (p.sin() + 0.6 * (2.0 * p).sin() + 0.3 * (3.0 * p).sin()) * (-t * 2.0).exp()
            })
            .collect()
    }

    #[test]
    fn names_bass_and_guitar_notes() {
        let sr = 44_100;
        for (hz, name) in [(41.2, "E1"), (55.0, "A1"), (98.0, "G2"), (329.63, "E4"), (277.18, "C#4")] {
            let p = note_pitch(&tone(hz, sr, 0.8), sr).unwrap_or_else(|| panic!("{hz} Hz not found"));
            assert_eq!(p.name(), name, "{hz} Hz -> {} Hz", p.hz);
            assert!((p.hz - hz).abs() / hz < 0.01);
        }
    }

    #[test]
    fn noise_is_unpitched() {
        let mut s = 1u32;
        let noise: Vec<f32> = (0..30_000)
            .map(|_| {
                s ^= s << 13;
                s ^= s >> 17;
                s ^= s << 5;
                s as f32 / u32::MAX as f32 - 0.5
            })
            .collect();
        assert!(note_pitch(&noise, 44_100).is_none());
        assert_eq!(note_name(60), "C4");
        assert_eq!(note_name(28), "E1");
    }
}
