//! Audio for SampleWright.
//!
//! * [`decode`] any common format (mp3, flac, ogg/vorbis, wav, aiff, m4a/aac, alac)
//! * [`wav`] write 16-bit, 24-bit or 32-bit float WAV files
//! * [`filter`], [`gate`] and [`edit`]: the per-region processing chain
//! * [`onset`] and [`pitch`]: finding and naming the notes and hits to slice
//! * [`peaks`]: waveform overviews for drawing

pub mod edit;
pub mod filter;
pub mod gate;
pub mod onset;
pub mod peaks;
pub mod pitch;
pub mod resample;
pub mod spectrum;
pub mod wav;

use std::path::Path;

use symphonia::core::audio::SampleBuffer;
use symphonia::core::codecs::DecoderOptions;
use symphonia::core::errors::Error as SymError;
use symphonia::core::formats::FormatOptions;
use symphonia::core::io::MediaSourceStream;
use symphonia::core::meta::MetadataOptions;
use symphonia::core::probe::Hint;

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("I/O error: {0}")]
    Io(#[from] std::io::Error),
    #[error("could not decode the audio: {0}")]
    Decode(String),
    #[error("the audio is empty")]
    Empty,
}

pub type Result<T> = std::result::Result<T, Error>;

/// Planar float audio (one Vec per channel), samples in -1.0..1.0.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct Audio {
    pub sample_rate: u32,
    pub channels: Vec<Vec<f32>>,
}

impl Audio {
    pub fn new(sample_rate: u32, channels: Vec<Vec<f32>>) -> Audio {
        Audio { sample_rate, channels }
    }

    pub fn frames(&self) -> usize {
        self.channels.first().map_or(0, Vec::len)
    }

    pub fn duration(&self) -> f64 {
        self.frames() as f64 / self.sample_rate.max(1) as f64
    }

    /// Mono mix (for analysis).
    pub fn mono(&self) -> Vec<f32> {
        let n = self.channels.len().max(1) as f32;
        (0..self.frames()).map(|i| self.channels.iter().map(|c| c[i]).sum::<f32>() / n).collect()
    }

    /// Exactly two channels: mono is doubled, surround keeps front left/right.
    pub fn to_stereo(&self) -> Result<Audio> {
        let channels = match self.channels.len() {
            0 => return Err(Error::Empty),
            1 => vec![self.channels[0].clone(), self.channels[0].clone()],
            _ => vec![self.channels[0].clone(), self.channels[1].clone()],
        };
        Ok(Audio { sample_rate: self.sample_rate, channels })
    }

    /// Band-limited resampling with exact timing (see [`resample`](mod@resample)).
    pub fn resample(&self, rate: u32) -> Audio {
        if rate == self.sample_rate {
            return self.clone();
        }
        use rayon::prelude::*;
        let channels = self.channels.par_iter().map(|c| resample::resample(c, self.sample_rate, rate)).collect();
        Audio { sample_rate: rate, channels }
    }

    /// Frames `from..to` (clamped).
    pub fn slice(&self, from: usize, to: usize) -> Audio {
        let n = self.frames();
        let (a, b) = (from.min(n), to.min(n).max(from.min(n)));
        Audio { sample_rate: self.sample_rate, channels: self.channels.iter().map(|c| c[a..b].to_vec()).collect() }
    }

    /// Adds `other` (same rate and channel count) scaled by `gain`, sample by sample.
    pub fn mix_in(&mut self, other: &Audio, gain: f32) {
        if self.channels.is_empty() {
            *self = Audio { sample_rate: other.sample_rate, channels: other.channels.iter().map(|c| c.iter().map(|v| v * gain).collect()).collect() };
            return;
        }
        for (d, s) in self.channels.iter_mut().zip(&other.channels) {
            if d.len() < s.len() {
                d.resize(s.len(), 0.0);
            }
            for (d, s) in d.iter_mut().zip(s) {
                *d += s * gain;
            }
        }
    }

    /// Largest absolute sample.
    pub fn peak(&self) -> f32 {
        self.channels.iter().flatten().fold(0f32, |m, v| m.max(v.abs()))
    }
}

/// Decodes an audio file (format detected from its contents and extension).
pub fn decode(path: impl AsRef<Path>) -> Result<Audio> {
    let path = path.as_ref();
    let file = std::fs::File::open(path)?;
    let mss = MediaSourceStream::new(Box::new(file), Default::default());
    let mut hint = Hint::new();
    if let Some(ext) = path.extension().and_then(|e| e.to_str()) {
        hint.with_extension(ext);
    }
    let probed = symphonia::default::get_probe()
        // gapless: trim MP3/AAC encoder delay and padding, so cut points line up
        // with the song as other players play it
        .format(&hint, mss, &FormatOptions { enable_gapless: true, ..Default::default() }, &MetadataOptions::default())
        .map_err(|e| Error::Decode(e.to_string()))?;
    let mut format = probed.format;
    let track = format.default_track().ok_or_else(|| Error::Decode("no audio track".into()))?;
    let track_id = track.id;
    let mut decoder = symphonia::default::get_codecs()
        .make(&track.codec_params, &DecoderOptions::default())
        .map_err(|e| Error::Decode(e.to_string()))?;
    let mut sample_rate = track.codec_params.sample_rate.unwrap_or(44_100);
    let mut channels: Vec<Vec<f32>> = Vec::new();

    loop {
        let packet = match format.next_packet() {
            Ok(p) => p,
            Err(SymError::IoError(e)) if e.kind() == std::io::ErrorKind::UnexpectedEof => break,
            Err(SymError::ResetRequired) => break,
            Err(e) => return Err(Error::Decode(e.to_string())),
        };
        if packet.track_id() != track_id {
            continue;
        }
        let decoded = match decoder.decode(&packet) {
            Ok(d) => d,
            Err(SymError::DecodeError(_)) => continue, // skip a corrupt frame
            Err(e) => return Err(Error::Decode(e.to_string())),
        };
        let spec = *decoded.spec();
        sample_rate = spec.rate;
        let n = spec.channels.count();
        if channels.is_empty() {
            channels = vec![Vec::new(); n];
        }
        let mut buf = SampleBuffer::<f32>::new(decoded.capacity() as u64, spec);
        buf.copy_interleaved_ref(decoded);
        for frame in buf.samples().chunks_exact(n) {
            for (c, s) in channels.iter_mut().zip(frame) {
                c.push(*s);
            }
        }
    }
    if channels.is_empty() || channels[0].is_empty() {
        return Err(Error::Empty);
    }
    Ok(Audio { sample_rate, channels })
}

/// Converts a level in dB to a linear gain.
pub fn db_to_gain(db: f32) -> f32 {
    10f32.powf(db / 20.0)
}

/// Converts a linear gain to dB (-inf for 0).
pub fn gain_to_db(g: f32) -> f32 {
    20.0 * g.max(1e-12).log10()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn decodes_what_wav_writes() {
        let sr = 44_100;
        let tone: Vec<f32> = (0..sr).map(|i| (i as f32 * 0.05).sin() * 0.5).collect();
        let a = Audio::new(sr, vec![tone.clone(), tone.iter().map(|v| -v).collect()]);
        let dir = std::env::temp_dir().join(format!("sw-audio-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        for fmt in [wav::Format::Pcm16, wav::Format::Pcm24, wav::Format::Float32] {
            let p = dir.join(format!("{fmt:?}.wav"));
            wav::write(&p, &a, fmt).unwrap();
            let b = decode(&p).unwrap();
            assert_eq!(b.sample_rate, sr);
            assert_eq!(b.frames(), a.frames());
            let tol = match fmt {
                wav::Format::Pcm16 => 1e-4,
                wav::Format::Pcm24 => 1e-6,
                wav::Format::Float32 => 0.0,
            };
            let err = a.channels.iter().flatten().zip(b.channels.iter().flatten()).fold(0f32, |m, (x, y)| m.max((x - y).abs()));
            assert!(err <= tol, "{fmt:?}: {err}");
        }
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn db_round_trip() {
        assert!((db_to_gain(-6.0) - 0.501).abs() < 1e-3);
        assert!((gain_to_db(db_to_gain(-12.5)) + 12.5).abs() < 1e-4);
    }
}
