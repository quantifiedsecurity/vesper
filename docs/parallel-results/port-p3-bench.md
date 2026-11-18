# Port wave P3a lane 5: vesper-bench

Claim: [#1040](https://github.com/robot-head/vesper/issues/1040). Baseline:
`main` at 3c075a4. The statistics, performance contracts, contract gate,
wire and layout formats, and the pure planning and folding of the execution
driver and causal profiler in `crates/vesper-bench` now run as the Vesper
Loom package `@vesper/bench` in `crates/vesper-bench/vesper/`, linked through
export ABI v2 behind the non-default `vesper-port` feature.

The Rust code stays the default build everywhere and is not deleted. By the
user's rule (2026-11-17), a port stays non-default until it matches Rust in
both functionality and performance, and the ABBA runs below measure the port
at 2.2x to 2.5x the Rust build's time on the estimators and 3.2x on the
statistics series. No dependent switches: the root and
`vesper-bench-pmu` link the frozen Rust seed `vesper-bench-seed`, and the
other ports' ABBA benchmarks keep using the Rust build (owner rule).

## API inventory (re-measured before porting)

The crate at 3c075a4 is 4.1k lines in 14 modules with 77 tests: 64
integration tests in 14 test files and 13 in-crate unit tests. Every public item keeps its
name, signature and derives in both builds:

- `arithmetic.rs`: the modular counter delta and width-specific counter
  model.
- `paired_difference.rs`: `exact_paired_differences`,
  `MAX_EXACT_DIFFERENCE_SAMPLES`.
- `walsh.rs`: `walsh_pseudomedian`, `WalshEstimate`, `MAX_WALSH_PAIRS`.
- `signed_rank.rs`: `signed_rank_exact`, `signed_rank_exact_paired_samples`,
  `SignedRankProbability`, `MAX_SIGNED_RANK_PAIRS`.
- `signed_rank_confidence.rs`: `signed_rank_confidence_set`,
  `paired_samples_confidence_set`, the classifiers and gates over them,
  `MAX_SIGNED_RANK_CONFIDENCE_PAIRS`.
- `hodges_lehmann.rs`: `hodges_lehmann_paired`, `gate_paired_difference`,
  `MAX_HODGES_LEHMANN_PAIRS`.
- `statistics.rs`: `PairedSample` (`new`, accessors,
  `regression_bp_bounds`), `analyze_pairs`, `Analysis`, `Decision`, the three
  Mann-Whitney functions, `MAX_EPOCHS`.
- `contract.rs`, `contract_gate.rs`: the `#[perf_contract]` attribute parser
  and the gate over measured evidence.
- `wire.rs`: `PmuSampleWire` (validate, encode, decode), `PmuSample`,
  `AvailabilityMask`, the backend-bound share.
- `layout.rs`: `layout_plan`, `layout_sample`, `layout_survival`,
  `run_randomized_layouts`, `HeapShim`, `OffsetBuffer`, env padding.
- `execution.rs`: `run_abba`, `run_abba_observed`, `RunConfig`,
  `BenchCase`, `WindowObserver` and their reports and errors.
- `causal.rs`: `CausalProfiler`, `profile`, `CausalConfig`, `CausalReport`.
- `lib.rs`: `IMPLEMENTATION` and the evidence requirement types.

The prompt describes the statistics as `f64` code. They are not: every
statistic is exact integer or `u128`/`i128` arithmetic (ratios compare by
cross-multiplication, probabilities are numerator/denominator pairs), so the
port is integer code throughout and matches bit for bit by construction.

## How the port is built

- **Seed pattern.** `crates/vesper-bench` (`vesper-bench`) builds Rust by
  default; `vesper-port` links `@vesper/bench` and sets `cfg(vesper_port)`,
  under which each ported function's body is a single call into the port
  (`#[cfg(vesper_port)] return port::f(..);`, the Rust body kept under
  `#[cfg(not(vesper_port))]`). `crates/vesper-bench/seed`
  (`vesper-bench-seed`) compiles the same `src/` without the feature and is
  what the root and `vesper-bench-pmu` link, so the compiler's closure never
  holds a port-backed build (`cargo tree -p vesper -e normal,build` shows
  only the seed; `closure_guard` passes).
- **128-bit values.** Vesper has no `i128`/`u128`, so the port carries them
  as `W { hi, lo }` two's-complement pairs (`vesper/src/wide.ves`): add, sub,
  negate, signed and unsigned compare, `mul_wide`-based 64x64 products, and
  division (one-word fast path, a 32-bit-limb path for divisors below 2^32,
  and a 128-step shift-subtract fallback).
- **Total arithmetic.** Every `+`, `-`, `*`, shift and index is either a
  wrapping builtin, a `try_get`/`try_set`, or a helper in `util.ves`, so no
  input can raise `Fail[ArithOverflow]` or `Fail[RuntimeError]` past the
  export boundary. Where Rust returns `ArithmeticOverflow`, the port raises
  the same typed error.
- **Boundary types.** Types named like crate types reuse the crate's types
  (`--rust-types crate::port`); the rest (`W`, `Sample`, `Segment`, the error
  and result records) are port-only records converted in `src/port.rs`. 40
  functions are exported.
- **One difference outside Rust's defined range.** `causal::predicted_bp`
  multiplies three 64-bit values in `i128`, which overflows when
  `effective_ns * progress * 10_000` passes 2^127: the Rust build panics in
  debug and wraps in release. The port raises `ArithmeticOverflow` there
  instead. Everywhere Rust's result is defined, the two agree; no test or
  caller reaches that range. The Rust code is unchanged.

## What stays in Rust, and why

- The clock (`Instant`), the callback-driven ABBA run loop and the
  `BenchCase`/`WindowObserver` traits: OS time and caller closures. The loop
  hands each epoch's four window samples to the port's `fold_epoch` and the
  config to the port's `validate_config`.
- `CausalProfiler`'s threads, atomics and pauses, the `profile` loop and
  region-name validation (strings checked against Rust identifiers). The
  port computes each experiment run, the median prediction, the predicted
  basis points and the region delay.
- `HeapShim`, `OffsetBuffer`, stack padding and the env-padding string:
  allocation and stack-frame placement. The port plans the layouts and
  scores survival.
- `WireError::Unavailable`'s field list (a `Vec<&'static str>` assembled
  from the mask the port checks).

No unsafe code was added outside the generated binding (`port/binding.rs`,
allowed in `src/lib.rs` only for that module); the crate stays
`forbid(unsafe_code)` in the Rust build and `deny(unsafe_code)` in the port
build.

## Tests

- All 64 integration tests run unchanged against the seed, the Rust
  package and the port.
- The 13 unit tests: 11 stay in `src/` (seven on the execution run loop
  and observers, four on the causal profiler; they run on both builds); the two `signed_rank_confidence.rs` classifier
  modules moved to `tests/gate_classification.rs` through
  `#[doc(hidden)] pub` entry points (`classify_signed_rank_set`,
  `compare_paired_endpoint`), so they run on both builds.
- `tests/differential.rs` (9 tests) compares every ported function's
  `Debug` output with the seed's over xorshift-generated inputs: tied,
  zero, extreme and fractional differences up to 32 pairs, paired samples
  up to 64 with tied ratios, contracts, wire records and evidence.
- `tests/implementation.rs` checks that the feature selects the build and
  that the port leaks nothing (`port_live_allocations`).

## Divergence checks

`build.rs` has one edit per ported module (14), chosen by
`VESPER_BENCH_DIVERGE` under `vesper-port-divergent`. `port/check.sh` (shard
3 of `scripts/test-port-harness.sh`) builds each one and requires the named
test to fail while an unrelated test in the same file passes:

| Module | Edit | Fails | Still passes |
| --- | --- | --- | --- |
| arithmetic | counter delta reversed | `statistics::width_specific_counter_model_has_explicit_wrap_and_range_limits` | `known_regression_improvement_ties_and_mixed_results` |
| paired_difference | fractional reported as out of range | `paired_difference::fractional_and_out_of_range_differences_fail_at_their_index` | `exact_per_operation_differences_preserve_sign_and_order` |
| walsh | odd and even medians swap | `hodges_lehmann::point_estimate_retains_quarter_units_and_integer_extremes` | `bounds_and_ambiguous_ranks_fail_closed` |
| signed_rank | each tie group counts twice | `signed_rank::zeros_ties_limits_and_extreme_magnitudes_are_explicit` | `exact_tied_signed_ranks_match_enumerated_sign_assignments` |
| signed_rank_confidence | inversion at 10%, not 5% | `differential::signed_rank_family_matches_rust` | `wire_records_match_rust` |
| hodges_lehmann | critical rank at 10% | `hodges_lehmann::paired_difference_gate_uses_exact_interval_bounds_and_preserves_evidence` | `point_estimate_retains_quarter_units_and_integer_extremes` |
| statistics | median tail at 2.5%, not 5% | `layout::survival_requires_the_interval_to_exclude_no_difference` | `plans_are_deterministic_bounded_and_varied` |
| mann_whitney | each tie counts twice | `statistics::mann_whitney_known_rank_vectors_and_ties` | `independent_binomial_rank_and_coverage_vectors` |
| contract | stabilizer key misspelled | `differential::contract_parser_and_gate_match_rust` | `wire_records_match_rust` |
| contract_gate | a count at the limit fails | `contract_gate::documented_numerical_gates_compare_exact_ratios` | `unknown_zero_and_unsupported_evidence_are_never_passes` |
| wire | 10,001 bp accepted | `wire::malformed_records_are_typed_errors` | `layout_is_exactly_one_padding_free_cache_line` |
| layout | heap offsets skip the last step | `differential::arithmetic_and_layout_match_rust` | `wire_records_match_rust` |
| execution | folded operations x3 | `execution::actual_monotonic_provider_executes_counted_setup_run_teardown_in_abba_order` | `invalid_configs_execute_no_callbacks` |
| causal | prediction base 20,000, not 10,000 | `causal::causal_profile_finds_the_known_bottleneck` | `invalid_experiments_are_typed_errors` |

## ABBA

`examples/abba.rs` links both builds into one binary and times them with
the seed's own `run_abba` (A = Rust, B = port, windows A B B A, one warmup
epoch, 12 measured epochs); the ratio is B/A per epoch with the exact 95%
interval from `analyze_pairs`. Both estimators accept at most 32 pairs, so
the estimator workload is many batches of n distinct paired differences,
each getting `signed_rank_confidence_set` and `hodges_lehmann_paired`. The
statistics workload is a series of 100,000 paired samples in windows of 64
(the most `analyze_pairs` takes), each window getting `analyze_pairs` and
each sample `regression_bp_bounds`. Callgrind counts come from
`BENCH_ABBA_ONLY=rust|port`, which runs one arm once and nothing else.

Setup: Linux x86_64 cloud container, 4 vCPUs, release profile, compiler
built from this branch's base, `cargo run --release -p vesper-bench
--features vesper-port --example abba`.

| Workload | Rust per pass | Port per pass | Median B/A | Exact 95% interval | Callgrind Rust / port (ratio) |
| --- | ---: | ---: | ---: | --- | --- |
| estimators, 10,000 batches, n = 8 | 295 ms | 716 ms | 2.48x | [2.29, 2.54] | 0.88G / 0.65G (0.74x)\* |
| estimators, 10,000 batches, n = 16 | 5.10 s | 11.25 s | 2.24x | [2.13, 2.26] | 7.90G / 12.00G (1.52x)\* |
| estimators, 400 batches, n = 32 | 4.83 s | 11.96 s | 2.47x | [2.44, 2.54] | 16.56G / 30.50G (1.84x)\* |
| statistics, 100,000 samples | 4.76 ms | 14.93 ms | 3.22x | [2.98, 3.26] | 36.2M / 163.9M (4.53x) |

Callgrind runs use 1,000 batches at n = 8 and 16, 100 at n = 32, and the
full 100,000-sample series; totals include process start and input
generation, which are the same in both arms.

\* The instruction totals understate the estimator gap. The Rust build
zeroes its fixed `[u64; 529]` distribution table with `memset` on every
`accepts` call, which is many instructions at very high throughput (71% of
Rust's count at n = 8, 29% at n = 16, 5% at n = 32). Comparing the hot
loop alone, the port's `accepts` runs 2.6x (n = 8), 2.2x (n = 16) and 2.0x
(n = 32) the instructions of Rust's, in line with wall clock.

Where the time goes: 97-99% of the port's estimator time is `accepts`, the
exact signed-rank test inverted at each candidate location, and inside it
the subset-sum update `dist[s + rank] += dist[s]`. Rust indexes a stack
array whose bounds checks LLVM removes; the port reads and writes a heap
`ChunkSeq` through `try_get`/`try_set` (an `Option` and a bounds test per
access), and compares 128-bit locations through the `W` emulation. In the
statistics series, 73% of the port's count is `analyze_pairs` (the ratio
sort, now a stable bottom-up merge sort, was 85% of a 1.6x larger count as
an insertion sort) where Rust's small-sort and native `u128` products are
far cheaper.

The port keeps Rust's algorithms so that the ratio measures the compiler.
One algorithmic change would help both builds equally and is left as a
follow-up: `accepts` could compute the positive and total ranks first and
run the subset-sum only up to the tail limit, which is far below the total
near the interval's ends.

At n = 32 a Rust pass over 10,000 batches takes about two minutes (each
batch inverts the exact signed-rank test over up to 528 Walsh
breakpoints), and the seed's run loop stops at a 600 s budget, so that row
uses 400 batches; per-batch cost is what the ratio measures.

## Compiler gaps (with repros)

1. **No 128-bit integers.** `fn f(x: i128) -> i128 { x }` gives `E0101
   unknown type 'i128'`. The crate is built on exact `u128`/`i128`
   products and binomial tails; the port emulates them (`wide.ves`), and the
   emulated compares and divisions are a large share of the remaining
   instruction gap.
2. **No fixed-size arrays, and no bounds-check elimination on sequences.**
   `fn f() -> [u64; 4] { [0; 4] }` gives `E0002 expected RBracket, found
   Semi`. Every scratch table (rank sums up to 528, breakpoints, Walsh
   averages) is a heap `ChunkSeq` read through `try_get`/`try_set`, where
   Rust uses stack arrays whose checks LLVM removes. This is the main
   estimator gap: the loop `for s in (0..=total).rev() { dist[s + rank] +=
   dist[s] }` (port: `util.ves` `add_rank`) is 97-99% of the port's time.
3. **Shifts by a variable are partial.** `fn f(a: u64, n: u64) -> u64 { a
   << n }` gives `E0401 unhandled effect 'Fail'`; total code needs
   `wrapping_shl_u64(a, wrapping_u32(n))`, which is fine but verbose. (The
   guard-range totality gap noted in the bit-builtins lane.)
4. **Unsuffixed literals default to `i64`** even where the binding's type
   is `u64` by inference from the other branch, so `let x = if c { 0 }
   else { y_u64 }` needs `let x: u64`. Ergonomic only.
5. **`state` is a keyword**, so ported parameters named `state` had to be
   renamed (`st`). Ergonomic only.

## Std candidates

- `W`: 128-bit two's-complement helpers (`w_add`, `w_sub`, `w_mul_u64`,
  `w_udivrem` with the small-divisor path, signed and unsigned compare,
  `w_fits_i64`) until the compiler has native 128-bit integers.
- Stable sorts over a `ChunkSeq` with a comparator (the port has a heapsort
  for `u64`/`W` and a stable bottom-up merge sort for samples).
- Total getters with a default (`try_get` plus a fallback value), used on
  every scratch read.

## Remaining gaps

- Performance parity: see the ABBA table. The port stays non-default.
- The Rust-only adapters listed above stay Rust by design (OS time,
  threads, allocation placement).
