# Port wave P1 lane 9: Grammar (#906)

All of `crates/vesper-grammar` runs in Vesper in the port-backed package
`vesper-grammar`, from the Loom package `@vesper/grammar` in
`crates/vesper-grammar/vesper/`: framing, primitive Arrow validation, Cap'n
Proto framing and packing, the executable grammar and its logical-value
codec, Protobuf, DER and Arrow IPC. Every test passes against both builds.
The port is about 6.3x slower than Rust on Protobuf decode and about 4.7x
slower on DER decode, so the 2x stop rule applies: **the Rust code is not
deleted**, and the Rust build stays the default for every dependent (the
user's standing rule of 2026-11-17: the Vesper build stays non-default until
it matches Rust in function and performance).

## Packages (seed-copy rule)

Grammar is linked by the compiler (`src/grammar_commands.rs`), so it follows
Beacon's seed-copy rule from #899:

- `crates/vesper-grammar/seed` is the package `vesper-grammar-seed`, the Rust
  build of `../src` under the library name `vesper_grammar`, with no build
  script. The root depends on it by rename
  (`vesper-grammar = { path = "crates/vesper-grammar/seed", package =
  "vesper-grammar-seed" }`). No other crate depends on Grammar today, so
  every dependent links Rust.
- `crates/vesper-grammar` is the port-backed package: `build.rs` builds the
  Loom package with `vesper_port_harness::Port`, links it, checks
  `port/binding.rs` against the generated binding and sets
  `cfg(vesper_port)`. Each ported Rust item is `#[cfg(not(vesper_port))]`,
  followed by a `#[cfg(vesper_port)]` item with the same signature that
  forwards to the port. The Rust bodies are unchanged.
- The closure guard (`vesper-port-harness` `tests/closure_guard.rs`) still
  passes: the port-backed package is outside `cargo tree -p vesper -e
  normal,build`.

## API inventory (measured before porting)

Public API of each module, unchanged by the port (both builds share the
types, `Display` impls and accessors):

- `lib.rs`: `ErrorKind` (25 variants), `Error`, `Result`.
- `framing.rs`: `Limits`, `Frame`, `Remainder`, `TailError`,
  `decode_canonical_u64_leb128`, `encode_u64_into`, `parse_frame(_exact)`,
  `parse_u24_frame(_exact)`, `encode_u24_frame_into`, `parse_u64(_exact)`,
  `frame_encoded_len`, `encode_frame_into`.
- `arrow.rs`: `AlignmentProfile`, `BufferDesc`, `FieldNode` (`from_wire`),
  `Limits`, `checked_buffer_range`, `I64Column` (`validate`, `len`,
  `is_empty`, `null_count`, `values_bytes`, `validity_bytes`, `get`,
  `checked_sum`).
- `capnp.rs`: `Limits`, `Message`, `Segments`, `StructPointer`,
  `ListPointer`, `ListKind`, `local_struct_pointer`, `local_list_pointer`,
  `parse_prefix`, `parse_exact`.
- `capnp_packed.rs`: `ErrorKind`, `Error`, `Limits`, `unpacked_len`,
  `unpack_into`, `packed_len`, `pack_into`.
- `grammar.rs`: `Rule`, `Branch`, `Limits`, `Capture`, `Parsed`, `Grammar`
  (`validate`, `nullable`, `first_contains`, `parse_prefix`, `parse_exact`).
- `grammar/codec.rs`: `CodecLimits` (`validate`), `Value`, `decode_exact`,
  `encoded_len`, `encode_into`.
- `protobuf.rs`: `Kind`, `FieldSchema`, `MessageSchema`, `Message`,
  `FieldValue`, `Value`, `ErrorKind`, `Error`, `Limits`, `PROFILE`,
  `MAX_WIRE_BYTES`, `Schema` (`validate`, `parse_exact`, `encoded_size`,
  `encode_into`).
- `der.rs`: `Limits`, `ErrorKind`, `Error`, `Schema`, `Kind`, `Document`,
  `Node`, `Children`, `Value`, `EncodingPlan`, `parse_prefix`, `parse_exact`,
  `encoding_plan`, `encoded_len`, `encode_into`, `reencode_into`,
  `Node::as_i64`.
- `ipc.rs`: `ErrorKind`, `Error`, `Limits` (`validate`), `Field`, `Schema`,
  `Batch`, `Stream`, `Parsed`, `parse_exact(_with_alignment)`,
  `parse_prefix(_with_alignment)`.

In-crate unit tests: none. Integration tests: 125 in 10 files (`arrow` 7,
`capnp_packed` 6, `codec` 15, `codec_acceptance` 13, `der` 18, `framing` 15,
`grammar` 8, `ipc` 11, `protobuf` 21, `repetition` 11), plus 5 doctests (2 of them compile-fail checks) and
five examples.

## What was ported

| Rust | Vesper module | Notes |
| --- | --- | --- |
| `framing.rs` | `framing.ves` | LEB128, dependent and u24 frames. |
| `arrow.rs` | `arrow.ves` | Range checks, `I64Column::validate`, `get`, `checked_sum`. |
| `capnp.rs`, `capnp_packed.rs` | `capnp.ves` | Segment table framing, local pointers, packing both ways. |
| `grammar.rs` | `grammar.ves` | `validate` (cycles, FIRST, nullable, height, budgets) and the executor. |
| `grammar/codec.rs` | `codec.ves` | Decoder, measured plan and writer with the same budgets. |
| `protobuf.rs` | `protobuf.ves`, `utf8.ves` | Schema validation, parser, sizer, writer. |
| `der.rs` | `der.ves` | Parser, encoding plan, SET OF sort, encoder, re-encoder, `as_i64`. |
| `ipc.rs` | `ipc.ves` | FlatBuffer checks, regions, schema and batch messages. |
| shared helpers | `common.ves`, `lib.ves` | Checked arithmetic, failure record, exports. |

Every export raises one failure type, `GErr { kind, offset, a, b }`, where
`kind` is the variant index in the Rust module's own `ErrorKind`; payload
variants (`der::ErrorKind::UnsupportedTag`, `ipc::ErrorKind::Column`) carry
their fields in `a` and `b`. Limits cross as word slices in field order,
followed by the Rust `size_of` values that the Rust budgets charge (Meta,
Capture, Record, Region, Field, I64Column, `Range<usize>`, Batch, codec
writes), so the port refuses the same inputs at the same budget. Trees cross
as flat word streams: rules, codec values and Protobuf messages in prefix
order, DER schemas and values as graphs numbered by address (shared nodes
once, which keeps DAG inputs linear), DER documents as their six-word
records, IPC streams as field, batch and column words. `src/port.rs` and the
`port` module at the end of `der.rs` and `ipc.rs` encode the inputs and
rebuild the Rust results; caller-supplied trees are flattened iteratively,
so deep untrusted input cannot exhaust the stack before the depth limit.

The port uses no `unsafe` (the Rust side keeps `forbid(unsafe_code)` in the
seed build and `deny(unsafe_code)` in the port build, with only the
generated binding allowed).

## What stays Rust, and why

- Type definitions, `Display`/`Error` impls, constructors and accessors
  (`Document`/`Node`/`Children`, `Stream`/`Batch`/`Schema`, `Segments`,
  `Remainder`, `Limits::default`): they hold borrowed views of the input that
  only Rust can hand back, and they do no checking.
- The wrappers built on ported functions (`parse_frame`, `parse_exact`
  variants, `encoded_len` over `encoding_plan`): they forward.
- `ipc::Limits::validate` stays public Rust; the port repeats the same check
  inside `parse_prefix`.
- Allocation failure (`AllocationFailed`): the Rust build reports a failed
  `try_reserve`; the port's sequences abort on allocation failure instead.
  Every budget is checked before either build allocates, so no test or
  realistic input reaches this.

## Unit tests

No in-crate unit tests existed. All 10 integration test files run unchanged
against both packages (`seed/Cargo.toml` lists them as `[[test]]` targets of
the seed package). New: `tests/no_panic.rs` (below) and
`tests/divergence.rs` (the Bazel divergence runner).

## No panic on untrusted bytes

The existing malformed-input tests pass against the port (`ipc`
`bounded_byte_mutation_never_panics`, `der`
`deterministic_byte_mutations_never_publish_noncanonical_roundtrips`, every
truncation and hostile-input test). `tests/no_panic.rs` adds 10^5 random
inputs (seeded xorshift, lengths 0-511, mixed byte styles) through every
decoder: LEB128, frames, u24 frames, Cap'n Proto framing, packed length and
unpack, the grammar executor, the codec decoder, Protobuf, DER and Arrow IPC.
It folds every outcome (value or error, with offsets) into a digest. Both
builds pass, and both print the same digest (`b830815282d14a4c`), so the
port agrees with Rust on all 1.3 million decodes, errors included.

## Divergence checks

`build.rs` `DIVERGENT` holds one wrong line per ported module, applied under
the `vesper-port-divergent` feature (`VESPER_GRAMMAR_DIVERGE=<module>`
selects one); `BUILD.bazel` mirrors them as one divergent object per module.

| Module | Edit (Vesper file) | Test file | Fails | Still passes |
| --- | --- | --- | --- | --- |
| framing | LEB128 shift 7 -> 6 per byte (`framing.ves`) | `framing` | `known_uleb_vectors_exact_prefix_and_transactional_output` | `u24_big_endian_frames_roundtrip_and_preserve_remainder` |
| arrow | `checked_sum` misses positive overflow (`arrow.ves`) | `arrow` | `checked_nullable_aggregation_matches_i128_prefix_oracle` | `alignment_checks_distinguish_relative_offsets_from_actual_host_addresses` |
| capnp | segment table pads the wrong counts (`capnp.ves`) | `framing` | `capnp_stream_segments_are_borrowed_and_framing_is_exact` | `u24_boundaries_fail_transactionally` |
| capnp_packed | a run tag counts one word too many (`capnp.ves`) | `capnp_packed` | `official_encoding_vectors_and_raw_runs_with_zeroes` | `every_short_output_preserves_the_whole_buffer` |
| grammar | hard depth ceiling 64 -> 3 (`grammar.ves`) | `grammar` | `aliased_dag_expansion_cannot_bypass_capture_or_work_budgets` | `ambiguity_duplicate_branch_ids_bad_references_and_recursion_fail` |
| codec | encoder plans numbers one byte long (`codec.ves`) | `codec` | `independently_constructed_numbers_have_known_canonical_encodings` | `codec_limit_configuration_is_checked_before_all_operations` |
| protobuf | negative int32 not sign-extended (`protobuf.ves`) | `protobuf` | `signed_twos_complement_varints_are_canonical_in_singular_and_packed_fields` | `singular_presence_and_defaults_are_actual_distinct_logical_values` |
| der | SET OF order checked backwards (`der.ves`) | `der` | `typed_set_of_sorts_complete_encodings_retains_duplicates_and_rejects_untyped_set` | `utf8_string_is_strict_borrowed_and_transactional` |
| ipc | metadata V4 (version 3) refused (`ipc.ves`) | `ipc` | `independent_arrow_values_and_versions` | `hostile_schema_and_union_discriminants` |

Each row was checked twice locally: with only that module's edit (attribution:
the failing test fails because of that edit) and with all edits at once (the
Cargo harness build).

Under Cargo, `scripts/test-port-harness.sh` builds all edits at once and
asserts each row's failing and passing test. Under Bazel,
`port_divergence_detected` runs each module's divergent test binary (its
test file linked against that module's divergent object) and asserts the
same pair.

## Benchmarks

`examples/grammar_abba.rs` (built by the seed package as
`grammar_abba_seed`): `Schema::parse_exact` of a 909-byte Protobuf record (a
scalar, a string, 16 nested messages, 128 packed u32) and `der::parse_exact`
with `Schema::Any` of a 1,764-byte certificate-shaped DER tree (16 entries of
INTEGER, OID, UTF8String, BIT STRING, OCTET STRING). 20,000 decodes per
window, 9 ABBA epochs, two release binaries (Rust seed arm A, port arm B),
paired with `vesper_bench::analyze_pairs`. Setup: cloud x86-64 container (4
vCPU), release profile, port compiled by `vesper` from main at 27fcfbe
(including the Stage 1 inliner, byte builtins and the parity-wave changes).

| Workload | Rust (A) | Port (B) | Median regression | 2x gate |
| --- | --- | --- | --- | --- |
| Protobuf decode | 3.1-4.0 us | 19.7-21.6 us | +53,429 bp (6.3x) | Regression |
| DER decode | 3.3-4.2 us | 15.0-19.4 us | +36,926 bp (4.7x) | Regression |

Under callgrind the instruction ratio is 4.6x (Protobuf) and 3.2x (DER). The
port's profile is flat across many small Vesper functions; the Rust side's
rebuild of the result (`port::protobuf_message`) is about 4% of the port's
instructions. The likely costs, from the code: every byte read goes through
a bounds-checked `Option` accessor, every count through a checked add or
`mul_wide`, every node pushes words to a `ChunkSeq`, and results cross as a
word stream the Rust side copies and rebuilds. Closing the gap needs compiler
work (cheaper element access and pushes, more inlining of the checked
helpers) plus a borrowed-result ABI that avoids the rebuild.

## Platforms run

- Linux x86-64 (cloud container): both packages, all tests, doctests and
  examples; the divergent build per module; `tests/no_panic.rs` on both;
  `cargo +1.100.0 clippy --all-targets -p vesper-grammar -p
  vesper-grammar-seed -- -D warnings`; `vesper fmt --check` on every `.ves`
  file; the ABBA benchmark.
- Linux (Bazel), macOS AArch64 and Windows x86-64 MSVC: through CI on the PR
  (Bazel targets in `crates/vesper-grammar/BUILD.bazel` and
  `crates/vesper-grammar/seed/BUILD.bazel`; Cargo lines in
  `scripts/test-port-harness.sh`).

## Compiler notes

- Compile time: the 5.5k-line package takes about 2 minutes to build with a
  debug `vesper` (about 50 s with `--no-opt`). Every build-script run of the
  port-backed package pays it.
- Workarounds this port needed, each with a small repro in the code:
  variable shifts and `*` raise `ArithOverflow` (helpers `shl`, `shr`,
  `wrapping_mul`); `for i in 0..3` infers `i64` (bind the bound to a `u64`);
  a slice view is only allowed as an argument, not a receiver
  (`same(in a[..], in b[..])` instead of `a[..].eq(...)`); `try_copy_from`
  takes a `Slice` but not a `ChunkSeq`; reading `out.len()` inside
  `out.try_set(..)` is a loan conflict (E0410). Remapping an arrow failure
  to `ipc::ErrorKind::Column` uses a handler that returns a tagged result,
  which the caller turns back into a failure.

## Was the Rust code deleted?

No. The port is over 2x slower (stop rule), and the user's rule keeps the
Vesper build non-default and the Rust code in place until parity. The Rust
code is frozen to bug fixes (README line linking #906).
