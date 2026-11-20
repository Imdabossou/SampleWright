# Contributing to SampleWright

Thanks for helping! A few things to know first.

- **Bugs and ideas:** open an issue. Include your operating system, GPU, and what you did, plus the song's format (not the song itself, unless it is freely licensed).
- **Branches:** open pull requests against `development`. `main` only gets tested releases.
- **Checks:** `cargo clippy --workspace --all-targets --locked -- -D warnings` and `cargo test --workspace --release --locked` must pass. CI runs both. See [BUILDING.md](BUILDING.md).
- **Where code goes:** keep the DSP in `sw-audio` and `sw-core`, with tests. The app (`sw-app`) only draws and wires things up.
- **No copyrighted audio:** never commit it. Tests synthesise their signals.
- **Licence:** contributions are accepted under the project's licence (GPL-3.0-or-later). Third-party code must be GPL-compatible, and new models or libraries need an entry in [LICENSES/](LICENSES/README.md).
- **Security issues:** please report them privately, see [SECURITY.md](SECURITY.md).
