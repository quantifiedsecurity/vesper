#!/bin/sh
# Port harness (port wave P0 lane 6, #878; build modes and the seed-copy rule
# from port wave P1 lane 2, #899) under Cargo, for the macOS and Windows
# checks; Linux runs the same checks as Bazel targets (BUILD.bazel of each
# crate below). Every port is compiled by one prebuilt `vesper` executable,
# VESPER_PORT_COMPILER (CI builds it first; this script builds it when unset).
#
# - The pilot's API tests must pass against its Rust build and its Vesper
#   build, and fail against a deliberately divergent Vesper build.
# - vesper-port-build links a multi-file Loom package with a path dependency,
#   and its divergent build must fail the tests.
# - vesper-beacon (port-backed) and vesper-beacon-seed (the compiler's Rust
#   copy) both pass Beacon's tests; vesper-beacon-arrow links the port; the
#   closure guard (vesper-port-harness tests/closure_guard.rs) checks that no
#   port-backed package is in the compiler's dependency closure.
#
# Ports append their own lines at the end.
set -eu
cd "$(dirname "$0")/.."
if [ -z "${VESPER_PORT_COMPILER:-}" ]; then
    cargo build --locked --bin vesper
    exe=vesper
    case "$(uname -s)" in MINGW* | MSYS* | CYGWIN* | Windows_NT) exe=vesper.exe ;; esac
    VESPER_PORT_COMPILER="${CARGO_TARGET_DIR:-$PWD/target}/debug/$exe"
fi
export VESPER_PORT_COMPILER
echo "compiling ports with $VESPER_PORT_COMPILER"

# divergent <description> <expected failing test> <expected passing test> <cargo test args...>
divergent() {
    what=$1 failing=$2 passing=$3
    shift 3
    log=$(mktemp)
    if cargo test --locked "$@" -- --test-threads=1 >"$log" 2>&1; then
        cat "$log"
        rm -f "$log"
        echo "the divergent $what passed its tests" >&2
        exit 1
    fi
    if ! grep -q "test $failing ... FAILED" "$log" || ! grep -q "test $passing ... ok" "$log"; then
        cat "$log"
        rm -f "$log"
        echo "the divergent $what failed for an unexpected reason" >&2
        exit 1
    fi
    rm -f "$log"
    echo "divergent $what detected ($failing failed, $passing agreed)"
}

cargo test --locked -p vesper-port-abi -p vesper-port-harness
cargo test --locked -p vesper-port-pilot
cargo test --locked -p vesper-port-pilot --features vesper-port
divergent "Vesper port of the pilot" bytes_and_booleans gcd_matches_euclid \
    -p vesper-port-pilot --features vesper-port-divergent --test api
cargo test --locked -p vesper-port-build
divergent "package port" root_package_modules_are_linked dependency_modules_are_linked \
    -p vesper-port-build --features divergent --test package
cargo test --locked -p vesper-beacon -p vesper-beacon-seed
cargo test --locked -p vesper-beacon-arrow --test beacon_build
