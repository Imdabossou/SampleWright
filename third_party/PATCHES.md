# Vendored code

Code compiled into SampleWright, copied from upstream so builds never depend
on anything outside this repository.

## demucs-rs (nikhilunni/demucs-rs, Apache-2.0)

Demucs source separation on burn. See `demucs-rs/VENDORED_FROM`.

- Only `demucs-core` is vendored (no CLI, plugin, wasm or web UI). No source
  changes.
- Its model download (a third-party Hugging Face mirror) is not used:
  SampleWright downloads weights converted from Meta's official checkpoints
  (docs/MODELS.md).
