# Port wave P1 lane 7: Quarry planner (#904)

The SQL front end, catalog DDL replay, checker, predicate-placing optimizer,
reference row executor and the cost-based access planner of
`crates/vesper-quarry/spec/planner-ddl.md` run in Vesper under
`--features vesper-port`, from the Loom package `@vesper/quarry` in
`crates/vesper-quarry/vesper/`. Every test passes against both builds. The
port is about 4.3x to 4.9x slower than Rust on the parse+plan benchmark (3.2x after the compiler's new inliner landed on main), so
the 2x stop rule applies: **the Rust code is not deleted**, and the Rust build
stays the default. The user's standing rule (2026-11-17) is that the Vesper
build stays non-default until it reaches reasonable performance parity.

## API inventory (re-measured before porting)

Public API of the ported modules, unchanged by the port (both builds share
these types in the crate root):

- `src/lib.rs`: `ScalarType`, `Value` (`scalar_type`), `ColumnDef`,
  `ParamDef`, `ErrorKind`, `Error`, `Result`, `Limits` (`validate`),
  `HermeticCatalog` (`revision`, `tables`, `apply_ddl`, `prepare`).
- `src/plan.rs`: `JoinKind`, `BoundColumn`, `TypedExpr` (`scalar_type`,
  `nullable`, `dependencies`), `RelPlan` (`bindings`), `PreparedQuery`
  (`plan`, `columns`, `output`, `parameters`, `catalog_revision`,
  `optimized`).
- `src/execute.rs`: `DataSet` (`new`, `tables`, `set_rows`), `QueryResult`,
  `PreparedQuery::execute`.
- `src/parse.rs`: crate-private (`select`, `ddl`, the AST, `Cmp`).

New API added by this lane (both builds): `IndexBound`, `AccessPath`,
`BindingPath`, `AccessPlan` (with a `Display` form such as
`o=index(o_owner,eq)`) and `PreparedQuery::access_plan(data, indexes,
params)`. The Rust build returns `ErrorKind::Unsupported` from
`access_plan`: the planner of the spec exists only in Vesper. Also
`port_live_allocations()` (`#[doc(hidden)]`, 0 in the Rust build) for the
leak check.

In-crate unit tests: one, `lib.rs`
`checked_counters_and_revision_overflow_fail_atomically`.

Parts that stay Rust (named by the lane prompt): `store/`, `pool.rs`,
`wal.rs`, `durable.rs`, `device.rs`, `page.rs`, `vector*`.

## What was ported

| Rust | Vesper module | Notes |
| --- | --- | --- |
| `parse.rs` lexer | `lex.ves` | Keywords resolved at lex time. |
| `parse.rs` parser | `parse.ves` | Builds flat `Raw`/`Sel`/`Bind` records. |
| `lib.rs` `apply_ddl_changes`, `parse.rs` `ddl` | `ddl.ves` | Atomic replay, revision overflow. |
| `plan.rs` `Checker`, `prepare` | `check.ves`, `plan.ves` | Typed expressions as prefix word streams. |
| `plan.rs` `optimize`, `place` | `plan.ves` | Same placement and FULL/outer rules. |
| `execute.rs` `run`, `eval`, `truth`, `Budget` | `values.ves`, `rows.ves`, `entry.ves` | Same work and byte charges. |
| `execute.rs` `set_rows` validation | `entry.ves` | |
| spec/planner-ddl.md §1-2 access planner (new) | `stats.ves`, `paths.ves` | Cost model, ties, unknown params. |
| catalog types, shared helpers | `lib.ves`, `util.ves` | |

Plans cross the ABI as flat word streams (`[tag, ...]` in prefix order, text
as `(offset, length)` ranges of one byte pool); `src/port.rs` encodes inputs
and rebuilds `RelPlan`, `TypedExpr` and results. The encodings are
documented at the top of `src/port.rs` and `vesper/src/lib.ves`. Every entry
point returns one owned `Q` handle with a sticky error (`err`, `msg`):
`Fail[E]` cannot cross an active `inout` parameter (E0402), so the port uses
sticky errors and wrapping arithmetic with explicit bound checks instead.

The package is split into 12 modules because `vesper fmt` refuses files over
8192 tokens; every file is canonically formatted (`vesper fmt --check`).

Thin Rust adapter kept: `src/port.rs` (encode/decode only). The binding
(`port/binding.rs`) is the only `unsafe`; the crate root keeps
`forbid(unsafe_code)` in the Rust build and `deny(unsafe_code)` in the port
build, with `src/port.rs` allowed.

## Unit tests and test changes

- `lib.rs` `checked_counters_and_revision_overflow_fail_atomically`: kept;
  runs in both builds (the overflow path goes through the port's DDL replay
  in the port build).
- Integration tests run unchanged against both builds: `sql` (16),
  `planner_spec`, `vector`, `vector_join`, plus every store/WAL test (which
  do not touch the port).
- Added: `planner_spec::front_end_transcript_matches_golden` (a 330-line
  transcript of lexer, parser, DDL and checker outcomes in
  `tests/fixtures/planner/front_end.{txt,golden}`; blessed only by the Rust
  build, byte-identical in the port), `planner_spec::planner_chooses_the_golden_access_paths`
  (all 22 golden plans of `cases.txt` chosen by the Vesper planner),
  `tests/port_memory.rs` (200 cycles of every entry point, successful and
  failing, leave no live port allocation) and `tests/port_bazel.rs` (Bazel
  divergence and binding checks; no-ops under Cargo).
- The golden `planner_spec` results are byte-identical in both builds.

## Divergence checks

One deliberately wrong edit per ported Rust module (`DIVERGENT` in
`build.rs` applies all of them under `vesper-port-divergent`; Bazel builds
each one alone, `PORT_DIVERGENT` in `BUILD.bazel`). Each was verified alone
and together:

| Rust module | Vesper file | Edit | Failing test |
| --- | --- | --- | --- |
| `parse.rs` lexer | `lex.ves` | `<=` lexes as `<` | `front_end_transcript_matches_golden` |
| `lib.rs` DDL | `ddl.ves` | duplicate table not `Duplicate` | `ddl_atomic_deterministic_and_stale_revision_or_content_rejected` |
| `plan.rs` checker | `plan.ves` | LEFT JOIN right side not nullable | `outer_join_null_extension_and_filter_placement` |
| `plan.rs` optimizer | `plan.ves` | predicates cross a FULL join | `outer_join_pushdown_differential_for_many_predicates_and_null_inputs` |
| `execute.rs` | `values.ves` | three-valued AND wrong | `sql_null_three_valued_logic_exhaustive` |
| access planner | `stats.ves` | `v > c` not flipped | `planner_chooses_the_golden_access_paths` |

Unchanged tests (`real_where_aliases_literals_and_typed_bindings`,
`golden_results_equal_filtered_index_lookups`) still pass on the divergent
builds. Cargo runs the check in `scripts/test-port-harness.sh` (macOS,
Windows); Bazel runs `//crates/vesper-quarry:port_divergence_detected`
(Linux), which also checks that `port/binding.rs` matches the generated
binding.

## Leaks

The first port build leaked: native code does not free function-local
`ChunkSeq` values yet. Every such local now lives in scratch fields of `Q`
(`aux`..`aux4`, `bits`) that the handle's drop frees. `tests/port_memory.rs`
guards it. The compiler-side gap (locals not dropped) is worth an issue for
the native backend.

## Benchmark

`examples/planner_abba.rs`: parse + check + optimize of the 22 golden cases
of `tests/fixtures/planner/cases.txt`, 200 passes per window, 9 ABBA epochs,
two release binaries (Rust arm A, port arm B), paired with
`vesper_bench::analyze_pairs`. Setup: cloud x86-64 container, 6 build jobs,
release profile, port compiled by a release `vesper` (Cranelift).

| Run | Rust (A) | Port (B) | Median regression |
| --- | --- | --- | --- |
| port as built (Cranelift default opt level) | 5.0-8.1 µs/query | 24-37 µs/query | +32608 to +39298 bp (4.3x-4.9x) |
| after main's inliner and bounds hoisting (main at 2026-11-17 23:26 UTC) | 4.6 µs/query | 14.5 µs/query | +21612 bp (3.2x) |
| experiment: compiler patched to `opt_level=speed` (not committed) | 5.7 µs/query | 25 µs/query | +31942 bp (4.2x) |

2x gate: **Regression**. Per the lane rule, the Rust code is not deleted.
A callgrind profile is flat: the top costs are `malloc`/`free`/`realloc`
from sequence growth and handle setup (~25%), per-byte sequence accessors
(`try_get` with an `Option` match, no inlining across Vesper functions), and
the encode/decode round trip in `src/port.rs` (~15-20%). Cranelift's own
optimizer gains only ~10%. What would close the gap is compiler work:
inlining small Vesper functions, cheaper `ChunkSeq` element access, and
reserve/with-capacity for sequences.

## Platforms run

- Linux x86-64 (cloud): both builds, all crate tests; divergence checks;
  `cargo +1.100.0 clippy --all-targets -p vesper-quarry -- -D warnings` with
  and without `vesper-port`; `vesper-flux` tests with
  `--features vesper-quarry/vesper-port`.
- Linux (Bazel), macOS AArch64 and Windows x86-64 MSVC: through CI on the PR
  (Bazel targets in `crates/vesper-quarry/BUILD.bazel`; Cargo lines in
  `scripts/test-port-harness.sh`).

## Was the Rust code deleted?

No. The port is over 2x slower (stop rule), and the user's rule keeps the
Vesper build non-default until performance parity. The Rust front end,
optimizer and executor are frozen (bug fixes only; README line), and
`vesper-port` stays an optional feature.
