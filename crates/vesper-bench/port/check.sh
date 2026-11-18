#!/bin/sh
# vesper-bench port checks under Cargo (port wave P3a lane 5, #1040), run by
# scripts/test-port-harness.sh; Bazel runs the same builds as targets
# (crates/vesper-bench/BUILD.bazel). Every test must pass against the Rust
# seed (vesper-bench-seed) and the port-backed package (vesper-bench), and
# each divergent build (one edit per ported module, EDITS in build.rs, picked
# by VESPER_BENCH_DIVERGE) must fail the test that guards its module while an
# unrelated one in the same file passes.
set -eu
cd "$(dirname "$0")/../../.."
cargo test --locked -p vesper-bench-seed -p vesper-bench
cargo test --locked -p vesper-bench --features vesper-port
log=$(mktemp)
trap 'rm -f "$log"' EXIT HUP INT TERM
# check <module> <test file> <expected failing test> <expected passing test>
check() {
    if VESPER_BENCH_DIVERGE=$1 cargo test --locked -p vesper-bench --features vesper-port-divergent --test "$2" -- --test-threads=1 >"$log" 2>&1; then
        cat "$log"
        echo "the divergent bench port ($1) passed tests/$2.rs" >&2
        exit 1
    fi
    if ! grep -q "test $3 ... FAILED" "$log" || ! grep -q "test $4 ... ok" "$log"; then
        cat "$log"
        echo "the divergent bench port ($1) failed for an unexpected reason" >&2
        exit 1
    fi
    echo "divergent bench port ($1) detected ($3 failed, $4 agreed)"
}
check arithmetic statistics width_specific_counter_model_has_explicit_wrap_and_range_limits known_regression_improvement_ties_and_mixed_results
check paired_difference paired_difference fractional_and_out_of_range_differences_fail_at_their_index exact_per_operation_differences_preserve_sign_and_order
check walsh hodges_lehmann point_estimate_retains_quarter_units_and_integer_extremes bounds_and_ambiguous_ranks_fail_closed
check signed_rank signed_rank zeros_ties_limits_and_extreme_magnitudes_are_explicit exact_tied_signed_ranks_match_enumerated_sign_assignments
check signed_rank_confidence differential signed_rank_family_matches_rust wire_records_match_rust
check hodges_lehmann hodges_lehmann paired_difference_gate_uses_exact_interval_bounds_and_preserves_evidence point_estimate_retains_quarter_units_and_integer_extremes
check statistics layout survival_requires_the_interval_to_exclude_no_difference plans_are_deterministic_bounded_and_varied
check mann_whitney statistics mann_whitney_known_rank_vectors_and_ties independent_binomial_rank_and_coverage_vectors
check contract differential contract_parser_and_gate_match_rust wire_records_match_rust
check contract_gate contract_gate documented_numerical_gates_compare_exact_ratios unknown_zero_and_unsupported_evidence_are_never_passes
check wire wire malformed_records_are_typed_errors layout_is_exactly_one_padding_free_cache_line
check layout differential arithmetic_and_layout_match_rust wire_records_match_rust
check execution execution actual_monotonic_provider_executes_counted_setup_run_teardown_in_abba_order invalid_configs_execute_no_callbacks
check causal causal causal_profile_finds_the_known_bottleneck invalid_experiments_are_typed_errors
