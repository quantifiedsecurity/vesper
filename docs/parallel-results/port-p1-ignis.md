# Port P1 lane 12: vesper-ignis render-graph planner

Claim: [#909](https://github.com/robot-head/vesper/issues/909). Baseline: main
at `b69383e` (export ABI v2, #921). Prompt:
`docs/agent-prompts/port-p1/12-ignis.md`.

## API inventory (re-measured before porting)

The crate root and its three public modules, unchanged by the port:

- **D3D12 vocabulary** (`src/d3d12.rs`): `Sync` and `Access` bit sets (their
  constants, `bits`, `from_bits`, `contains`, `intersects`, `|`, `Display`,
  plus `Sync::expanded`, `Sync::covers`, `Access::is_write`,
  `Access::incompatible_with_sync`), `Layout` (25 variants, `PRESENT`, `name`,
  `raw`, `compatible_access`, `allows`, `Display`), `QueueKind` (`name`,
  `supports_layout`, `can_transition`, `supported_access`, `supported_sync`,
  `Display`).
- **Graph** (`src/graph.rs`): `RenderGraph` (`new`, `add_queue`, `queue_kind`,
  `add_buffer`, `add_texture`, `resource_name`, `pass_name`, `pass_count`,
  `add_pass`, `add_dependency`, `plan`, `plan_with`), `QueueId`,
  `BufferHandle`, `TextureHandle`, `ResourceRef`, `PassId`, `ByteRange`,
  `SubresourceRange`, `BufferDesc`, `TextureDesc`, `PassDesc`,
  `QueueRestriction`, `GraphError`.
- **Plan** (`src/plan.rs`): `PlanOptions`, `DependencyKind`, `Dependency`,
  `BufferBarrier`, `TextureBarrier`, `Barrier`, `Command`, `QueueStream`,
  `Plan` (`barrier_count`, `render`), `PlanError`, and
  `PLAN_CONTRACT_VERSION`.
- **`multi_backend`**: `GpuApiTarget`, `NativeBackendBarrier`,
  `LoweredCommand`, `LoweredQueueStream`, `LoweredPlan` (`barrier_count`),
  `lower_to_target_barrier`, `lower_to_target_barrier_for_queue`,
  `lower_plan`, `vk_stage_bits`, `try_vk_stage_bits`, `vk_access_bits`,
  `vk_access_bits_for_sync`, `try_vk_access_bits`, `mtl4_stage_bits`.
- **`lifecycle`**: `MAX_FENCE_VALUE`, `TimelineId`, `FrameTicket`,
  `FencePoint`, `Completion`, `Limits`, `QueueConfig`, `LifecycleError`,
  `OwnedError`, `PreparedPlan`, `ReadyAllocator`, `QueueBinding`, the
  typestate markers (`Recording`, `Closed`, `Uninitialized`, `Ready`),
  `WorkGraphScratch`, `ScratchOwner`, `ScratchAction`, `ScratchLease`,
  `BatchInputs`, `CommandBatch` (`RecordingBatch`, `ClosedBatch`),
  `BatchParts`, `QueueCommand`, `SubmissionStream`, `Submission`,
  `OwnerViews`, `BackendSubmit`, `PollBatch`, the `Backend` trait,
  `SubmitError`, `SubmitResult`, `RetiredFenceReceipt`, `RetiredFrame`,
  `Runtime` (`new`, `timeline`, `observed`, `allocator`, `pending`,
  `submission`, `record`, `submit`, `complete_original`, `poll`, `drain`,
  `shutdown`), `PollError`, `PollReport`, `DrainReport`, `Shutdown`.
- **`recording`**: `SubmitMode`, `RecordingError`, `RecordingBackend`
  (`new`, `Default`, `push_submit_mode`, `push_poll_failure`, `history`, and
  its `Backend` impl).

Unit tests: two `#[cfg(test)]` tests in `src/multi_backend.rs`. Integration
tests: 38 in `tests/planner.rs`, 19 in `tests/lifecycle.rs`, 10 in
`tests/multi_backend.rs` (69 with the two unit tests), plus two
`compile_fail` doctests and the crate-level doctest.

Parts named in the prompt as staying in Rust: the backend adaptor (the
`Backend` trait and its callers).

## What was ported

`vesper/` is the Loom package `@vesper/ignis` (capability ceiling `Fail`).
The library is 5,700 lines of Vesper in eleven files under `vesper/src/`,
joined in the order `build.rs` (and `BUILD.bazel`) list: `vesper fmt` refuses
files over 8,192 tokens (and over its work budget), and the harness compiles
one source file, so the library is split at item boundaries and joined
before `vesper build --lib`. Every file is canonically formatted. The joined
library exports 54 functions.

| Rust module | Vesper files | What runs in Vesper |
| --- | --- | --- |
| `d3d12.rs` | `d3d12.ves` | Sync aggregate expansion and covering, access/sync and access/layout compatibility tables, queue layout/access/sync support, transition rules, raw layout values. |
| `graph.rs` | `graph.ves` | The graph state (`Graph` handle): queues, resources, passes, uses, explicit edges; all declaration validation (resource checks, per-use access/sync/layout/queue/range checks, pairwise conflicts), dependency edges. |
| `plan.rs` | `plan_order.ves`, `plan_state.ves`, `plan_passes.ves`, `assemble_slots.ves`, `assemble.ves` | Hazard edges, the deterministic topological order and cycle report, the per-subresource and per-byte-segment state machine (layouts, reads, coverage, split barriers, queue handoffs, final layouts), barrier slot grouping into boxes, cross-queue waits with transitive elision, fence values, and command-stream assembly. |
| `multi_backend.rs` | `lower.ves` | Vulkan stage/access tables, Metal 4 stage table, per-barrier lowering for all three targets, and the split-event pairing of `lower_plan`. |
| `lifecycle.rs` | `lifecycle.ves`, `lifecycle_completion.ves` | The fence ledger (`Lifecycle` handle): limit checks, input validation, scratch checks, translation of a plan into fenced streams with reservations, in-flight records, settle, completion validation and retirement. |
| `recording.rs` | `recording.ves` | The queue model (`Recorder` handle): submit modes, history and pending bounds, waits, signals, per-timeline ordering, budgeted polling, scripted poll failures. |

The Rust crate keeps the public types in `src/*.rs`, shared by both builds.
The logic is in `src/reference/` (the default Rust build) or `src/port/`
(`--features vesper-port`), selected in `src/lib.rs`. `src/port/ffi.rs`
holds the Vesper record types; the generated binding is `port/binding.rs`.

The lifecycle `Runtime` is now shared by both builds: it owns the payloads
and calls a `Ledger` (Rust in `src/reference/lifecycle.rs`, Vesper-backed in
`src/port/lifecycle.rs`). The Rust build was refactored the same way so that
both builds run the identical ownership code.

## What stays in Rust, and why

| File | What | Reason, and what unblocks it |
| --- | --- | --- |
| `src/lifecycle.rs` | `Runtime` payload ownership, the `Backend` trait, `poll`/`drain`/`shutdown` loops, typestate markers, scratch retirement, the runtime-id counter | The prompt keeps the backend adaptor in Rust. Generic payloads (`A`, `R`, `T`) and a Rust trait object cannot cross the export ABI; the Vesper ledger decides everything and Rust only moves owners. |
| `src/port/recording.rs` | The `Submission` history and the `Backend` impl | `history()` returns `&[Submission]`, Rust values with barrier vectors; the Vesper recorder holds the waits and signals it simulates. |
| `src/graph.rs`, `src/port/graph.rs` | Resource and pass names, `GraphError` strings, graph ids | Strings are not exportable yet (export ABI v2 has no `String`). |
| `src/d3d12.rs`, `src/plan.rs` | `Display` impls, `Layout::name`, `QueueKind::name`, `Plan::render` | Same: string results. |
| `src/port/plan.rs`, `src/port/multi_backend.rs` | Rebuilding `Plan`, `LoweredPlan` and `NativeBackendBarrier` from records | Handles carry the Rust graph id; the records carry indexes. A thin adapter, no decisions. |
| `src/port/lifecycle.rs` | Rebuilding `Submission` from the translated records; `LifecycleError::Limit(&'static str)` messages | `&'static str` payloads are not exportable. |

Panic messages (`more than 65535 queues`, `more than u32::MAX resources`,
`more than u32::MAX passes`, `recording backend fault after acceptance`) are
raised by the Rust wrapper on the Vesper outcome.

API differences under `vesper-port`: `Layout::raw` and the six
`multi_backend` table functions are `const fn` in the Rust build and plain
`fn` in the Vesper build (a call into the library cannot be `const`). No
caller in the repository uses them in a const context. `Runtime::submit`
now reserves its in-flight slot before translation (both builds), so an
allocation failure is reported before a lifecycle error; that order is not
observable without running out of memory.

## Unit tests

The two `#[cfg(test)]` tests of `src/multi_backend.rs`
(`metal4_elides_layout_only_transition_but_keeps_hazard`,
`vulkan14_maps_texture_barrier_bits`) were translated to
`tests/multi_backend_unit.rs` through the public API, unchanged otherwise.

Results (Linux x86-64, cloud): all 69 tests, the crate doctest and both
`compile_fail` doctests pass against the Rust build and against the Vesper
build. The test files are unchanged.

## Divergence checks

`--features vesper-port-divergent` applies one edit per ported module (EDITS
in `build.rs`, the same `edits` on `:ignis_vesper_divergent` in Bazel).
`tests/divergence.rs` runs the divergent test binaries and requires each
module's test to fail:

| Module | Edit | Test that catches it |
| --- | --- | --- |
| d3d12 | `NON_PIXEL_SHADING` also expands to pixel shading | `planner::sync_aggregates_expand_as_specified` |
| graph | overlapping uses with different explicit layouts are not a conflict | `planner::declaration_errors_are_structured` |
| plan | a buffer write no longer orders after the previous writer | `planner::buffer_writers_are_serialized_even_on_disjoint_ranges` |
| plan (assembly) | every planner signal is one fence value too high | `planner::cross_queue_read_after_write_uses_fence_not_barrier` |
| multi_backend | `COPY` maps to the Metal dispatch stage instead of blit | `multi_backend::metal4_emits_native_mtlstages_bitmasks` |
| lifecycle | a completion at the observed value is not stale | `lifecycle::stale_foreign_and_future_completions_return_the_report_and_keep_work` |
| recording | the first-queue-only submit mode publishes every queue | `lifecycle::partial_multi_queue_publication_cannot_retire_when_unsubmitted_terminal_never_occurs` |

Under Cargo, `port/check.sh` (one line in `scripts/test-port-harness.sh`)
runs both builds and the divergence check on macOS and Windows. On Linux,
Bazel runs `:port_planner_test`, `:port_lifecycle_test`,
`:port_multi_backend_test`, `:port_multi_backend_unit_test` and
`:port_divergence_detected`.

## Benchmarks

vesper-ignis is not on the wave's hot-path list and has no benchmark of its
own. To decide on parity, an ad-hoc timing loop (not committed) built a
graph of 8 textures (4 mips) and one buffer on a direct and a compute queue,
with alternating write/read passes, and timed 200 iterations of building
the graph, `plan()`, and `lower_plan(Vulkan14)`. `cargo test --release`,
Linux x86-64 cloud container, one run each (mean microseconds per call):

| Passes | Build | | Plan | | Lower | |
| --- | --- | --- | --- | --- | --- | --- |
| | Rust | Vesper | Rust | Vesper | Rust | Vesper |
| 64 | 14.2 | 84.5 (6.0x) | 176 | 1,898 (10.8x) | 4.1 | 24.2 (5.9x) |
| 256 | 62.1 | 588 (9.5x) | 1,942 | 25,219 (13.0x) | 13.7 | 73.9 (5.4x) |

Both builds emitted the same barrier totals. The Vesper build is 5x to 13x
slower, so it stays non-default. Likely costs: copy records cross the ABI by
value, `ChunkSeq` reads go through `try_get` with bounds checks, and the
planner state uses linear scans (`signal_value`, coverage lists) where the
Rust build uses `BTreeMap`s.

## Platforms

- Linux x86-64 (cloud): both builds, all tests, divergence check.
- macOS AArch64, Windows x86-64 MSVC, and Linux under Bazel: the required CI
  checks of the pull request.
- `.github/workflows/ignis-vulkan.yml` builds vesper-ignis-vulkan against
  the default (Rust) build; its API is unchanged.

## Rust code deleted?

No, by the user's rule for every port (2026-11-17): the Rust code stays the
default build and the Vesper build stays behind `--features vesper-port`
until the port reaches reasonable performance parity. Both builds are kept
and tested; the measurements are under Benchmarks. Deletion would also need
the port-backed Bazel model of port P1 lane 2 (#919), since a port-only
build must link a prebuilt object.
