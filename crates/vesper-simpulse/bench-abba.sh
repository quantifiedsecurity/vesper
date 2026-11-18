#!/bin/sh
# ABBA benchmark of the Rust build (vesper-simpulse-seed) against the Vesper
# port (vesper-simpulse); see examples/abba.rs.
# Usage: crates/vesper-simpulse/bench-abba.sh [epochs] [runs per measurement]
set -eu
cd "$(dirname "$0")/../.."
epochs=${1:-11}
ops=${2:-40}
out="${CARGO_TARGET_DIR:-$PWD/target}/release/examples"
dir=$(mktemp -d)
trap 'rm -rf "$dir"' EXIT
cargo build --locked --release -q -p vesper-simpulse-seed --example abba
cp "$out/abba" "$dir/rust"
cargo build --locked --release -q -p vesper-simpulse --example abba
cp "$out/abba" "$dir/port"
"$dir/rust" compare "$dir/rust" "$dir/port" "$epochs" "$ops"
