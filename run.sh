#!/usr/bin/env bash
# Builds (first time only, needs Rust: https://rustup.rs) and starts SampleWright.
#   ./run.sh            start the app
#   ./run.sh <song>     start with a song
#   ./run.sh cli ...    command line tool (samplewright-cli without arguments lists its options)
set -euo pipefail
cd "$(dirname "$0")"
if ! command -v cargo >/dev/null 2>&1; then
    echo "Rust is needed to build SampleWright from source: https://rustup.rs" >&2
    echo "(or download a ready-made release instead)" >&2
    exit 1
fi
if [ "${1:-}" = "cli" ]; then
    shift
    cargo build --release --quiet -p sw-cli
    exec target/release/samplewright-cli "$@"
fi
cargo build --release --quiet -p sw-app
exec target/release/samplewright "$@"
