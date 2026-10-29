//! WAV writing: 16-bit and 24-bit PCM (with TPDF dither for 16-bit) and
//! 32-bit float, the formats every DAW and sampler reads.

use std::io::Write;
use std::path::Path;

use serde::{Deserialize, Serialize};

use crate::{Audio, Result};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub enum Format {
    Pcm16,
    #[default]
    Pcm24,
    Float32,
}

impl Format {
    pub const ALL: [Format; 3] = [Format::Pcm16, Format::Pcm24, Format::Float32];

    pub fn label(self) -> &'static str {
        match self {
            Format::Pcm16 => "16-bit",
            Format::Pcm24 => "24-bit",
            Format::Float32 => "32-bit float",
        }
    }

    fn bytes(self) -> u16 {
        match self {
            Format::Pcm16 => 2,
            Format::Pcm24 => 3,
            Format::Float32 => 4,
        }
    }
}

/// The WAV file as bytes.
pub fn encode(audio: &Audio, format: Format) -> Vec<u8> {
    let ch = audio.channels.len().max(1) as u16;
    let frames = audio.frames();
    let bps = format.bytes();
    let data_len = frames * ch as usize * bps as usize;
    let float = format == Format::Float32;
    // WAVE_FORMAT_IEEE_FLOAT needs the extended fmt chunk size (18) and a fact chunk
    let fmt_len: u32 = if float { 18 } else { 16 };
    let fact_len: u32 = if float { 12 } else { 0 };
    let mut w = Vec::with_capacity(44 + data_len + 16);
    w.extend_from_slice(b"RIFF");
    w.extend_from_slice(&(4 + 8 + fmt_len + fact_len + 8 + data_len as u32).to_le_bytes());
    w.extend_from_slice(b"WAVEfmt ");
    w.extend_from_slice(&fmt_len.to_le_bytes());
    w.extend_from_slice(&(if float { 3u16 } else { 1u16 }).to_le_bytes());
    w.extend_from_slice(&ch.to_le_bytes());
    w.extend_from_slice(&audio.sample_rate.to_le_bytes());
    w.extend_from_slice(&(audio.sample_rate * ch as u32 * bps as u32).to_le_bytes());
    w.extend_from_slice(&(ch * bps).to_le_bytes());
    w.extend_from_slice(&(bps * 8).to_le_bytes());
    if float {
        w.extend_from_slice(&0u16.to_le_bytes());
        w.extend_from_slice(b"fact");
        w.extend_from_slice(&4u32.to_le_bytes());
        w.extend_from_slice(&(frames as u32).to_le_bytes());
    }
    w.extend_from_slice(b"data");
    w.extend_from_slice(&(data_len as u32).to_le_bytes());
    let mut rng = 0x2545_F491u32;
    let mut noise = || {
        rng ^= rng << 13;
        rng ^= rng >> 17;
        rng ^= rng << 5;
        rng as f32 / u32::MAX as f32
    };
    for i in 0..frames {
        for c in &audio.channels {
            let s = c[i];
            match format {
                Format::Float32 => w.extend_from_slice(&s.to_le_bytes()),
                Format::Pcm16 => {
                    // triangular dither of +-1 LSB hides the truncation distortion of quiet tails
                    let d = noise() - noise();
                    let v = (s * 32767.0 + d).round().clamp(-32768.0, 32767.0) as i16;
                    w.extend_from_slice(&v.to_le_bytes());
                }
                Format::Pcm24 => {
                    let v = (s as f64 * 8_388_607.0).round().clamp(-8_388_608.0, 8_388_607.0) as i32;
                    w.extend_from_slice(&v.to_le_bytes()[..3]);
                }
            }
        }
    }
    w
}

/// Writes a WAV file (through a temporary file, so a failed export never
/// leaves half a sample behind).
pub fn write(path: &Path, audio: &Audio, format: Format) -> Result<()> {
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)?;
    }
    let tmp = path.with_extension("wav.part");
    let mut f = std::fs::File::create(&tmp)?;
    f.write_all(&encode(audio, format))?;
    f.sync_all()?;
    drop(f);
    std::fs::rename(&tmp, path)?;
    Ok(())
}
