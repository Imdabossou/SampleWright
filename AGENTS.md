# AGENTS.md

Desktop app that separates songs into stems (Demucs v4 through vendored demucs-rs) and cuts loops, one-shots and slices for a DAW. Rust workspace, egui app, plus `samplewright-cli`.

## Commands

- Tests: `cargo test --workspace --release --locked`
- Lint: `cargo clippy --workspace --all-targets --locked -- -D warnings`
- Run: `./run.sh` (app), `./run.sh cli --help`
- Package: `cargo xtask package`
- More in [BUILDING.md](BUILDING.md)

## Conventions

- Branches: work on `development`; `main` only gets tested releases.
- SampleWright is standalone. It shares code ideas with FretWright but must not depend on it or merge into it.
- DSP lives in `sw-audio` and `sw-core`, with tests. `sw-app` only draws and wires things up.
- Never commit copyrighted audio. Tests synthesise their signals; screenshots use Creative Commons music only.
- Follow the system light/dark theme and accent colour, with a remembered override.
- Keep README.md short. User how-to goes in `docs/guide.md`, developer detail in BUILDING.md.

## Traps

- Debug builds need `opt-level = 3` for `sw-audio` and `sw-core` (set in `Cargo.toml`), or resampling seven lanes takes minutes.
- Background jobs must call `request_repaint()` when they finish, or the window stays stale until the mouse moves.
- Don't send `ViewportCommand::Title` every frame. It forces continuous repaints and burns CPU while idle.
- Model files are checked by size and SHA-256. Changing a model means updating the `models-1` release and the checksums together.
- Windows and macOS bundles are built by CI but have barely been run on real machines.
