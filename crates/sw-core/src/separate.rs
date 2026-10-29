//! Opening a song: decode it, separate the stems with Demucs (once per
//! model; they are kept in the session folder) and find the beat grid.

use std::path::Path;

use sw_audio::Audio;
use sw_models::{beat_this, demucs};

use crate::{Error, FORMAT, Grid, MIX, Result, SAMPLE_RATE, Session, stem_path};

/// What the separation is doing, for progress display.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Stage {
    Decoding,
    Separating(f64),
    Beats(f64),
    Saving,
}

impl Stage {
    pub fn label(self) -> String {
        match self {
            Stage::Decoding => "Reading the song".into(),
            Stage::Separating(f) => format!("Separating stems {:.0}%", f * 100.0),
            Stage::Beats(f) => format!("Finding the beats {:.0}%", f * 100.0),
            Stage::Saving => "Saving stems".into(),
        }
    }

    /// Overall progress 0..1 (separation is most of the work).
    pub fn fraction(self) -> f64 {
        match self {
            Stage::Decoding => 0.0,
            Stage::Separating(f) => 0.02 + 0.85 * f,
            Stage::Beats(f) => 0.87 + 0.1 * f,
            Stage::Saving => 0.98,
        }
    }
}

/// The model files a separation with `model` needs that are not installed.
pub fn missing_models(model: &str) -> Vec<&'static sw_models::download::Asset> {
    [demucs::model_file(model), beat_this::DEFAULT_MODEL.to_string()]
        .iter()
        .filter_map(|f| sw_models::download::asset(f))
        .filter(|a| sw_models::find_model(a.file).is_err())
        .collect()
}

/// Opens `song`: reuses its session (and stems) if it was opened before,
/// separating again only if `model`'s stems are not there yet.
pub fn open(song: &Path, model: &str, device: demucs::Device, progress: &(dyn Fn(Stage) + Sync)) -> Result<Session> {
    let dir = crate::session_dir_for(song)?;
    let existing = Session::load(&dir).ok();
    if let Some(s) = &existing
        && (s.model == model || model.is_empty())
        && s.stems.iter().chain([&MIX.to_string()]).all(|st| s.stem_path(st).is_file())
    {
        return Ok(s.clone());
    }
    let model = if model.is_empty() { "htdemucs_6s" } else { model };

    progress(Stage::Decoding);
    let mix_path = stem_path(&dir, model, MIX);
    let mix = if mix_path.is_file() {
        sw_audio::decode(&mix_path)?
    } else {
        sw_audio::decode(song)?.to_stereo()?.resample(SAMPLE_RATE)
    };
    if mix.frames() < SAMPLE_RATE as usize / 2 {
        return Err(Error::Other("the song is shorter than half a second".into()));
    }
    let names = demucs::stems_of(model);
    let have_stems = names.iter().all(|n| stem_path(&dir, model, n).is_file());
    let mut device_name = existing.as_ref().map(|s| s.device.clone()).unwrap_or_default();
    if !have_stems {
        let sep = demucs::separate(model, &names, device, &mix.channels[0], &mix.channels[1], &|f| progress(Stage::Separating(f)))?;
        progress(Stage::Saving);
        for (name, l, r) in sep.stems {
            sw_audio::wav::write(&stem_path(&dir, model, &name), &Audio::new(SAMPLE_RATE, vec![l, r]), sw_audio::wav::Format::Float32)?;
        }
        device_name = sep.device;
    }
    if !mix_path.is_file() {
        sw_audio::wav::write(&mix_path, &mix, sw_audio::wav::Format::Float32)?;
    }

    let grid = match &existing {
        Some(s) if !s.grid.beats.is_empty() => s.grid.clone(),
        _ => {
            progress(Stage::Beats(0.0));
            let tracker = beat_this::BeatThis::load_default()?;
            let y = sw_audio::resample::resample(&mix.mono(), SAMPLE_RATE, beat_this::SAMPLE_RATE);
            let b = tracker.track(&y, &mut |f| progress(Stage::Beats(f)))?;
            Grid { beats: b.beats, downbeats: b.downbeats }
        }
    };
    let session = Session {
        format: FORMAT,
        source: song.to_path_buf(),
        title: existing.as_ref().map_or_else(|| crate::title_of(song), |s| s.title.clone()),
        model: model.to_string(),
        stems: names.iter().map(|s| s.to_string()).collect(),
        device: device_name,
        duration: mix.duration(),
        grid,
        regions: existing.as_ref().map(|s| s.regions.clone()).unwrap_or_default(),
        export: existing.map(|s| s.export).unwrap_or_default(),
        dir: dir.clone(),
    };
    session.save()?;
    Ok(session)
}
