# Releasing

1. On `development`: set `version` in the root `Cargo.toml` (SemVer; 0.x is
   early access), run `cargo test --workspace --release` and try the app.
2. Merge `development` into `main` and push.
3. Tag `main` with an annotated tag `vX.Y.Z` (it must match `Cargo.toml`) and
   push it. The `build` workflow tests, builds the Linux, Windows and macOS
   bundles and publishes them as the release "SampleWright vX.Y.Z".

## Model files

The model weights live in a separate release, `models-1`, so app releases stay
small and old versions keep downloading the same files. Its assets are the
files of `docs/MODELS.md` with `/` replaced by `-` (`demucs-htdemucs_6s.safetensors`);
their sizes and SHA-256 sums are in `crates/sw-models/src/download.rs`.
Changing a model means a new `models-N` release and updating that file.
