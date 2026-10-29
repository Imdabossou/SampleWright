//! Neural networks for SampleWright, run in pure Rust (no Python):
//!
//! * [`demucs`]: Demucs source separation on the GPU (wgpu: Vulkan, Metal or
//!   DX12) or the CPU
//! * [`beat_this`]: Beat This! beat and downbeat tracking (Foscarin et al.,
//!   ISMIR 2024), run on the CPU with rten
//!
//! Model weights are not part of the source tree; [`download`] fetches them
//! and [`find_model`] looks for them in the usual places.

pub mod beat_this;
pub mod demucs;
pub mod download;

use std::path::PathBuf;

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("model file {name} not found (looked in: {searched})")]
    NotFound { name: String, searched: String },
    #[error("could not load model {path}: {msg}")]
    Load { path: PathBuf, msg: String },
    #[error("inference failed: {0}")]
    Run(String),
    #[error("model download failed: {0}")]
    Download(String),
}

pub type Result<T> = std::result::Result<T, Error>;

/// Directories searched for model files, in order:
/// `$SAMPLEWRIGHT_MODELS`, `models/` next to the executable (and its parents,
/// for `target/release` builds), the per-user data directory
/// (`~/.local/share/samplewright/models`, `%APPDATA%\samplewright\models`,
/// `~/Library/Application Support/samplewright/models`), and last
/// FretWright's, which uses the same files, so they are not downloaded twice.
pub fn model_dirs() -> Vec<PathBuf> {
    let mut dirs = Vec::new();
    if let Some(d) = std::env::var_os("SAMPLEWRIGHT_MODELS") {
        dirs.push(PathBuf::from(d));
    }
    if let Ok(exe) = std::env::current_exe() {
        for anc in exe.ancestors().skip(1).take(4) {
            dirs.push(anc.join("models"));
        }
    }
    if let Some(d) = user_data_dir() {
        dirs.push(d.join("models"));
    }
    if let Some(d) = data_dir_of("fretwright") {
        dirs.push(d.join("models"));
    }
    dirs
}

fn data_dir_of(name: &str) -> Option<PathBuf> {
    let env = |k: &str| std::env::var_os(k).filter(|v| !v.is_empty()).map(PathBuf::from);
    if cfg!(windows) {
        env("APPDATA").map(|d| d.join(name))
    } else if cfg!(target_os = "macos") {
        env("HOME").map(|h| h.join("Library/Application Support").join(name))
    } else {
        env("XDG_DATA_HOME").or_else(|| env("HOME").map(|h| h.join(".local/share"))).map(|d| d.join(name))
    }
}

/// The per-user data directory of SampleWright.
pub fn user_data_dir() -> Option<PathBuf> {
    data_dir_of("samplewright")
}

/// Finds a model file by its relative name, e.g. `beat_this/final0.onnx`.
pub fn find_model(name: &str) -> Result<PathBuf> {
    let dirs = model_dirs();
    dirs.iter().map(|d| d.join(name)).find(|p| p.is_file()).ok_or_else(|| Error::NotFound {
        name: name.to_string(),
        searched: dirs.iter().map(|d| d.display().to_string()).collect::<Vec<_>>().join(", "),
    })
}
