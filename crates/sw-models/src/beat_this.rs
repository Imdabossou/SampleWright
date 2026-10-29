//! Beat This! beat and downbeat tracking.
//!
//! A port of the inference path of CPJKU/beat_this (MIT, Institute of
//! Computational Perception, JKU Linz): log-mel front end, chunked
//! prediction and the "minimal" peak-picking post-processor. The chunking
//! follows danigb/beat-this-rs (MIT). The network itself is the official
//! `final0` checkpoint exported to ONNX and run with rten.
//!
//! Input is mono audio at 22.05 kHz (resample with `sw_audio::resample`,
//! which matches the soxr resampling the model was trained with).

use std::path::Path;

use rayon::prelude::*;
use realfft::RealFftPlanner;
use rten::{Model, NodeId, ValueView};
use rten_tensor::prelude::*;

use crate::{Error, Result};

pub const SAMPLE_RATE: u32 = 22_050;
/// Prediction frames per second.
pub const FPS: f64 = 50.0;
/// File name of the default checkpoint, relative to a model directory.
pub const DEFAULT_MODEL: &str = "beat_this/final0.onnx";

const N_FFT: usize = 1024;
const HOP: usize = 441;
const N_MELS: usize = 128;
const F_MIN: f64 = 30.0;
const F_MAX: f64 = 11_000.0;
const LOG_MULTIPLIER: f64 = 1000.0;

/// Frames per chunk (30 s, the training length).
const CHUNK: usize = 1500;
/// Frames dropped at each chunk edge (the model never learnt the edges).
const BORDER: usize = 6;

/// Slaney mel scale (linear below 1 kHz, logarithmic above).
fn hz_to_mel(f: f64) -> f64 {
    let (f_sp, min_log_hz, logstep) = (200.0 / 3.0, 1000.0, (6.4f64).ln() / 27.0);
    if f >= min_log_hz { min_log_hz / f_sp + (f / min_log_hz).ln() / logstep } else { f / f_sp }
}

fn mel_to_hz(m: f64) -> f64 {
    let (f_sp, min_log_hz, logstep) = (200.0 / 3.0, 1000.0, (6.4f64).ln() / 27.0);
    let min_log_mel = min_log_hz / f_sp;
    if m >= min_log_mel { min_log_hz * (logstep * (m - min_log_mel)).exp() } else { f_sp * m }
}

/// Triangular mel filters without area normalisation (torchaudio
/// `melscale_fbanks(norm=None, mel_scale="slaney")`), `[freq][mel]`.
fn mel_filters() -> Vec<[f64; N_MELS]> {
    let n_freqs = N_FFT / 2 + 1;
    let (m_min, m_max) = (hz_to_mel(F_MIN), hz_to_mel(F_MAX));
    let f_pts: Vec<f64> = (0..N_MELS + 2).map(|i| mel_to_hz(m_min + (m_max - m_min) * i as f64 / (N_MELS + 1) as f64)).collect();
    (0..n_freqs)
        .map(|k| {
            let f = SAMPLE_RATE as f64 / 2.0 * k as f64 / (n_freqs - 1) as f64;
            let mut row = [0.0; N_MELS];
            for (m, r) in row.iter_mut().enumerate() {
                let down = (f - f_pts[m]) / (f_pts[m + 1] - f_pts[m]);
                let up = (f_pts[m + 2] - f) / (f_pts[m + 2] - f_pts[m + 1]);
                *r = down.min(up).max(0.0);
            }
            row
        })
        .collect()
}

/// Log-mel spectrogram, one 128-band row per 20 ms frame (beat_this
/// `LogMelSpect`: |STFT| of a centred, reflect-padded, periodic-Hann frame
/// normalised by sqrt(n_fft), slaney mels from 30 Hz to 11 kHz, log1p(1000 x)).
pub fn log_mel(y: &[f32]) -> Vec<[f32; N_MELS]> {
    let pad = N_FFT / 2;
    let n = y.len();
    // torch's reflect padding mirrors around the edge samples; for clips too
    // short to mirror, fall back to zero padding
    let at = |i: isize| -> f64 {
        let n = n as isize;
        let j = if i < 0 { -i } else if i >= n { 2 * (n - 1) - i } else { i };
        if (0..n).contains(&j) { y[j as usize] as f64 } else { 0.0 }
    };
    let frames = 1 + n / HOP;
    let window: Vec<f64> = (0..N_FFT).map(|i| 0.5 - 0.5 * (2.0 * std::f64::consts::PI * i as f64 / N_FFT as f64).cos()).collect();
    let filters = mel_filters();
    let plan = RealFftPlanner::<f64>::new().plan_fft_forward(N_FFT);
    let norm = 1.0 / (N_FFT as f64).sqrt();
    let mut out = vec![[0f32; N_MELS]; frames];
    out.par_chunks_mut(256).enumerate().for_each(|(c, chunk)| {
        let mut input = plan.make_input_vec();
        let mut spec = plan.make_output_vec();
        let mut mel = [0f64; N_MELS];
        for (o, row) in chunk.iter_mut().enumerate() {
            let start = ((c * 256 + o) * HOP) as isize - pad as isize;
            for (i, (x, w)) in input.iter_mut().zip(&window).enumerate() {
                *x = at(start + i as isize) * w;
            }
            // the buffers always match the plan length
            plan.process(&mut input, &mut spec).expect("fft length matches plan");
            mel.fill(0.0);
            for (s, f) in spec.iter().zip(&filters) {
                let mag = s.norm() * norm;
                for (m, w) in mel.iter_mut().zip(f) {
                    *m += mag * w;
                }
            }
            for (r, m) in row.iter_mut().zip(&mel) {
                *r = (LOG_MULTIPLIER * m).ln_1p() as f32;
            }
        }
    });
    out
}

/// Chunk start frames (beat_this `split_piece` with `avoid_short_end`).
fn chunk_starts(frames: usize) -> Vec<i64> {
    let (b, stride) = (BORDER as i64, (CHUNK - 2 * BORDER) as i64);
    let mut starts: Vec<i64> = (0..).map(|i| -b + i * stride).take_while(|&s| s < frames as i64 - b).collect();
    if frames as i64 > stride
        && let Some(last) = starts.last_mut() {
            *last = frames as i64 - (CHUNK as i64 - b);
        }
    starts
}

/// Beat and downbeat times in seconds.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct Beats {
    pub beats: Vec<f64>,
    pub downbeats: Vec<f64>,
}

/// A loaded Beat This! network.
pub struct BeatThis {
    model: Model,
    input: NodeId,
    beat: NodeId,
    downbeat: NodeId,
}

impl BeatThis {
    /// Loads an ONNX export of a Beat This! checkpoint.
    pub fn load(path: &Path) -> Result<BeatThis> {
        let err = |msg: String| Error::Load { path: path.to_path_buf(), msg };
        let model = Model::load_file(path).map_err(|e| err(e.to_string()))?;
        let node = |name: &str| model.find_node(name).ok_or_else(|| err(format!("the model has no '{name}' node")));
        let (input, beat, downbeat) = (node("spectrogram")?, node("beat")?, node("downbeat")?);
        Ok(BeatThis { model, input, beat, downbeat })
    }

    /// Loads the default checkpoint from the model directories.
    pub fn load_default() -> Result<BeatThis> {
        BeatThis::load(&crate::find_model(DEFAULT_MODEL)?)
    }

    fn run(&self, chunk: &[f32], frames: usize) -> Result<(Vec<f32>, Vec<f32>)> {
        let view = ValueView::from_shape(&[1, frames, N_MELS][..], chunk).map_err(|e| Error::Run(e.to_string()))?;
        let [beat, downbeat] =
            self.model.run_n(vec![(self.input, view.into())], [self.beat, self.downbeat], None).map_err(|e| Error::Run(e.to_string()))?;
        let take = |v: rten::Value| -> Result<Vec<f32>> {
            let t = v.into_tensor::<f32>().ok_or_else(|| Error::Run("the model output is not f32".into()))?;
            Ok(t.to_vec())
        };
        Ok((take(beat)?, take(downbeat)?))
    }

    /// Frame-wise beat and downbeat logits for a whole spectrogram, predicted
    /// in overlapping 30 s chunks (earlier chunks win in the overlaps).
    pub fn logits(&self, mel: &[[f32; N_MELS]], progress: &mut dyn FnMut(f64)) -> Result<(Vec<f32>, Vec<f32>)> {
        let n = mel.len();
        let mut beat = vec![-1000f32; n];
        let mut downbeat = vec![-1000f32; n];
        let starts = chunk_starts(n);
        for (done, &start) in starts.iter().rev().enumerate() {
            let left = (-start).max(0) as usize;
            let (from, to) = (start.max(0) as usize, ((start + CHUNK as i64) as usize).min(n));
            let right = (start + CHUNK as i64 - n as i64).clamp(0, BORDER as i64) as usize;
            let frames = left + (to - from) + right;
            let mut chunk = vec![0f32; frames * N_MELS];
            for (t, row) in mel[from..to].iter().enumerate() {
                chunk[(left + t) * N_MELS..(left + t + 1) * N_MELS].copy_from_slice(row);
            }
            let (b, d) = self.run(&chunk, frames)?;
            if b.len() != frames || d.len() != frames {
                return Err(Error::Run(format!("expected {frames} output frames, got {}", b.len())));
            }
            for t in BORDER..frames.saturating_sub(BORDER) {
                let dest = start + t as i64;
                if (0..n as i64).contains(&dest) {
                    beat[dest as usize] = b[t];
                    downbeat[dest as usize] = d[t];
                }
            }
            progress((done + 1) as f64 / starts.len() as f64);
        }
        Ok((beat, downbeat))
    }

    /// Beats and downbeats of mono 22.05 kHz audio.
    pub fn track(&self, y: &[f32], progress: &mut dyn FnMut(f64)) -> Result<Beats> {
        let mel = log_mel(y);
        let (b, d) = self.logits(&mel, progress)?;
        Ok(postprocess(&b, &d))
    }
}

/// Frames that are the maximum within +-3 frames (70 ms) and above 0.5
/// probability, with runs of adjacent frames merged to their mean.
fn peaks(logits: &[f32]) -> Vec<f64> {
    let n = logits.len();
    let mut out: Vec<f64> = Vec::new();
    let mut run: Option<(f64, f64)> = None; // (mean frame, count)
    let mut last = 0usize;
    for i in 0..n {
        let window = &logits[i.saturating_sub(3)..(i + 4).min(n)];
        let max = window.iter().copied().fold(f32::NEG_INFINITY, f32::max);
        if logits[i] != max || logits[i] <= 0.0 {
            continue;
        }
        run = match run {
            Some((mean, c)) if i - last <= 1 => Some((mean + (i as f64 - mean) / (c + 1.0), c + 1.0)),
            Some((mean, _)) => {
                out.push(mean);
                Some((i as f64, 1.0))
            }
            None => Some((i as f64, 1.0)),
        };
        last = i;
    }
    out.extend(run.map(|(m, _)| m));
    out
}

/// beat_this's "minimal" post-processor: peak picking, then each downbeat is
/// moved onto its nearest beat.
pub fn postprocess(beat_logits: &[f32], downbeat_logits: &[f32]) -> Beats {
    let beats: Vec<f64> = peaks(beat_logits).into_iter().map(|f| f / FPS).collect();
    let mut downbeats: Vec<f64> = peaks(downbeat_logits).into_iter().map(|f| f / FPS).collect();
    if !beats.is_empty() {
        for d in &mut downbeats {
            // first of equally near beats, like numpy argmin
            let nearest = beats.iter().fold(beats[0], |best, &b| if (b - *d).abs() < (best - *d).abs() { b } else { best });
            *d = nearest;
        }
    }
    downbeats.sort_by(|a, b| a.total_cmp(b));
    downbeats.dedup();
    Beats { beats, downbeats }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn chunking_matches_split_piece() {
        assert_eq!(chunk_starts(100), vec![-6]);
        assert_eq!(chunk_starts(1500), vec![-6, 6]);
        assert_eq!(chunk_starts(2000), vec![-6, 506]);
        assert_eq!(chunk_starts(5000), vec![-6, 1482, 2970, 3506]);
    }

    #[test]
    fn peak_picking_merges_adjacent_maxima() {
        let mut l = vec![-5f32; 30];
        l[5] = 2.0;
        l[10] = 1.0;
        l[11] = 1.0; // plateau: both frames are maxima, merged to 10.5
        l[13] = 0.5; // within 3 frames of a larger value? no: 11 is 1.0 > 0.5
        l[20] = -0.1; // below 0.5 probability
        assert_eq!(peaks(&l), vec![5.0, 10.5]);
    }

    #[test]
    fn downbeats_snap_to_beats() {
        let mut b = vec![-5f32; 200];
        let mut d = vec![-5f32; 200];
        for f in [10, 35, 60, 85, 110] {
            b[f] = 3.0;
        }
        d[11] = 3.0;
        d[109] = 3.0;
        let r = postprocess(&b, &d);
        assert_eq!(r.beats, vec![0.2, 0.7, 1.2, 1.7, 2.2]);
        assert_eq!(r.downbeats, vec![0.2, 2.2]);
    }

    #[test]
    fn mel_scale_round_trips() {
        for f in [30.0, 500.0, 1000.0, 4321.0, 11_000.0] {
            assert!((mel_to_hz(hz_to_mel(f)) - f).abs() < 1e-9);
        }
    }
}
