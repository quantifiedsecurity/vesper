# Port parity lane 6: Ignis (#954)

Baseline: `main` at cee8b1a (a descendant of 69c79b9). Port-only: every change
is under `crates/vesper-ignis/**`, plus the crate's `vesper-bench`
dev-dependency in `Cargo.lock`. Rust stays the default build and no Rust code
was deleted.

## What changed

1. **A committed ABBA benchmark.** `examples/abba.rs` and `bench-abba.sh`,
   built the way Aura's is: the example is compiled once per build (the
   `vesper-port` feature swaps the engine behind one `RenderGraph`, so the two
   builds cannot share a binary), and `abba compare` runs them as separate
   processes in Rust, port, port, Rust order per epoch, then reports the
   median Vesper/Rust ratio, its exact 95% interval (`vesper_bench::analyze_pairs`)
   and the range. Workloads: `build` (the graph), `plan` (`RenderGraph::plan`)
   and `lower` (`lower_plan(Vulkan14)`), at 64 and 256 passes, over the P1
   report's graph: 8 textures with 4 mips and a 64 KiB buffer on a direct and
   a compute queue, alternating write/read passes that all read the buffer,
   every fourth pass on the compute queue and every sixteenth writing the
   buffer. `abba digest` hashes each graph's rendered plan, dependencies and
   lowering; the two builds printed the same digest after every step below.
   `abba profile <workload> <passes> <ops>` runs one workload for callgrind.
2. **Hazard edges indexed per resource** (`vesper/src/plan_order.ves`). The
   planner compared every earlier pass with every later one, use by use, then
   merge-sorted all edges twice (once as edges, once per direction in
   `pairs_by`). It now builds a per-resource index of uses in pass order and,
   for each pass, walks the later uses of each of its resources, merging those
   walks by pass. Edges come out sorted by `(before, after)`, so only one pass
   pair's hazards need sorting (an insertion sort of a few elements), the
   explicit edges are merged in, `pairs_by` by `before` is a dedup and by
   `after` a stable counting sort. A buffer read has hazards only with later
   writes (two reads of bytes never order), so its walk jumps to the next
   write. The edge set and its order are unchanged.
3. **Export ABI v3** (`// @export abi 3` in `d3d12.ves`, the first joined
   file). The copy-struct slices of `graph_add_pass`, `lower_plan`,
   `lifecycle_submit`, `lifecycle_validate`, `scratch_check` and
   `recorder_submit`, and the `Limits`/`BarrierRec` arguments, are lent in
   place as `repr::*` mirrors; the Rust glue builds `Vec<repr::T>` directly.
4. **Block copies.** `sort_seq` fills its buffers with `extend` and copies
   each merge round back with one `try_copy_from` instead of element loops;
   `graph_add_pass` appends a pass's uses with `extend`.

Scratch in handle fields (lever 4 of the prompt): none to move. Ignis already
keeps its scratch sequences in locals (the #945 report saw its allocator work
fall by 18% for that reason). `fill` for barrier and slot tables (lever 5):
the tables are built by `push` loops that run once per plan; they do not show
in the profiles, so they were left alone.

## Measurements

Setup: cloud x86-64 container (Intel Xeon @ 2.30GHz, 4 cores, Linux), Rust
1.97.0 release profile for both builds, ports compiled by this tree's
`vesper` with `optimize = true` (the port harness's default). Each step is
one ABBA run, 11 epochs of 100 operations (64 passes) or 25 (256 passes) per
process, same machine; the baseline and the final state were re-run with 21
epochs. Ratio = Vesper time / Rust time, median with its 95% interval.

| Step | build 64 | build 256 | plan 64 | plan 256 | lower 64 | lower 256 |
| --- | --- | --- | --- | --- | --- | --- |
| `main` (21 epochs) | 4.30x (4.08-4.48) | 4.43x (4.21-4.53) | 5.21x (5.04-5.44) | 4.53x (4.44-4.72) | 4.42x (4.14-4.69) | 3.73x (3.50-3.88) |
| hazard index | 4.29x (4.15-4.56) | 4.41x (4.16-4.78) | 3.72x (3.60-3.89) | 2.33x (2.21-2.43) | 4.10x (3.71-4.24) | 3.42x (3.34-3.63) |
| + buffer reads jump to writes | 4.15x (4.06-4.58) | 4.30x (4.20-4.47) | 2.49x (2.46-2.68) | 1.15x (1.13-1.22) | 4.31x (4.04-4.41) | 3.54x (3.25-3.71) |
| + ABI v3 | 4.06x (3.96-4.45) | 4.23x (4.15-4.90) | 2.53x (2.48-2.74) | 1.16x (1.15-1.25) | 4.29x (3.89-4.34) | 3.51x (3.30-3.58) |
| + block copies (final, 21 epochs) | 3.74x (3.64-3.91) | 3.93x (3.85-4.02) | 2.41x (2.37-2.47) | 1.12x (1.10-1.13) | 4.19x (4.03-4.30) | 3.51x (3.47-3.73) |

Absolute times at the end (median per operation): Rust plan 73 µs / 1.23 ms
and Vesper 176 µs / 1.37 ms at 64/256 passes; build 5.7/21.5 µs against
21.2/83.9 µs; lower 3.0/16.5 µs against 12.5/59.9 µs.

ABI v3 changed nothing measurable: no call here converts a large struct, so
the v2 conversion was never the cost (same finding as Quarry in the v3
report). It is kept because it is what the plan asks for and it removes a
heap copy per call.

### Why the ratio grew with the pass count

The #945 numbers (plan 6.9x at 64 passes, 8.2x at 256) grew with the pass
count. On this graph `main` does not grow (5.2x, then 4.5x), but plan is
quadratic in both builds: both compare every pair of passes. The Vesper
build paid much more per pair. Callgrind of `plan` on `main` (port build with
named symbols): `hazard_edges` 34% of instructions, the two merge sorts of
the edge lists 37% (`sort_seq[Edge2]` 21%, `sort_seq[Edge]` 16%), their
derived `cmp` calls 9%. The Rust build inserts into a `BTreeSet` as it goes.
The sorts are O(E log E) over a set that grows with the square of the pass
count, which is where the growth came from. It was the port's data
structure, so the port fixed it.

The fix gives the port a better algorithm than the Rust build, which still
compares all pass pairs. The plan 256 result (1.12x) is therefore not
compiler parity: the Rust build would gain the same way from the same index.
The plan 64 ratio (2.41x) is closer to the true per-operation gap.

## What is left (profile of the final state)

Callgrind, final port build with named symbols.

- **plan, 64 passes** (111 M instructions over 40 plans, Rust 48 M):
  `hazard_edges` 23%, `assemble` 9%, `topological_order` 7%, `pairs_by` 7%,
  `process` 5%, the export glue `vesper_ignis_graph_plan` 5%, malloc/free
  about 9%. The remaining planner work is the same algorithm as Rust's,
  written with `try_get`/`try_set` and copy-struct temporaries.
- **build, 256 passes**: `validate_use` 27%, `graph_add_pass` 11%,
  `check_pairs` 7%, malloc/free 18% (the Rust glue's `Vec<repr::UseDesc>` and
  pass names, Vesper's per-pass `uses`).
- **lower, 256 passes** (includes the setup plan): `vesper_ignis_lower_plan`
  28% of the run, `memcpy` 9%, the Rust glue's result decoding 6%.

None of these is a port data structure any more. The heaviest functions
(`validate_use`, `graph_add_pass`, `lower_plan`) are long straight-line code
that builds many copy enums and structs, and their cost is in the code the
compiler emits for that, below.

## Compiler gaps

1. **Aggregate temporaries are zero-filled before they are written, often
   twice, and every aggregate gets its own stack slot.**
   `vesper_ignis_lower_plan` has a 6,016-byte frame and 749 `movq $0x0`
   stores, `vf179_validate_use` a 4,432-byte frame with 70 zero stores in its
   prologue alone (each of 35 slots stored twice), `graph_add_pass` 1,184
   bytes and 165, `lower_barrier` 864 bytes and 144. Each constructed variant
   is built in one slot, copied to a second and then to the result. These
   functions are 27%, 11% and 28% of the build and lower runs; about 630
   instructions per `validate_use` call do a dozen bit tests. Minimal
   repro (`vesper build --lib zero.ves --lib-name zero`): 148 instructions
   and 32 zero stores for four returns of one copy enum.

   ~~~vesper
   copy struct Rec {
       a: u64,
       b: u64,
       c: u32,
       d: bool
   }

   copy enum Check {
       Ok(Rec),
       BadA(u64),
       BadB(u64),
       BadC(u32)
   }

   pub fn check(a: u64, b: u64, c: u32) -> Check {
       if a == 0 {
           return Check::BadA(a);
       }
       if b == 0 {
           return Check::BadB(b);
       }
       if c == 0 {
           return Check::BadC(c);
       }
       Check::Ok(Rec {
           a: a,
           b: b,
           c: c,
           d: true
       })
   }
   ~~~

   The emitted code zeroes the `Rec` slot and the `Check` slot of every
   arm, writes the fields, copies `Rec` into the `Check`
   payload, and spills the scalar parameters to the stack before testing
   them. Wanted: no zero-fill of a slot whose every word is written before it
   is read, a variant built in the result slot directly, and parameters kept
   in registers.
2. **Derived `Ord::cmp` is not inlined into generic callers.** On `main`,
   `impl_Ord_Edge2_cmp` and `impl_Ord_Edge_cmp` were 9% of `plan`, called
   from `sort_seq`. The port no longer sorts edges, but `sort_seq[SlotItem]`
   (2-5% of plan before the block copies) and `sort_seq[TexEntry]` still
   call `cmp` out of line. Repro: any `sort_seq[T]` over a `copy struct` with
   `impl Ord for T {}` (`vesper/src/plan_order.ves`).
3. **`ChunkSeq::copy_from` must be the `try_` form inside an `inout` loan.**
   Copying between two equal-length sequences inside `sort_seq` (which holds
   `inout s`) needs `try_copy_from`, whose `bool` the code has to discard. A
   form that proves the lengths would read better; minor, not a cost.

## Parity

Not met. Every port test passes with and without `--features vesper-port`
and the divergence check passes (locally on Linux; the four required CI
platforms run on the PR). The ABBA medians are 3.7-4.2x for build and lower
and 2.4x for plan at 64 passes. Plan at 256 passes (1.12x, interval
1.10-1.13x) is inside the 1.05x bar only by way of a better algorithm than
the Rust build's, so it does not count. Rust stays the default build.
