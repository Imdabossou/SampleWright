# Works SampleWright is derived from

Parts of SampleWright are Rust ports of code from these projects, or use their
model weights. None of their source files are included, but the ports are
derivative works, so their licences apply to those parts and are reproduced here.

| Project | Licence | Used in |
|---------|---------|---------|
| [Beat This!](https://github.com/CPJKU/beat_this) (JKU Linz) | MIT, [beat_this.txt](beat_this.txt) | log-mel front end, chunked inference and post-processing in `sw-models`; the model weights |
| [beat-this-rs](https://github.com/danigb/beat-this-rs) | MIT, [beat-this-rs.txt](beat-this-rs.txt) | the chunking scheme and ONNX export settings in `sw-models` |
| [Demucs](https://github.com/facebookresearch/demucs) (Meta) | MIT, [demucs.txt](demucs.txt) | the model weights, run by the vendored demucs-rs |

Vendored code, compiled into SampleWright, keeps its licence next to it in
`third_party/` (demucs-rs: Apache-2.0), and release bundles carry
`THIRD_PARTY_LICENSES.txt` with the licence of every crate.

The filters follow Robert Bristow-Johnson's Audio EQ Cookbook formulas, and the
pitch detector the published YIN algorithm (de Cheveigné and Kawahara, 2002).
The resampler, gate, onset detection, slicing, export and the app are original.
