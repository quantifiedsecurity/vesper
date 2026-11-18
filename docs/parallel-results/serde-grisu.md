# vesper-serde port: Grisu f64 encode and the scalar stage 1 split (#982)

Follow-up to port P1 lane 19 (#916, PR #975), stacked on that branch. Rust
stays the default build and nothing was deleted (the owner's parity rule).

## Grisu with the Dragon fallback

`crates/vesper-serde/vesper/src/grisu.ves` is Rust's
`core::num::flt2dec::strategy::grisu::format_shortest_opt`: the 81-entry
cached power table (alpha = -60, gamma = -32, as a literal `match`, checked
against the generator in the Rust source's comment), `Fp::mul` as the high
word of `mul_wide` rounded half up (equal to Rust's four partial products),
digit generation and `round_and_weed`, with Rust's wrapping arithmetic.
When Grisu gives up, `dragon.ves` runs the existing exact Dragon, as
`grisu::format_shortest` does, so the text is Rust's `{}` in every case.

The digits now travel as one integer (at most 17 digits) in a `Shortest`
record instead of a `ChunkSeq[u8]`; `shortest_len` sizes the text and
`put_shortest` writes it straight into the output slice, so the fast path
allocates nothing. The preflight and the write each format once, as the Rust
encoder does.

Tests: `tests/float_encode.rs` gains
`text_is_exactly_rusts_display_on_the_grisu_and_dragon_paths`, which checks
the written text equals `format!("{value}")` (the old tests only checked that
the text round-trips, which a non-closest shortest digit string still does)
over 12 values on which Grisu gives up (found with a model of
`format_shortest_opt`; about one random value in 200 falls back), boundary
values, five fraction patterns for every exponent, 200,000 random finite bit
patterns, and 40,000 integers and short decimals. Both packages pass all
294 tests.

Divergence checks (build.rs `DIVERGENT_EDITS`, Bazel objects, and
`port/check.sh`), both caught by the new test while
`all_nonfinite_sign_and_payload_classes_refuse_before_any_nested_output_write`
passes:

| Module | Edit |
| --- | --- |
| grisu | weeding keeps the last digit (still round-trips) |
| dragon | the Dragon fallback never rounds its last digit up |

The existing `encode` edit (minus sign printed as `+`) moved to
`put_shortest` and is still caught.

## Benchmark

`examples/serde_abba.rs`, same setup as the lane report (shared 4 vCPU cloud
container, both arms `--release`, Rust 1.100, port built by the in-tree
compiler with the inliner on). Port / Rust, median of 9 ABBA epochs:

| Workload | Before (#975) | Now |
| --- | --- | --- |
| f64 encode, 4,000 17-digit values | 64.9x | **2.40x** (2.01-2.63) |
| scalar parse | 4.83x | 5.08x (unchanged code; noise) |
| two-stage SIMD parse | 5.70x | 5.12x (unchanged code; noise) |
| f64 decode | 5.23x | 5.20x (unchanged code) |
| telemetry tiles | 4.67x | 4.05x (unchanged code; noise) |

f64 encode is still over the 2x gate, so Rust stays the default. Callgrind
of the port's encode: `grisu_shortest` is about half of the instructions,
and most of its own cost is the prologue zeroing every stack slot twice
(a 1 KiB frame for about 120 locals), the stack-slot locals and zeroing gap
the compiler-gap thread (#972) is fixing. The rest is the adapter's
flattening of the `Value` tree into a word stream (one allocation per call
the Rust build does not make), which goes once borrowed values can cross
the export ABI.

## Scalar stage 1 split

The scalar stage 1 (`classify`, the per-bit `reference_block`, and the
`Isa::Scalar` `run`) moved from `src/simd/mod.rs` into `src/simd/scalar.rs`.
It now shares no file with the SIMD kernels (`kernels.rs`) or the index
machinery (`Sink`, `StructuralIndex`, `block_finish` in `mod.rs`), so it is a
self-contained, safe-Rust codec a port can replace. It is not ported yet: a
port needs the stage 1 index (`Sink`'s `u32` buffer) to cross the export ABI
by `inout`, which is the next step. No behavior changed; the Bazel and seed
globs pick up the new file.
