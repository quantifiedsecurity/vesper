# Port wave P1 lane 6: Strata (vesper-alloc) region bookkeeping in Vesper

Claim: #903. Prompt: `docs/agent-prompts/port-p1/06-strata-alloc.md`. Base:
`main` at 69c79b9 (2026-11-18). Recipe: `docs/maintenance/porting-a-crate.md`.

## Result

- The bookkeeping of a Strata `Region` is Vesper source:
  `crates/vesper-alloc/vesper/src/lib.ves` (Loom package `@vesper/alloc`,
  library `alloc`, export ABI v2). Everything a region decides is there:
  chunk fit and alignment, spare-chunk reuse, the growth policy and the
  reservation bound, the allocation table with never-reused serials, handle
  validation (stale, moved, wrong kind), tail reclamation across chunk
  boundaries, watermarks with LIFO rewind, commit, reset with partial chunk
  retention, close and drop, the linear-obligation count, and the element
  metadata of region vectors and frozen slices (length, capacity, amortized
  and exact growth with the exact fallback, growth in place at the tip,
  relocation, the watermark protection rule, pop, truncate, drain, release
  and freeze).
- Seed-copy rule, as Beacon (#899): `crates/vesper-alloc` is the port-backed
  package (its `build.rs` always links the port and sets `cfg(vesper_port)`),
  and `crates/vesper-alloc/seed` is the frozen Rust seed copy
  `vesper-alloc-seed` (library `vesper_alloc`). The compiler (root
  `[workspace.dependencies]`), `vesper-kinetic` and `vesper-crucible` depend
  on the seed by rename. All three are inside the compiler's closure, so no
  crate links the port build yet except the crate's own tests and example.
- **Rust stays the default and is not deleted** (the owner's parity rule,
  2026-11-17): the port is 9-11x slower on the region hot paths (below).
- Every test of the crate passes against both packages:
  `cargo test -p vesper-alloc -p vesper-alloc-seed` (262 tests each: 225
  unit and integration tests and 37 doc tests, including the `compile_fail`
  ones). `tests/implementation.rs` asserts which build
  each package links.

## API inventory (re-measured at 69c79b9)

Public API of the crate (`src/lib.rs` re-exports), by module:

| Module | Public items | This lane |
|---|---|---|
| `region.rs` | `Region` (`new`, `scoped`, `with_backing`, `scoped_in`, `id`, `config`, `backing`, `stats`, `alloc_bytes`, `alloc_bytes_copy`, `len`, `bytes`, `bytes_mut`, `bytes_mut_pair`, `extend_bytes`, `release_bytes`, `bytes_ptr`, `alloc_raw`, `alloc_value`, `alloc_linear`, `get`, `get_mut`, `box_ref`, `ref_get`, `ref_get_mut`, `ref_get_disjoint_mut`, `take`, `checkpoint`, `rewind`, `commit`, `reset`, `close`), `RegionConfig`, `RegionStats`, `RegionId`, `RegionBytes`, `RegionBox`, `RegionRef`, `RefPairError`, `Watermark`, `CHUNK_ALIGN` | **ported**: every method forwards to the book (`src/region/port.rs`) |
| `region/vec.rs` | `RegionVec`, `RegionSlice`; `Region::vec_*` (new, with_capacity, with_capacity_linear, len, is_empty, capacity, as_slice, as_mut_slice, get, get_mut, reserve, reserve_exact, push, extend_from_slice, pop, truncate, clear, drain, release, freeze), `slice_len`, `slice`, `slice_release` | **ported** (`src/region/vec/port.rs`) |
| `region/string.rs` | `RegionString`, `RegionStr`, `Region::string_*` | built on the vector API, so it runs on the port unchanged |
| `region/arena.rs` | `RegionSlotArena`, `ArenaKey`, `ArenaId`, `ArenaError`, `ArenaInsertError`, `HARD_MAX_ARENA_ENTRY_BYTES` | built on region vectors: runs on the port; its own key/generation logic stays Rust (gap 1) |
| `region/flat.rs` | `RegionFlatTable`, `FlatConfig`, `FlatError`, `FlatInsertError`, `FlatIter`, `FLAT_GROUP_WIDTH`, `HARD_MAX_FLAT_GROUP_BYTES`, `HARD_MAX_FLAT_GROUPS` | built on region vectors: runs on the port; probing stays Rust (gap 1) |
| `region/tiled.rs` | `RegionTiled2`, `TiledError`, `HARD_MAX_TILE_BYTES` | built on region vectors: runs on the port; tile layout stays Rust (gap 1) |
| `error.rs` | `ConfigError`, `AllocError`, `HandleError`, `RegionError`, `ValueAllocError`, `InsertError`, `OwnerError`, `StringError`, `RewindError` | shared types; the book reports one `Fault` enum that the shim maps to them |
| `cache_btree.rs` | `CacheBTree<K, V, O>`, `CacheBTreeConfig`, `CacheBTreeError`, `CacheBTreeInsertError`, `CacheBTreeStats`, `CacheBTreeIter`, `KeyOrder`, `Canonical`, `ReverseOrder`, `CACHE_BTREE_B` | stays Rust (gap 2) |
| `tiled.rs` | `Tiled<T, N>`, `TiledConfig`, `TiledSeqError`, `TiledPushError`, `TileRef`, `TileMut`, `Col`, traits `HotColumns`, `TiledRecord` | stays Rust (gap 3) |
| `capability.rs` | trait `Alloc`, `RawBlock` | stays Rust: a trait over the slabs, which stay Rust |
| `backing.rs` | trait `ChunkBacking`, `SystemBacking` | stays Rust (OS) |
| `virtual_arena.rs`, `virtual_arena/os.rs` | `VirtualArena`, `VirtualVec`, `VirtualBytes`, `VirtualChunkBacking`, ... | stays Rust (OS: mmap, VirtualAlloc, 2 MiB hints) |
| `slab.rs`, `slab/sharded.rs` | `ShardSlab`, `SlabBlock`, `ShardedSlab`, `Shard`, `ShardBlock`, ... | stays Rust (needs F12 shared-memory concurrency) |

In-crate `#[cfg(test)]` unit tests (4): `slab::tests::identities_and_generations_exhaust_without_wrapping_or_changing_owners`,
`region::arena::tests::{local_identity_and_generation_successors_exhaust_without_wrapping, exhausted_local_arena_preserves_input_and_keeps_existing_values_usable, private_malformed_slot_and_generation_are_checked_without_public_forge_api}`.
They test Rust code that this lane does not port (the slab and the arena's
key logic), so they stay where they are and run in both packages unchanged.

## How the port is wired

The book owns no memory that Rust code reads. The Rust shim owns three
things Vesper cannot have: chunk memory from the `ChunkBacking`, the Rust
payloads (it writes, reads and moves them at the addresses the book
returns), and their destructors (the book stores the address of the
monomorphized `drop_value::<T>` / `drop_elems::<T>` and hands it back).

- **Needing a chunk.** An allocating call that needs a new chunk changes
  nothing and answers `Need { cap, align }`. The shim obtains that chunk from
  the backing and repeats the same call with the chunk; the book is unchanged
  in between, so the repeated call takes the same path and adopts it. A
  backing refusal becomes `OutOfMemory`; for amortized vector growth it
  retries the exact requirement (`retry`/`fallback` in `Grown::Need`), as the
  Rust build does.
- **Queued work.** Destructor calls and chunk releases come back as
  `Action`s, taken in order with `book_take_pending`. The shim removes each
  action before it runs, so a panicking destructor cannot run twice, and
  `Drop` finishes the rest. All bookkeeping is updated before any
  destructor runs.
- **Storage.** Chunks and slots are flat `ChunkSeq[u64]` tables (4 and 13
  words per entry), so the hot paths read only the words they need. The
  first version stored `ChunkSeq[Slot]` of 16-word structs and was 15-18x
  slower than Rust; the flat tables and fast paths for vector growth and
  rewind brought that to 9-11x.
- **Addresses.** Chunk bases are exposed (`expose_provenance`) when they are
  obtained, and the shim turns the book's addresses back into pointers with
  `with_exposed_provenance_mut`.

## What stays Rust, and why

| File | Reason | Unblocked by |
|---|---|---|
| `backing.rs`, `virtual_arena.rs`, `virtual_arena/os.rs` | OS memory (mmap, VirtualAlloc, MADV_HUGEPAGE) | stays Rust by the lane prompt |
| `slab.rs`, `slab/sharded.rs` | lock-free remote queue, per-core shards | F12 (shared-memory concurrency) |
| `capability.rs` | the `Alloc` trait is implemented by the slabs | follows the slabs |
| `region/port.rs`, `region/vec/port.rs` (new) | the shim: chunk memory, Rust payloads and destructors | source-level regions (#588) and `dyn`/drop glue (F7b) |
| `cache_btree.rs` (gap 2) | `CacheBTree<K, V, O: KeyOrder<K>>` orders keys through a Rust trait and returns `&V` borrows; the export ABI has no callbacks and no generic user types, so the tree cannot call the order and the shim would need a token slab for keys and values. `@vesper-std/collections::btree::CacheBTree` (#897) already replays the #847 traces byte for byte (`tests/vesper_std.rs`) | callbacks or `dyn Trait` in the export ABI (F7b), or a key-encoding instance list |
| `tiled.rs` (gap 3) | `Tiled<T, N>` is const-generic over Rust record types (`TiledRecord`, `HotColumns`) and hands out borrowed column views; its decisions are index arithmetic that the ABI round trip would cost more than | generic user types in the export ABI |
| `region/{arena,flat,tiled,string}.rs` own logic (gap 1) | already layered on the ported vector API, so their storage runs on the port; their own decisions (slot-arena generations, flat-table probing, tile layout, UTF-8 checks) are still Rust | follow-up lanes, one per collection |
| `error.rs` | shared types of both builds (the recipe keeps public types in the crate) | - |

`unsafe` counts: the Rust build has 23 `unsafe` in `region.rs` (19 blocks)
and 16 in `region/vec.rs` (13 blocks). The port shim has 20 in
`region/port.rs` (17 blocks) and 12 in `region/vec/port.rs` (11 blocks): the
payload reads and writes stay, and the shim adds the two `transmute`s that
turn a destructor address back into a function pointer. The Vesper source
has none. The prompt's goal that `unsafe` in ported modules disappears is
not reachable while the payloads are Rust values.

## Divergence checks

`build.rs` applies three edits under the `vesper-port-divergent` feature
(Bazel builds the same edits as `:alloc_vesper_divergent`), and
`scripts/test-port-harness.sh` asserts that each is caught by its test and
that an unrelated test still passes:

| Area | Edit | Caught by | Still passes |
|---|---|---|---|
| placement (`region.rs`) | spare chunks kept by a rewind are never reused | `region::rewind_reuses_bytes_and_keeps_spare_chunks` | `region::size_arithmetic_is_checked` |
| watermarks (`region.rs`) | a rewound watermark stays outstanding | `region::rewinding_an_outer_watermark_invalidates_inner_ones` | `region::size_arithmetic_is_checked` |
| vectors (`vec.rs`) | a linear vector of one element is no live obligation | `collections::linear_elements_block_discard_until_drained` | `collections::capacity_arithmetic_is_checked` |

While choosing the edits, two plausible bugs were not caught by any test:
clamping a new chunk to the remaining reservation (`capacity = max(growth,
size)` instead of `max(min(growth, remaining), size)`), and the minimum
amortized vector capacity (8 for bytes, 4 otherwise). Both are coverage gaps
of the existing Rust tests, not of the port.

## Benchmarks

`examples/region_abba.rs`: two release binaries (Rust seed arm A, port arm B),
9 ABBA epochs per workload, paired with `vesper_bench::analyze_pairs` against
the plan's 2x stop rule (+10000 bp). Setup: cloud x86-64 container, 4 vCPUs,
release profile, port compiled by the in-tree `vesper` (Cranelift, default
optimization with the Stage 1 inliner).

| Workload | Rust (A) | Port (B) | B/A | 2x gate |
|---|---|---|---|---|
| `vec_push`: 4096 pushes into a fresh vector, read back, rewind | 5.2 ns/op | 47.2 ns/op | 9.0x | Regression (+81088 bp) |
| `values`: 1024 `alloc_value` + `get`, rewind | 14.5 ns/op | 131.7 ns/op | 9.1x | Regression (+81374 bp) |
| `bytes`: 1024 `alloc_bytes_copy` (24 B) + `bytes`, rewind | 12.1 ns/op | 137.9 ns/op | 11.4x | Regression (+106414 bp) |
| `btree`: `CacheBTree<u64, u64>` insert, lookup, range (Rust in both arms: noise control) | 113.6 ns/op | 118.0 ns/op | 1.04x | Pass (+318 bp) |

The port fails the 2x stop rule, so nothing is deleted and every dependent
stays on the seed. Callgrind on `values` puts most of the remaining cost in
the book itself (about 230 instructions per `book_alloc` and 200 per
`book_view` for a handful of table reads): every table access is a checked
`try_get` with an `Option` match, results are multi-word enums written
through the result pointer, and each operation is one call across the C ABI
that nothing inlines into the Rust caller. What would close the gap, in the
order of the expected gain: an export path that lets the Rust caller inline
small Vesper functions (or a Vesper fast path for the bump case that returns
a scalar), unchecked indexing where a bound is already proven
(`src/bounds_hoist.rs` covers loops, not these point reads), and returning
small results in registers rather than through memory (as #946 did for
`Fail`).

Counting-allocator steady state: `tests/zero_alloc.rs` passes against the
port. Its region case (`region_ref_access_does_not_allocate`) reads only;
the book's tables grow through the Vesper runtime's allocator, which the
Rust counting allocator does not see, and they keep their capacity across
rewind and reset, so a warmed-up region does not grow them.

## Platforms run

- Linux x86-64 (cloud container): `cargo test -p vesper-alloc -p vesper-alloc-seed`,
  the three divergence checks, `cargo +1.100.0 clippy --all-targets` on both
  packages with `-D warnings`, `cargo fmt --check`, `vesper fmt --check`,
  the closure guard (`vesper-port-harness` `closure_guard`), and
  `tools/bazel/sync_build_files.py --check`.
- Bazel (Linux), macOS and Windows: through CI on the pull request.

## Root patch

Own commit: `crates/vesper-alloc/seed` joins `members` and `default-members`;
the root `vesper-alloc` workspace dependency, `vesper-kinetic` and
`vesper-crucible` point at the seed by rename; `Cargo.lock` regenerated.
`tests/fixtures/self_host/driver.rs` skips `crates/vesper-alloc/vesper`
(one line, like the other ports).

## Remaining gaps

1. Port the collections' own logic (slot-arena keys and generations,
   flat-table probing, the tiled layout, string UTF-8 checks) into the book.
2. Forward `CacheBTree` once the ABI can call a key order, or for a fixed
   list of key instances through an order-preserving key encoding.
3. Forward `Tiled<T, N>` once the ABI has generic user types.
4. Reach performance parity (above) before any dependent moves to the port.
5. Lean: `lean/StrataCollections.lean` was not extended; the port keeps the
   Rust build's invariants and the existing model tests run against it.
