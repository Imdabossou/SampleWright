# Model files

SampleWright runs two neural networks. Their weights are not in the repository;
the app downloads them on first start (or `samplewright-cli models download`) from the
`models-1` GitHub release and checks each file's size and SHA-256
(`crates/sw-models/src/download.rs`).

| File | Size | What | Source and licence |
|------|------|------|--------------------|
| `beat_this/final0.onnx` | 83 MB | Beat This! beat and downbeat tracker | official `final0` checkpoint, JKU Linz, MIT |
| `demucs/htdemucs_ft.safetensors` | 336 MB | Demucs v4 fine-tuned (best-quality 4-stem) | Meta's official checkpoints `f7e0c4bc`, `d12395a8`, `92cfc3b6`, `04573f0d`, MIT |
| `demucs/htdemucs.safetensors` | 84 MB | Demucs v4 | Meta `955717e8`, MIT |
| `demucs/htdemucs_6s.safetensors` | 55 MB | Demucs v4 6-stem (default separator) | Meta `5c90dfd2`, MIT |

## How they were made

They were converted once, with PyTorch, from the official checkpoints; nothing
was retrained or changed.

- **Beat This!**: the network (not the spectrogram front end, which SampleWright
  computes itself) exported to ONNX with tuple outputs `beat` and `downbeat`,
  input `spectrogram` `[batch, time, 128]`, dynamic batch and time axes, opset 17,
  TorchScript exporter. The export is deterministic.
- **Demucs**: each sub-model's `state_dict` saved as safetensors with keys
  `<signature>.<parameter>`, the layout demucs-rs reads, in float16. Meta's
  checkpoints are already half precision, so this is lossless: separations are
  bit-identical to float32 copies.

## Verifying

The Rust ports were checked against the PyTorch originals when they were
written for [FretWright](https://github.com/Imdabossou/FretWright), which uses
the same files: with identical input audio, Beat This! produces identical beats
and downbeats (logits within 1e-4), and the Demucs bass stem is within -30 dB of
PyTorch's (the rest is PyTorch's own random time-shift augmentation).

## Elsewhere

`SAMPLEWRIGHT_MODELS=<folder>` or a `models` folder next to the program makes
SampleWright use local copies (e.g. offline installs; `cargo xtask package
--with-models` bundles them). `SAMPLEWRIGHT_MODEL_URL` points the download at a
mirror. Models FretWright already downloaded are found and used as well.
