#!/bin/sh
# Port P1 lane 5 (#902), sourced by scripts/test-port-harness.sh (which defines `divergent`): Simpulse's
# kernel tests pass against the port-backed package and the Rust seed copy,
# and the divergent port (EDITS in build.rs) fails the round test.
set -eu
cargo test --locked -p vesper-simpulse -p vesper-simpulse-seed
divergent "Simpulse port (lib.rs)" each_fixed_runnable_task_runs_once_per_round \
    virtual_clock_only_jumps_when_nothing_can_run \
    -p vesper-simpulse --features vesper-port-divergent --test kernel
