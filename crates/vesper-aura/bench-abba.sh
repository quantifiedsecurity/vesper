#!/bin/sh
# ABBA benchmark of the replaced Rust implementation (baseline) against the
# Vesper port (examples/abba.rs). The Rust implementation was deleted in #979,
# so the baseline is built from a commit that still has it, in a temporary
# git worktree.
# Usage: crates/vesper-aura/bench-abba.sh [epochs] [ops per run] [rust commit]
set -eu
cd "$(dirname "$0")/../.."
epochs=${1:-15}
ops=${2:-200}
# The last measured commit with the Rust implementation as the default
# (docs/parallel-results/port-parity-aura.md).
base=${3:-cee8b1ac3552d7a2beccb5553ef43fee56b00868}
target="${CARGO_TARGET_DIR:-$PWD/target}"
dir=$(mktemp -d)
tree="$dir/rust-tree"
trap 'git worktree remove --force "$tree" >/dev/null 2>&1 || true; rm -rf "$dir"' EXIT
git worktree add --detach -q "$tree" "$base"
(cd "$tree" && CARGO_TARGET_DIR="$target/aura-rust-baseline" \
    cargo build --locked --release -q -p vesper-aura --example abba)
cp "$target/aura-rust-baseline/release/examples/abba" "$dir/rust"
cargo build --locked --release -q -p vesper-aura --example abba
cp "$target/release/examples/abba" "$dir/port"
"$dir/port" compare "$dir/rust" "$dir/port" "$epochs" "$ops"
