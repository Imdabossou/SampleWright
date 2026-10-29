//! The model behind SampleWright: a [`Session`] is one song, separated into
//! stems, with its beat grid and the [`Region`]s marked for export.
//!
//! * [`separate`]: decode, separate (Demucs) and beat-track a song
//! * [`region`]: a marked stretch of some stems and its processing chain
//! * [`slice`]: cut a region into one-shots at its onsets
//! * [`export`]: write regions, slices and whole stems as WAV files

pub mod export;
pub mod grid;
pub mod region;
pub mod separate;
pub mod slice;

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use serde::{Deserialize, Serialize};
use sw_audio::Audio;

pub use grid::Grid;
pub use region::{Chain, Kind, Region};

/// The rate stems are stored and processed at (Demucs' rate).
pub const SAMPLE_RATE: u32 = 44_100;

/// Name of the pseudo-stem holding the original song.
pub const MIX: &str = "mix";

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error(transparent)]
    Audio(#[from] sw_audio::Error),
    #[error(transparent)]
    Model(#[from] sw_models::Error),
    #[error("{0}")]
    Io(#[from] std::io::Error),
    #[error("session file: {0}")]
    Json(#[from] serde_json::Error),
    #[error("{0}")]
    Other(String),
}

pub type Result<T> = std::result::Result<T, Error>;

/// Bumped when the session file changes incompatibly.
const FORMAT: u32 = 1;

/// One song: what was separated, its beat grid and its regions. Saved as
/// `session.json` in its folder, next to the stems.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Session {
    pub format: u32,
    pub source: PathBuf,
    pub title: String,
    /// Demucs model of the stems in use.
    pub model: String,
    /// Stem names in display order (the mix is not listed).
    pub stems: Vec<String>,
    /// What ran the separation, e.g. the GPU's name.
    #[serde(default)]
    pub device: String,
    pub duration: f64,
    pub grid: Grid,
    pub regions: Vec<Region>,
    #[serde(default)]
    pub export: export::Settings,
    #[serde(skip)]
    pub dir: PathBuf,
}

impl Session {
    pub fn file(dir: &Path) -> PathBuf {
        dir.join("session.json")
    }

    pub fn load(dir: &Path) -> Result<Session> {
        let mut s: Session = serde_json::from_slice(&std::fs::read(Session::file(dir))?)?;
        if s.format > FORMAT {
            return Err(Error::Other("this session was saved by a newer SampleWright".into()));
        }
        s.dir = dir.to_path_buf();
        Ok(s)
    }

    pub fn save(&self) -> Result<()> {
        std::fs::create_dir_all(&self.dir)?;
        let tmp = self.dir.join("session.json.part");
        std::fs::write(&tmp, serde_json::to_vec_pretty(self)?)?;
        std::fs::rename(tmp, Session::file(&self.dir))?;
        Ok(())
    }

    pub fn stem_path(&self, stem: &str) -> PathBuf {
        stem_path(&self.dir, &self.model, stem)
    }

    /// All lanes: the mix, then the stems.
    pub fn lanes(&self) -> Vec<String> {
        std::iter::once(MIX.to_string()).chain(self.stems.iter().cloned()).collect()
    }

    /// Decodes the mix and every stem.
    pub fn load_audio(&self) -> Result<Stems> {
        use rayon::prelude::*;
        let lanes = self.lanes();
        let decoded: Vec<Result<(String, Arc<Audio>)>> =
            lanes.par_iter().map(|l| Ok((l.clone(), Arc::new(sw_audio::decode(self.stem_path(l))?)))).collect();
        let mut map = BTreeMap::new();
        for d in decoded {
            let (k, v) = d?;
            map.insert(k, v);
        }
        Ok(Stems(map))
    }

    /// A new region id.
    pub fn next_region_id(&self) -> u64 {
        self.regions.iter().map(|r| r.id).max().unwrap_or(0) + 1
    }

    /// Models whose stems are already on disk for this song.
    pub fn separated_models(&self) -> Vec<String> {
        sw_models::demucs::models()
            .filter(|m| sw_models::demucs::stems_of(m).iter().all(|s| stem_path(&self.dir, m, s).is_file()))
            .map(str::to_string)
            .collect()
    }
}

pub fn stem_path(dir: &Path, model: &str, stem: &str) -> PathBuf {
    if stem == MIX { dir.join("mix.wav") } else { dir.join("stems").join(model).join(format!("{stem}.wav")) }
}

/// Decoded audio of a session's lanes (stereo, [`SAMPLE_RATE`]).
#[derive(Debug, Clone, Default)]
pub struct Stems(pub BTreeMap<String, Arc<Audio>>);

impl Stems {
    pub fn get(&self, name: &str) -> Option<&Arc<Audio>> {
        self.0.get(name)
    }
}

/// Where sessions live: `<data dir>/sessions`.
pub fn sessions_dir() -> Result<PathBuf> {
    sw_models::user_data_dir().map(|d| d.join("sessions")).ok_or_else(|| Error::Other("no home directory to keep sessions in".into()))
}

/// The session folder of a song: named by a hash of the file's contents, so
/// renaming or moving the song still finds its stems.
pub fn session_dir_for(song: &Path) -> Result<PathBuf> {
    use sha2::{Digest, Sha256};
    let mut f = std::fs::File::open(song)?;
    let mut h = Sha256::new();
    std::io::copy(&mut f, &mut h)?;
    let hex: String = h.finalize().iter().take(8).map(|b| format!("{b:02x}")).collect();
    Ok(sessions_dir()?.join(hex))
}

/// A tidy title from a file name: "01 - Artist - Song (Official Video).mp3" -> "Artist - Song".
pub fn title_of(path: &Path) -> String {
    let stem = path.file_stem().map(|s| s.to_string_lossy().to_string()).unwrap_or_default();
    let mut t = stem.as_str();
    for (open, close) in [('(', ')'), ('[', ']')] {
        if let Some(i) = t.find(open)
            && t[i..].contains(close)
            && i > 0
        {
            t = &t[..i];
        }
    }
    let t = t.trim().trim_start_matches(|c: char| c.is_ascii_digit()).trim_start_matches(['.', '-', '_', ' ']).trim();
    if t.is_empty() { stem } else { t.to_string() }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn titles() {
        assert_eq!(title_of(Path::new("/x/01 - Kevin MacLeod - Funkorama (Official).mp3")), "Kevin MacLeod - Funkorama");
        assert_eq!(title_of(Path::new("song [live].flac")), "song");
        assert_eq!(title_of(Path::new("1979.mp3")), "1979");
    }
}
