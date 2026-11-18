# Port wave P1 lane 4: vesper-beacon

Claim: [#901](https://github.com/robot-head/vesper/issues/901). Baseline:
`main` at 69c79b9 (a descendant of d523661). The whole runtime of
`crates/vesper-beacon` (both histograms, the v00 traceparent parser and
formatter, the span pool and the telemetry ring) now runs as the Vesper Loom
package `@vesper/beacon` in `crates/vesper-beacon/vesper/`, linked through
export ABI v3 behind the non-default `vesper-port` feature. It replaces lane
2's one-function stub.

The Rust code stays the default build everywhere and is not deleted. By the
user's rule (2026-11-17), a port stays non-default until it reaches
reasonable performance parity, and the benchmarks below measure the port at
2.1x to 7.1x the Rust build's time. This overrides the plan's default that
dependents outside the compiler's closure switch to the port.

## API inventory (re-measured before porting)

Public items, all kept with the same names, signatures and derives:

- `lib.rs`: `IMPLEMENTATION`, `Secret<T>` (`new`, `redact`, `declassify`),
  `Redacted` (`as_bytes`).
- `histogram.rs`: `HISTOGRAM_BINS`, `HistogramError`, `BinRange`
  (`estimate`), `FastLogHistogram` (`Default`, `FOOTPRINT_BYTES`,
  `bucket_index`, `bin_range`, `record_with_exemplar`, `sum`, `min`, `max`,
  `total_count`, `count`, `exemplar`).
- `exponential.rs`: `EXP_HISTOGRAM_BUCKETS`, `EXP_HISTOGRAM_MAX_SCALE`,
  `EXP_HISTOGRAM_MIN_SCALE`, `exp_bucket_index`, `ExponentialHistogram`
  (`Default`, `FOOTPRINT_BYTES`, `scale`, `offset`, `positive_counts`,
  `zero_count`, `total_count`, `sum`, `min`, `max`, `record`).
- `traceparent.rs`: `TraceError` (+ `Display`, `Error`), `W3cSpanContext`
  (`new`, `trace_id`, `span_id`, `trace_flags`, `sampled`,
  `to_traceparent`), `parse_traceparent_55`.
- `spans.rs`: `SpanSchema`, `ErrorCode`, `PrimaryAttribute`, `SpanStatus`,
  `StartError`, `FinishError`, `SpanPool<ACTIVE>` (`new`, `unclosed_count`,
  `start_span`), `ActiveSpan` (`context`, `cancel`, `finish_ok`,
  `finish_err`), `FinishFailure<S>`, `SpanRecord`, `TelemetryRing<QUEUE>`
  (`Default`, `len`, `is_empty`, `records`, `try_consume_prefix`, `drain`),
  `ColumnBatch<BATCH>` (column accessors), `SpanRecords` (`Iterator`,
  `ExactSizeIterator`, `FusedIterator`).
- `redaction.rs`: `Pii<T>` (`new`, `redact`, `declassify`), `Sensitive`
  (sealed), `TelemetryValue`, `Telemetrable` and its impls.

In-crate unit tests: two, both in `src/histogram.rs`
(`all_ranges_partition_u64_and_bound_error`,
`checked_counters_are_transactional`). Doctests: eight, all `compile_fail`
(the type-discipline checks of `Secret`, `Pii`, `Telemetrable`, `ActiveSpan`
and `try_consume_prefix`).

## How the port is built

- **State stays in Rust, lent in place.** Each stateful type keeps its
  fixed-size arrays in the Rust struct (`[u64; 496]` counters and
  exemplars, `[u64; 160]` exponential counts, `[u8; ACTIVE]` pool slots,
  `[SpanRow; QUEUE]` ring records) and its scalars in a `repr` mirror of a
  Vesper copy struct (`HistogramStats`, `ExpState`, `PoolState`,
  `RingState`). The binding lends them as `&mut [u64]`, `&mut [u8]`,
  `&mut [repr::SpanRow]` and `&mut repr::*` (export ABI v3, #934), so a call
  copies nothing and neither build allocates. The const generics `ACTIVE`,
  `QUEUE` and `BATCH` need no generic instances: Vesper sees each as a
  slice length.
- **No `u128`.** Vesper has no 128-bit integers, so trace IDs and the exact
  sums cross as (high, low) words; the exponential histogram's Q1.63
  squaring uses `mul_wide`.
- **The consumer API.** `try_consume_prefix` asks Vesper for the prefix
  length, runs the consumer over a `SpanRecords` view whose `next` reads
  one ring row through Vesper, and pops the prefix through Vesper only if
  the consumer succeeded. `drain::<BATCH>` lets Vesper move up to `BATCH`
  rows into a stack buffer, then fills the `ColumnBatch` columns.
- **Errors.** `HistogramError`, `TraceError`, `StartError` and
  `FinishError` are the crate's own enums (`--rust-types crate::port`).
  Functions whose row also has `Fail[ArithOverflow]` (variable shifts and
  checked arithmetic) map it to `unreachable!`: no input reaches it, and
  the tests cover the edge values (`u64::MAX`, every bin bound, both
  scale limits).
- **Speedups in the source.** `find_byte` finds the free pool slot; the
  traceparent IDs are decoded and encoded eight bytes at a time
  (`try_read_u64_be`/`try_write_u64_be` and SWAR arithmetic), with the byte
  loop kept as the slow path that reports the offending offset; the highest
  set bit comes from the exponent of `nearest_f64` instead of a branch tree;
  the squaring loop is branch-free. Before these, the port was 5x to 19x
  slower.

## What stays in Rust, and why

| Part | Reason | Unblocked by |
|---|---|---|
| The whole crate as the default build, and the seed package `vesper-beacon-seed` | The user's parity rule; the compiler links the seed until Stage 2 | Performance parity; Stage 2 |
| `Secret`, `Pii`, `Redacted`, `Sensitive`, `Telemetrable`, `TelemetryValue` (`lib.rs`, `redaction.rs`) | Type-level markers: their checks are Rust trait coherence and `compile_fail` doctests, and their runtime is a constant marker (`as_bytes` is a `const fn` returning `&'static [u8]`) and field reads. There is no logic to move | - |
| Field accessors (`trace_id`, `sampled`, `len`, `sum`, `min`, `max`, ...) | They are `const fn` field reads (`sampled` is one mask); a C call cannot be `const` | - |
| `TraceError`'s `Display` | Formatting of the crate's error text | - |
| The row encoding of `SpanStatus` and `PrimaryAttribute` (`src/port.rs`) | `PrimaryAttribute::Redacted` holds the crate-private `Redacted` marker, so the ring rows carry (kind, payload) codes | - |

No unsafe code was added outside the generated binding (`port/binding.rs`).
The Rust wrappers in `src/` are safe; the crate keeps `forbid(unsafe_code)`
in the Rust build and `deny(unsafe_code)` in the port build.

## Builds and dependents

- `vesper-beacon` builds the Rust code by default. Its build script always
  depends on `vesper-port-harness` (so the seed-copy guard still counts it
  as port-backed), but links the port only under `vesper-port`.
- The compiler closure links `vesper-beacon-seed`, unchanged.
- `vesper-beacon-arrow` keeps the Rust build by default and gains a
  `vesper-port` feature that forwards to Beacon's; its `beacon_build` test
  checks the selected build.

## Unit tests

- `all_ranges_partition_u64_and_bound_error`: **translated** to
  `tests/histogram.rs`, which runs against both builds (plus
  `records_track_counts_exemplars_sum_min_and_max` and
  `footprint_is_fixed`). The Rust-only original stays in the Rust build.
- `checked_counters_are_transactional`: reaches the overflows by setting
  private fields. **Ported** for the port build as
  `port_tests::checked_counters_are_transactional`, which sets the same
  overflows in the port's arrays and stats mirror; the original stays in
  the Rust build.
- `FOOTPRINT_BYTES` is 7,984 in the Rust build and 7,976 in the port build
  (the sum is two words, not a 16-byte-aligned `u128`); each build's unit
  test asserts its own value.

## Divergence checks

One deliberately divergent edit per ported module, in `build.rs`
(`vesper-port-divergent`) and `BUILD.bazel` (`:port_divergence_detected`):

| Module | Edit | Fails | Still passes |
|---|---|---|---|
| histogram | the bin offset inside an octave is off by one | `all_ranges_partition_u64_and_bound_error` | `footprint_is_fixed` |
| exponential | the window keeps 80 buckets instead of 160 | `starts_at_the_finest_scale_and_downscales_to_fit` | `bucket_index_is_exact_at_small_scales` |
| traceparent | uppercase hex letters are accepted | `lengths_separators_lowercase_version_and_nonzero_are_strict` | `drain_wraparound_partial_zero_and_fifo_ownership` |
| spans | a child may take its imported parent's span ID | `ids_never_wrap_or_repeat_and_imported_self_parent_is_skipped` | `drain_wraparound_partial_zero_and_fifo_ownership` |

All four were run locally under Cargo and each failed exactly as listed.

## Benchmarks

`tests/abba.rs` runs ABBA with `vesper-bench` (30 epochs, 2 warm-up, 16
operations per window, layout seed 1), the port (`vesper_beacon` under
`vesper-port`) against the Rust seed package in the same process, on 4,096
pseudo-random latency-like values per operation. Setup: Linux x86_64 cloud
container, Intel Xeon @ 2.10GHz, 4 vCPUs, `cargo test --release`, the port
compiled by the in-tree `vesper build --lib` (equality saturation and the
Stage 1 inliner on), 2026-11-18.

| Workload | Rust per call | Vesper per call | Vesper/Rust median (min, max) | 2x gate |
|---|---|---|---|---|
| `FastLogHistogram::record_with_exemplar` | 2.5 ns | 16.3 ns | 6.50 (4.39, 7.01) | regression |
| `ExponentialHistogram::record` | 7.5 ns | 52.9 ns | 7.07 (4.07, 7.85) | regression |
| `to_traceparent` + `parse_traceparent_55` | 100.8 ns | 209.0 ns | 2.08 (1.57, 2.44) | regression |
| span start + finish + `records` + `drain`, per span | 41.1 ns | 131.5 ns | 3.20 (2.81, 3.31) | regression |

Where the time goes (callgrind on the same workloads): every port call pays
a C call plus a stack frame that zeroes its failure record and spills its
locals, and small helpers (`slot_u64`, `bucket_index`, `high_bit`) are
called rather than inlined into the exported function. The histogram's
Rust build compiles to one `lzcnt` and two increments; Vesper has no
leading-zeros builtin. Compiler work that would close most of the gap:
inlining into exported functions, no zeroed failure records on the success
path, and a `leading_zeros` builtin. The port parity wave (#958) is the
place for that.

## Platforms run

Locally (Linux x86_64): `cargo test -p vesper-beacon -p vesper-beacon-seed`,
`cargo test -p vesper-beacon --features vesper-port`,
`cargo test -p vesper-beacon-arrow --features vesper-port` (every adapter
test on the port), the four divergent builds, `cargo test -p
vesper-port-harness` (closure guard), `cargo +1.100.0 clippy --all-targets
-- -D warnings` on both builds, `vesper fmt --check`. Linux through Bazel
(`:port_*_test`, `:port_divergence_detected`) and macOS and Windows through
`scripts/test-port-harness.sh` run in CI.

## Rust code deleted?

No. Both builds are kept, the Rust build is the default for every
dependent, and the Rust code takes bug fixes only (freeze line in the crate
README) until the port reaches parity.
