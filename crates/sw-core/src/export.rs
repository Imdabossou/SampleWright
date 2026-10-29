//! Writing samples: regions, their slices and whole stems, as WAV files named
//! so a DAW's browser sorts and explains them
//! (`Song_bass_120bpm_bars09-16.wav`, `Song_drums_03_kick.wav`).

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use sw_audio::Audio;
use sw_audio::wav::{self, Format};

use crate::{Kind, Region, Result, SAMPLE_RATE, Session, Stems, slice};

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Settings {
    pub format: Format,
    /// 44100 or 48000.
    pub sample_rate: u32,
    pub mono: bool,
    /// One file per stem of a multi-stem region, instead of one mixed file.
    pub separate_stems: bool,
    /// Export folder; `None` means `<Music>/SampleWright/<song>`.
    pub dir: Option<PathBuf>,
}

impl Default for Settings {
    fn default() -> Settings {
        Settings { format: Format::Pcm24, sample_rate: 44_100, mono: false, separate_stems: false, dir: None }
    }
}

/// Keeps file names portable: letters, digits, space, `-`, `_`, `+`, `#` and `.`.
pub fn sanitize(s: &str) -> String {
    let t: String = s.chars().map(|c| if c.is_alphanumeric() || " -_+#.".contains(c) { c } else { '_' }).collect();
    let t = t.trim().trim_matches('.').to_string();
    if t.is_empty() { "untitled".into() } else { t }
}

/// The default export folder for a song.
pub fn default_dir(title: &str) -> PathBuf {
    let home = std::env::var_os(if cfg!(windows) { "USERPROFILE" } else { "HOME" }).map(PathBuf::from).unwrap_or_default();
    let music = std::env::var_os("XDG_MUSIC_DIR").map(PathBuf::from).unwrap_or_else(|| home.join("Music"));
    music.join("SampleWright").join(sanitize(title))
}

impl Session {
    pub fn export_dir(&self) -> PathBuf {
        self.export.dir.clone().unwrap_or_else(|| default_dir(&self.title))
    }

    /// The tempo and position part of a name: "120bpm_bars09-16", or the
    /// time in seconds when the region is not on bar lines.
    pub fn position_label(&self, r: &Region) -> String {
        let bpm = self.grid.bpm().map(|b| format!("{}bpm_", b.round() as i64)).unwrap_or_default();
        match self.grid.bars_of(r.start, r.end) {
            Some((a, b)) if a == b => format!("{bpm}bar{a:02}"),
            Some((a, b)) => format!("{bpm}bars{a:02}-{b:02}"),
            None => format!("{bpm}{:.2}s", r.start),
        }
    }

    fn prepare(&self, a: Audio) -> Audio {
        let mut a = if self.export.sample_rate != SAMPLE_RATE { a.resample(self.export.sample_rate) } else { a };
        if self.export.mono {
            a.channels = vec![a.mono()];
        }
        a
    }

    /// Exports a region into `dir`, returning the files written.
    pub fn export_region(&self, stems: &Stems, r: &Region, dir: &Path) -> Result<Vec<PathBuf>> {
        let song = sanitize(&self.title);
        let groups: Vec<Vec<String>> =
            if self.export.separate_stems && r.stems.len() > 1 { r.stems.iter().map(|s| vec![s.clone()]).collect() } else { vec![r.stems.clone()] };
        let mut written = Vec::new();
        for lanes in groups {
            let who = if lanes.len() == 1 && lanes[0] != r.name && r.stems.len() > 1 { format!("{}_{}", sanitize(&r.name), lanes[0]) } else { sanitize(&r.name) };
            match r.kind {
                Kind::Loop | Kind::OneShot => {
                    let path = dir.join(format!("{song}_{who}_{}.wav", self.position_label(r)));
                    wav::write(&path, &self.prepare(r.render(stems, &lanes)), self.export.format)?;
                    written.push(path);
                }
                Kind::Slices => {
                    // detect on everything the region holds, cut each lane at the same points
                    let cuts = slice::detect(r, stems);
                    let p = r.process(stems, &lanes);
                    let sub = dir.join(format!("{song}_{who}_slices_{}", self.position_label(r)));
                    for (i, s) in cuts.iter().enumerate() {
                        let label = s.label.as_deref().map(|l| format!("_{l}")).unwrap_or_default();
                        let path = sub.join(format!("{who}_{:02}{label}.wav", i + 1));
                        wav::write(&path, &self.prepare(slice::render(r, &p, s)), self.export.format)?;
                        written.push(path);
                    }
                }
            }
        }
        Ok(written)
    }

    /// Exports whole stems (the full song length, unprocessed).
    pub fn export_stems(&self, stems: &Stems, names: &[String], dir: &Path) -> Result<Vec<PathBuf>> {
        let song = sanitize(&self.title);
        let mut written = Vec::new();
        for n in names {
            if let Some(a) = stems.get(n) {
                let path = dir.join(format!("{song}_{n}.wav"));
                wav::write(&path, &self.prepare((**a).clone()), self.export.format)?;
                written.push(path);
            }
        }
        Ok(written)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{Grid, MIX};
    use std::sync::Arc;

    #[test]
    fn names_are_portable() {
        assert_eq!(sanitize("AC/DC: Back in Black?"), "AC_DC_ Back in Black_");
        assert_eq!(sanitize("..."), "untitled");
        assert_eq!(sanitize("bass+drums C#2"), "bass+drums C#2");
    }

    #[test]
    fn exports_a_bar_loop_with_a_useful_name() {
        let beats: Vec<f64> = (0..32).map(|i| i as f64 * 0.5).collect();
        let grid = Grid { downbeats: beats.iter().step_by(4).copied().collect(), beats };
        let tmp = std::env::temp_dir().join(format!("sw-export-{}", std::process::id()));
        let s = Session {
            format: 1,
            source: "x.mp3".into(),
            title: "Demo: Song".into(),
            model: "htdemucs".into(),
            stems: vec!["bass".into()],
            device: String::new(),
            duration: 16.0,
            grid,
            regions: vec![],
            export: Settings { sample_rate: 48_000, ..Default::default() },
            dir: tmp.clone(),
        };
        let x: Vec<f32> = (0..SAMPLE_RATE as usize * 16).map(|i| (i as f32 * 0.01).sin() * 0.3).collect();
        let mut stems = Stems::default();
        stems.0.insert("bass".into(), Arc::new(Audio::new(SAMPLE_RATE, vec![x.clone(), x.clone()])));
        stems.0.insert(MIX.into(), Arc::new(Audio::new(SAMPLE_RATE, vec![x.clone(), x])));
        let r = Region::new(1, 2.0, 6.0, vec!["bass".into()]);
        let files = s.export_region(&stems, &r, &tmp).unwrap();
        assert_eq!(files.len(), 1);
        assert_eq!(files[0].file_name().unwrap().to_string_lossy(), "Demo_ Song_bass_120bpm_bars02-03.wav");
        let back = sw_audio::decode(&files[0]).unwrap();
        assert_eq!(back.sample_rate, 48_000);
        assert_eq!(back.frames(), 4 * 48_000);
        std::fs::remove_dir_all(&tmp).unwrap();
    }
}
