# Building SampleWright

## Requirements

- [Rust](https://rustup.rs) (stable)
- Linux: the ALSA headers (`libasound2-dev` on Debian/Ubuntu, `alsa-lib` on Arch)

## Commands

```sh
./run.sh                                             # build and start the app
./run.sh cli --help                                  # the command-line tool
cargo test --workspace --release --locked            # tests
cargo clippy --workspace --all-targets --locked -- -D warnings
cargo xtask package                                  # a release bundle in dist/
```

CI runs clippy and the tests on every push to `main` and `development`, and builds bundles for Linux, Windows and macOS. Pushing a `v*` tag publishes them as a release; see [docs/RELEASING.md](docs/RELEASING.md). The AI models live in a separate `models-1` release, described in [docs/MODELS.md](docs/MODELS.md).

## Crates

| Crate | What |
| --- | --- |
| `sw-audio` | Decoding, WAV writing, filters, noise gate, loop crossfades, onset detection, pitch, peaks and spectrum |
| `sw-models` | Model download and checking. Also finds models FretWright has downloaded |
| `sw-core` | Sessions (one per song, cached by content), regions and their cleanup chains, slicing, export naming |
| `sw-cli` | `samplewright-cli` |
| `sw-app` | The desktop app (egui). It only draws and wires things up |
| `xtask` | Release packaging |

Separation uses the vendored [demucs-rs](https://github.com/nikhilunni/demucs-rs) on the GPU through Vulkan, Metal or DX12. Beat and bar tracking uses Beat This!.
