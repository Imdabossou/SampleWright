# Contributing

Bug reports and ideas are welcome as GitHub issues. Please include your
operating system, GPU, and what you did, plus the song's format (not the song
itself, unless it is freely licensed).

For code:

- `cargo clippy --workspace --all-targets -- -D warnings` and
  `cargo test --workspace --release` must pass.
- Keep the DSP in `sw-audio` and `sw-core` tested; the app (`sw-app`) only
  draws and wires things up.
- Never commit copyrighted audio: tests synthesise their signals.

Security problems: see [SECURITY.md](SECURITY.md).
