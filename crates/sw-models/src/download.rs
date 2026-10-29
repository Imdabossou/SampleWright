//! Downloading the model weights, verified by size and SHA-256.
//!
//! The files are published as assets of a GitHub release (Beat This! exported
//! to ONNX, Demucs converted to safetensors (docs/MODELS.md), both from the
//! official checkpoints). Set `SAMPLEWRIGHT_MODEL_URL` to use a mirror.

use std::io::{Read, Write};
use std::path::PathBuf;

use sha2::{Digest, Sha256};

use crate::{Error, Result};

/// A downloadable model file.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Asset {
    /// Path relative to a model directory (also what [`crate::find_model`] looks for).
    pub file: &'static str,
    pub size: u64,
    pub sha256: &'static str,
    pub what: &'static str,
    /// Downloaded on first start (what the default separation needs).
    pub default: bool,
}

pub const ASSETS: &[Asset] = &[
    Asset {
        file: "beat_this/final0.onnx",
        size: 83_162_650,
        sha256: "04de70d97eb6ad6a850ccfb7943b602bfda49ac49c7c87b94b2818b63b25b86f",
        what: "Beat This! beat and bar tracker",
        default: true,
    },
    Asset {
        file: "demucs/htdemucs_ft.safetensors",
        size: 336_118_040,
        sha256: "aade0d59ae52c875f33a9021924a412c1692433028d08dcb121052222f77d409",
        what: "Demucs v4 fine-tuned 4-stem (best quality)",
        default: false,
    },
    Asset {
        file: "demucs/htdemucs.safetensors",
        size: 84_028_720,
        sha256: "96bc4547c2c7042739197ad9aa40ccb1be74db70104b001517c2e8075d9364e5",
        what: "Demucs v4 4-stem (fastest)",
        default: false,
    },
    Asset {
        file: "demucs/htdemucs_6s.safetensors",
        size: 54_888_928,
        sha256: "4689e2eada8b85a45513403240ae8eed67d84bb950756aa70d149949013f833c",
        what: "Demucs v4 6-stem (adds guitar and piano)",
        default: true,
    },
];

pub const DEFAULT_BASE_URL: &str = "https://github.com/Imdabossou/SampleWright/releases/download/models-1/";

pub fn base_url() -> String {
    std::env::var("SAMPLEWRIGHT_MODEL_URL").ok().filter(|u| !u.is_empty()).unwrap_or_else(|| DEFAULT_BASE_URL.to_string())
}

/// Release asset name: the relative path with '/' replaced by '-'.
pub fn url(a: &Asset) -> String {
    let base = base_url();
    let sep = if base.ends_with('/') { "" } else { "/" };
    format!("{base}{sep}{}", a.file.replace('/', "-"))
}

pub fn asset(file: &str) -> Option<&'static Asset> {
    ASSETS.iter().find(|a| a.file == file)
}

/// Default assets that are not installed anywhere yet.
pub fn missing_defaults() -> Vec<&'static Asset> {
    ASSETS.iter().filter(|a| a.default && crate::find_model(a.file).is_err()).collect()
}

/// Where downloads go: the per-user model directory.
pub fn install_dir() -> Result<PathBuf> {
    crate::user_data_dir().map(|d| d.join("models")).ok_or_else(|| Error::Download("no home directory to store models in".into()))
}

/// Downloads one asset into the per-user model directory, reporting
/// `(bytes so far, total)`. The file only appears once it is complete and
/// its checksum matches.
pub fn download(a: &Asset, progress: &mut dyn FnMut(u64, u64)) -> Result<PathBuf> {
    let dest = install_dir()?.join(a.file);
    if let Some(dir) = dest.parent() {
        std::fs::create_dir_all(dir).map_err(|e| Error::Download(format!("{}: {e}", dir.display())))?;
    }
    let part = dest.with_extension("part");
    let url = url(a);
    let fail = |m: String| Error::Download(format!("{url}: {m}"));
    let resp = ureq::get(&url).call().map_err(|e| fail(e.to_string()))?;
    let mut body = resp.into_body();
    let mut reader = body.as_reader();
    let mut file = std::fs::File::create(&part).map_err(|e| fail(e.to_string()))?;
    let mut hasher = Sha256::new();
    let mut buf = vec![0u8; 1 << 16];
    let mut done = 0u64;
    loop {
        let n = reader.read(&mut buf).map_err(|e| fail(e.to_string()))?;
        if n == 0 {
            break;
        }
        hasher.update(&buf[..n]);
        file.write_all(&buf[..n]).map_err(|e| fail(e.to_string()))?;
        done += n as u64;
        if done > a.size {
            break;
        }
        progress(done, a.size);
    }
    drop(file);
    let digest: String = hasher.finalize().iter().map(|b| format!("{b:02x}")).collect();
    if done != a.size || digest != a.sha256 {
        let _ = std::fs::remove_file(&part);
        return Err(fail(format!("download is corrupt ({done} of {} bytes, sha256 {digest})", a.size)));
    }
    std::fs::rename(&part, &dest).map_err(|e| fail(e.to_string()))?;
    Ok(dest)
}

/// True if the file at `path` is exactly the published asset.
pub fn verify(path: &std::path::Path, a: &Asset) -> bool {
    let Ok(mut f) = std::fs::File::open(path) else { return false };
    if f.metadata().map(|m| m.len()).ok() != Some(a.size) {
        return false;
    }
    let mut hasher = Sha256::new();
    if std::io::copy(&mut f, &mut hasher).is_err() {
        return false;
    }
    hasher.finalize().iter().map(|b| format!("{b:02x}")).collect::<String>() == a.sha256
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn asset_urls_are_flat_names() {
        assert!(url(&ASSETS[0]).ends_with("/beat_this-final0.onnx"));
        assert!(ASSETS.iter().all(|a| a.sha256.len() == 64));
        assert_eq!(asset("demucs/htdemucs_6s.safetensors").map(|a| a.default), Some(true));
    }
}
