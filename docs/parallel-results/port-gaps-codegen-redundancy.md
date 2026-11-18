# Port gaps lane 7: codegen redundancy (#990)

Claim #990, PR #1006, base `07e7d4e` (main with #983). The native optimize
path now builds variants in place, does one compare per fused read, and
hoists sequence headers out of loops. The unoptimized path, the verified
backend, `src/ir.rs`, `ir_opt::optimize`, `lean/**`, `bootstrap/**` and
`packages/vesper-compiler/**` are unchanged.

**Result.** All four repros got smaller (Ignis `check` 103 to 45
instructions, BBR `outer_new` 51 to 15, warp-wheel `sum3` 34 to 25, and
Quarry `lower_ok` no longer loads 3 header words or compares twice per
byte). On the committed benchmarks, the seven ports run 0% to 4.8% fewer
instructions. None reaches parity from this lane alone. Rust stays the
default build of every port, and no port source changed.

## What changed

1. **Aggregates built where they end up** (`src/native_aggregates.rs`, and
   the `data_into`, `call_function_into`, `Return` and `Exit` hunks of
   `src/native.rs`).
   - `plan()` looks for each constructor (`Op::Data`, `Op::Tuple`) or
     direct-call result that is used exactly once, later in its own block.
     If that use is the graph's `return` (or its own `Exit`), the value gets
     `Home::Result`. If the use is a field of a later constructor, it gets
     `Home::Field`.
   - The value is then built in the result buffer, or inside the parent's
     storage. That storage is allocated uninitialized when the parent has
     no home of its own. Its consumer skips the copy.
   - A call with a home passes the home as its output buffer. A
     register-result call stores its words there, and an inlined effect
     specialization copies its result there once.
   - Constant aggregates become constant stores into the result, so no
     separate folding pass is needed.
   - Graphs that mention `Box` keep the old lowering, because FIP reuse
     slots and Box drop elaboration track their slots.
   - Why this is sound: the value has one use, in the same block. The
     result buffer is the caller's fresh output, or the function's own
     register-result buffer. A field home belongs to a constructor that does
     not exist yet. So nothing in between can read or alias the home.
   - Call outputs of plain-data types are no longer zeroed, because the
     callee writes every word. Plain data here means no sequence, string,
     box, handle, function or `f32`; an `f32` store writes only half its
     word.
2. **One compare per fused read** (fused hunks of `src/seq/native.rs`, and
   the `Match` terminator of `src/native.rs`). When a `match` on a fused
   `try_get` already covers both outcomes of the bounds check (`Some` and
   `None`, or a `_`), its last such arm jumps straight to its block, with no
   second compare and no `ud2`. `try_get` reads in a `bounds_hoist` fast
   copy (`SeqOp::Hoisted`) are now fused too, with constant validity, so
   they no longer build an `Option` in memory.
3. **Sequence headers hoisted out of loops** (`bounds_hoist::header_hoists`
   in `src/bounds_hoist.rs`, plus `seq_hoist_header` and `seq_word` in
   `src/seq/native.rs`).
   - Which loops: single-entry natural loops with no closure, handler,
     indirect call or effect operation.
   - Which receivers: a `ChunkSeq`/`Slice` place qualifies when no
     instruction in the loop writes an overlapping place, rebinds or moves
     it, resizes it, or lends it other than `in`. Writing a disjoint sibling
     field, as `lower_ok`'s `q.seen` is, still qualifies. Every use of the
     read must be the receiver of an element, length or view access.
   - What happens: the handle and `{len, data}` (for a slice, also the view
     words) are read once at the end of the preheader. The loop's accesses
     reuse them through `ExitTarget.header_words`.
   - Bounds are still checked against the hoisted length, so no guard is
     needed (#942's versioning guard still applies where it fires).
   - A loop that calls, or allocates through a sequence operation, hoists
     one receiver at most. With no cap, Ignis ran 4.1% *more* instructions,
     because its pass loop spilled six hoisted words around calls and
     reloaded them, which cost more than reading the headers.
   - Unreachable blocks (after a `return` or `break`) are ignored when
     finding a loop's predecessors.

### Coordination with #972 and its follow-up

#983 already zeroes only the uncovered words of a constructor, and builds
struct literals without per-field temporaries. This lane builds on that.
#1001 (the #972 follow-up) zeroes `f32` words in `data()`. Its fix applies
to the in-place path too, because the in-place path goes through the same
`data_into`. Plain-data call outputs exclude `f32` for the same reason.

## Repros

Built with `vesper build <file> --lib`, then `objdump -d`, counting
instructions in the emitted function.

| Repro | main `07e7d4e` | this branch | What is left |
|---|---:|---:|---|
| Ignis `check` (gap 1) | 103 (frame 0xc0, 10 zero stores) | 45 (no frame slots) | Each `Bad*` arm still zeroes the 3 unused payload words |
| BBR `outer_new` (gap 2) | 51 | 15 | 9 constant stores, 1 status register, 5 frame instructions |
| warp-wheel `sum3` (gap 3) | 34 (4 compares, 2 `ud2`) | 25 (2 compares, no `ud2`) | The `in s` parameter is still spilled to a slot (`movq $0,(%rsp)`) |
| Quarry `lower_ok` (gap 6) | 39; per byte: 3 header loads, 2 compares | 38; per byte: 0 header loads, 1 compare | The 3 loads now run once, before the loop |

## Ports

Setup:
- Machine: cloud x86-64 container, 4 vCPU Intel Xeon, Linux 6.18, Rust 1.97,
  `--release`.
- Builds: each port was built by `VESPER_PORT_COMPILER`, using either main
  `07e7d4e`'s release `vesper` or this branch's. Both used the same port
  sources and `optimize = true` (the harness default).
- Instructions: callgrind `Ir`, whole process, with the commands of each
  port's parity report:
  - BBR: `ack_path_abba measure 20000`
  - Quarry: `planner_abba window 20`
  - Prism: `abba measure 20`
  - Ignis: `abba measure 20`
  - Aura: `abba measure 40`
  - warp-tcp: `abba worker codec` with 64 requests, and `abba worker
    transfer` with 4 requests
- The Rust arm is the same example built without `vesper-port`. Aura has no
  Rust arm, because its Rust build lives only in history (#979).
- warp-wheel was measured by wall clock: `tests/abba.rs --ignored` (30
  epochs, both builds in one process). Each compiler was run once, back to
  back.

| Port | Rust Ir | Vesper Ir, main | Vesper Ir, branch | Change | Ratio, main | Ratio, branch |
|---|---:|---:|---:|---:|---:|---:|
| BBR | 21.97 M | 51.73 M | 51.35 M | -0.7% | 2.35x | 2.34x |
| Quarry | 19.47 M | 32.94 M | 32.62 M | -1.0% | 1.69x | 1.68x |
| Prism | 53.31 M | 84.19 M | 81.21 M | -3.5% | 1.58x | 1.52x |
| Ignis | 116.13 M | 175.75 M | 173.23 M | -1.4% | 1.51x | 1.49x |
| Aura | n/a | 338.11 M | 328.13 M | -3.0% | n/a | n/a |
| warp-tcp codec | 33.94 M | 24.90 M | 24.30 M | -2.4% | 0.73x | 0.72x |
| warp-tcp transfer | 8.33 M | 12.75 M | 12.33 M | -3.3% | 1.53x | 1.48x |
| warp-wheel (wall clock) | | | | | 1.832x [1.817, 1.847] | 1.828x [1.806, 1.839] |

The warp-wheel intervals overlap, so its change is within noise.

Hoisting variants, measured in instructions:

| Port | No hoist cap | Leaf loops only | One receiver in loops that call (shipped) |
|---|---:|---:|---:|
| Prism | 80.91 M | 82.86 M | 81.21 M |
| Ignis | 183.03 M | 173.27 M | 173.23 M |
| Aura | 325.62 M | 329.56 M | 328.13 M |
| Quarry | 32.63 M | 32.74 M | 32.62 M |

The shipped variant keeps most of Prism's and Aura's gain without Ignis's
regression.

The ports gain far less than the repros because their hot loops were already
rewritten by hand, in the parity wave, around exactly these costs:
- Quarry's identifier loop was inlined by hand.
- BBR caches its empty progress record.
- Ignis builds its blocks by copy.

A lane 9 re-score with those workarounds undone would show more. This lane
does not change port sources.

## Tests

- `tests/native_aggregates.rs`:
  - variants returned from each arm;
  - nested constant aggregates;
  - call results built in fields, in tuples and in variant payloads;
  - `swap` through `in`;
  - early `return`s of constructed variants from a loop.

  Recursive helpers stay calls, so the result-buffer paths also run in the
  JIT. Each program runs in the evaluator, the JIT with the optimizer on and
  off, x86-64 and AArch64 objects, and an executable.
- `tests/native_hoist_headers.rs`:
  - Hoisting plans: `q.pool` is hoisted past a sibling write; a slice
    parameter is hoisted; nested loops hoist both receivers to the outer
    preheader; a `try_set` loop is hoisted.
  - Loops that push, lend `inout q` to a call, or rebind the sequence are
    not hoisted.
  - Every case also runs the same differential checks.
- The existing native suites still pass: `native_seq_access`,
  `native_seq_drop`, `native_bytes`, `native_bytes_builtins`,
  `native_fail_status` and `native_inline`.

## Stage 1 guard

No file in the guarded set changed. The bootstrap-stage1 workflow on this
branch (run 44260320431, commit `e0d31ff`) succeeded, and so did the PR's
"Stage 1 reproducibility and cross-backend ratchet" check.

## What is left

- Unused payload words of a variant are still zeroed (3 stores per `Bad*`
  arm of `check`). Dropping them needs a rule that nothing reads inactive
  payload words: equality, hashing, the export ABI and drop flags.
- Values that reach their consumer through a block parameter get no home.
  This covers a callee inlined at IR level whose `return` becomes a jump to
  a join block, and an `if` that picks between two constructors. Following
  single-use block parameters would extend `plan()` to them.
- `Write` destinations (`q.f = T { .. }`) are still built in a temporary
  and copied. Building them in place needs proof that no operand reads the
  destination after the first in-place store.
- `in` aggregate parameters are still spilled to a slot at entry (`sum3`).
- A loop that calls hoists at most one receiver. Hoisting into callee-saved
  registers, or placing the hoisted words by register pressure, would allow
  more.
