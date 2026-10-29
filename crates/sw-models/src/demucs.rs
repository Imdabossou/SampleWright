//! Demucs source separation (Hybrid Transformer Demucs, Rouard, Massa and
//! Défossez, ICASSP 2023) through demucs-rs (Apache-2.0, vendored in
//! `third_party/demucs-rs`), on the GPU with wgpu (Vulkan, Metal or DX12) or
//! on the CPU.
//!
//! Weights are Meta's official checkpoints converted to safetensors
//! (docs/MODELS.md; `demucs/<model>.safetensors` in a model directory). Input and output are stereo at 44.1 kHz.

use burn::backend::wgpu::{RuntimeOptions, WgpuDevice, graphics::AutoGraphicsApi, init_setup};
use burn::backend::{NdArray, Wgpu};
use burn::prelude::Backend as BurnBackend;
use demucs_core::listener::{ForwardEvent, ForwardListener};
use demucs_core::model::metadata::{ALL_MODELS, HTDEMUCS_6S_ID, HTDEMUCS_FT_ID, ModelInfo, StemId};
use demucs_core::{Demucs, ModelOptions};

use crate::{Error, Result};

pub const SAMPLE_RATE: u32 = 44_100;

/// Where to run the network.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Device {
    /// The GPU if one works, else the CPU.
    #[default]
    Auto,
    Gpu,
    Cpu,
}

/// (stem name, left, right)
pub type Stem = (String, Vec<f32>, Vec<f32>);

/// Separated stems at [`SAMPLE_RATE`].
#[derive(Debug, Clone)]
pub struct Separation {
    pub stems: Vec<Stem>,
    /// What ran the network, e.g. "AMD Radeon RX 7900 XTX (RADV NAVI31)" or "CPU".
    pub device: String,
}

impl Separation {
    pub fn stem(&self, name: &str) -> Option<(&[f32], &[f32])> {
        self.stems.iter().find(|s| s.0 == name).map(|s| (s.1.as_slice(), s.2.as_slice()))
    }
}

/// Model names: htdemucs, htdemucs_6s (adds guitar and piano), htdemucs_ft
/// (one fine-tuned network per stem: best quality, and only the requested
/// stems' networks run).
pub fn models() -> impl Iterator<Item = &'static str> {
    ALL_MODELS.iter().map(|m| m.id)
}

/// The stems a model separates, e.g. drums, bass, other, vocals.
pub fn stems_of(model: &str) -> Vec<&'static str> {
    info(model).map(|i| i.stems.iter().map(StemId::as_str).collect()).unwrap_or_default()
}

/// Model file of a model, relative to a model directory.
pub fn model_file(model: &str) -> String {
    format!("demucs/{model}.safetensors")
}

fn info(model: &str) -> Result<&'static ModelInfo> {
    ALL_MODELS.iter().copied().find(|m| m.id == model).ok_or_else(|| {
        Error::Run(format!("unknown Demucs model '{model}' (have: {})", models().collect::<Vec<_>>().join(", ")))
    })
}

struct Progress<'a>(&'a (dyn Fn(f64) + Sync));

impl ForwardListener for Progress<'_> {
    fn on_event(&mut self, event: ForwardEvent) {
        if let ForwardEvent::ChunkDone { index, total } = event {
            (self.0)((index + 1) as f64 / total.max(1) as f64);
        }
    }
}

fn run<B: BurnBackend>(
    info: &'static ModelInfo,
    wanted: &[StemId],
    bytes: &[u8],
    device: B::Device,
    left: &[f32],
    right: &[f32],
    progress: &(dyn Fn(f64) + Sync),
) -> Result<Vec<Stem>> {
    let opts = if info.id == HTDEMUCS_FT_ID {
        ModelOptions::FineTuned(wanted.to_vec())
    } else if info.id == HTDEMUCS_6S_ID {
        ModelOptions::SixStem
    } else {
        ModelOptions::FourStem
    };
    let model = Demucs::<B>::from_bytes(opts, bytes, device).map_err(|e| Error::Run(e.to_string()))?;
    let stems = pollster::block_on(model.separate_with_listener(left, right, SAMPLE_RATE, &mut Progress(progress)))
        .map_err(|e| Error::Run(e.to_string()))?;
    Ok(stems.into_iter().filter(|s| wanted.contains(&s.id)).map(|s| (s.id.as_str().to_string(), s.left, s.right)).collect())
}

fn gpu_device() -> std::result::Result<(WgpuDevice, String), String> {
    static INIT: std::sync::OnceLock<std::result::Result<String, String>> = std::sync::OnceLock::new();
    let device = WgpuDevice::DefaultDevice;
    let name = INIT.get_or_init(|| {
        // remember the fastest kernel variants across runs (in our data
        // directory, not cubecl's default of the current project's target/)
        use cubecl::config::{GlobalConfig, autotune::AutotuneConfig, cache::CacheConfig};
        let cache = crate::user_data_dir().map_or(CacheConfig::Global, |d| CacheConfig::File(d.join("gpu-cache")));
        GlobalConfig::set(GlobalConfig {
            autotune: AutotuneConfig { cache, ..Default::default() },
            ..Default::default()
        });
        // the runtime panics when there is no usable adapter
        let d = device.clone();
        std::panic::catch_unwind(move || {
            let setup = init_setup::<AutoGraphicsApi>(&d, RuntimeOptions { tasks_max: 128, ..Default::default() });
            setup.adapter.get_info().name
        })
        .map_err(|_| "no usable GPU (Vulkan, Metal or DX12) found".to_string())
    });
    name.clone().map(|n| (device, n))
}

/// Name of the GPU Demucs runs on, or why none is usable.
pub fn gpu_name() -> std::result::Result<String, String> {
    gpu_device().map(|(_, name)| name)
}

/// Separates the requested stems ("bass", "drums", ...) of stereo 44.1 kHz audio.
pub fn separate(model: &str, stems: &[&str], device: Device, left: &[f32], right: &[f32], progress: &(dyn Fn(f64) + Sync)) -> Result<Separation> {
    let info = info(model)?;
    let mut wanted = Vec::new();
    for s in stems {
        match StemId::parse(s) {
            Some(id) if info.stems.contains(&id) => wanted.push(id),
            _ => return Err(Error::Run(format!("model {model} has no '{s}' stem"))),
        }
    }
    // normalise like the demucs CLI does
    let n = left.len().min(right.len());
    let mono: Vec<f64> = (0..n).map(|i| (left[i] as f64 + right[i] as f64) / 2.0).collect();
    let mean = mono.iter().sum::<f64>() / n.max(1) as f64;
    let std = (mono.iter().map(|v| (v - mean).powi(2)).sum::<f64>() / (n.max(2) - 1) as f64).sqrt() + 1e-8;
    let norm = |x: &[f32]| -> Vec<f32> { x[..n].iter().map(|&v| ((v as f64 - mean) / std) as f32).collect() };
    let (left, right) = (&norm(left)[..], &norm(right)[..]);
    let path = crate::find_model(&model_file(model))?;
    let bytes = std::fs::read(&path).map_err(|e| Error::Load { path: path.clone(), msg: e.to_string() })?;

    // the network's graph is deep: run it on a thread with a generous stack
    // (Windows' 1 MB default is not enough), which also contains GPU-driver panics
    let on_big_stack = |job: &(dyn Fn() -> Result<Separation> + Sync)| -> Result<Separation> {
        std::thread::scope(|s| {
            std::thread::Builder::new()
                .name("demucs".into())
                .stack_size(16 << 20)
                .spawn_scoped(s, job)
                .map_err(|e| Error::Run(e.to_string()))?
                .join()
                .unwrap_or_else(|_| Err(Error::Run("Demucs crashed".into())))
        })
    };
    let gpu = || -> Result<Separation> {
        let (dev, name) = gpu_device().map_err(Error::Run)?;
        let stems = run::<Wgpu>(info, &wanted, &bytes, dev, left, right, progress)?;
        Ok(Separation { stems, device: name })
    };
    let cpu = || -> Result<Separation> {
        let stems = run::<NdArray<f32>>(info, &wanted, &bytes, Default::default(), left, right, progress)?;
        Ok(Separation { stems, device: "CPU".into() })
    };
    let mut sep = match device {
        Device::Gpu => on_big_stack(&gpu),
        Device::Cpu => on_big_stack(&cpu),
        Device::Auto => on_big_stack(&gpu).or_else(|_| on_big_stack(&cpu)),
    }?;
    for (_, l, r) in &mut sep.stems {
        for v in l.iter_mut().chain(r.iter_mut()) {
            *v = (*v as f64 * std + mean) as f32;
        }
    }
    Ok(sep)
}
