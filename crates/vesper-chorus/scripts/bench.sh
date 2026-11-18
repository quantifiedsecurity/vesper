#!/bin/sh
# ABBA benchmark of the Rust and Vesper builds of vesper-chorus (see
# examples/abba.rs): builds the example both ways (release), keeps each
# binary, and drives both workloads. Not run in CI; the numbers are in
# docs/parallel-results/port-p3b-chorus.md.
set -eu
cd "$(dirname "$0")/../../.."
dir=$(mktemp -d)
trap 'rm -rf "$dir"' EXIT HUP INT TERM
exe=""
case "$(uname -s)" in MINGW*|MSYS*|CYGWIN*) exe=".exe" ;; esac
cargo build --locked --release -p vesper-chorus --example abba
cp "target/release/examples/abba$exe" "$dir/rust$exe"
cargo build --locked --release -p vesper-chorus --features vesper-port --example abba
cp "target/release/examples/abba$exe" "$dir/vesper$exe"
for workload in project explore; do
    "$dir/rust$exe" drive "$workload" "$dir/rust$exe" "$dir/vesper$exe"
done
