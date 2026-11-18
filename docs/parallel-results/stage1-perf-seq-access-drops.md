# Stage 1 perf lane 2: sequence access and drops (#932)

Base: `main` at 943d901 (merged forward to f4bf424 before the PR). Branch
`claude/project-thread-q7e388`. Plan: `docs/agent-prompts/stage1-perf/02-seq-access-drops.md`.

## What changed

**Drops of function-local and by-value sequences and strings** (`src/fip.rs`
`seq_drop_plan`, the drop hooks in `src/native.rs`, `src/seq/native.rs`).
Native code now frees the `ChunkSeq[T]` and `String` values a function owns:
its `let` bindings and its by-value (`sink`) parameters. The slot is its own
drop flag:

- a slot starts empty (every native local is zeroed at entry);
- a `sink` read of a sequence slot (a move: returned, passed by value, stored
  into an aggregate, bound elsewhere) empties it;
- binding or assigning the slot again frees what it held, so a `let` inside
  a loop frees the previous iteration's sequence;
- every exit frees what the slots hold: a `return` (`Terminator::Return`), a
  `return` from inside a handled block (`exit_to`), and a `Fail` exit
  (`fail_exit`, including a failure re-raised from a callee).

A sequence is therefore freed once, by its last owner. A `String` lives inline
in the frame, so its slot cannot be emptied by a move; a string is planned
only when it is never moved from, assigned or named by a handler, and a
`let`-bound string gets its own 24-byte storage so that rebinding it in a loop
frees the right buffer. `ChunkSeq::finish` frees the drained sequence.
`in`/`inout` parameters, closure captures, external captures, bindings inside
a destructuring pattern and `@fip` functions are not planned; they keep the
previous behavior. Freestanding images get no drops (no allocator).

The JIT runtime (`src/seq/native_runtime.rs`) keeps its live headers and
grown string buffers in hash maps (growth used to scan a `Vec` of every
header the thread ever made, quadratic in the number of sequences), exports
`vesper_seq_free` and `vesper_str_release` like the AOT runtime, and frees at
teardown only what generated code did not. `teardown_frees()` and
`stray_frees()` let tests prove that nothing was left for teardown and that no
drop hit a sequence that was not live.

**Option-free element reads** (`seq_native::fused_reads`). A `try_get`
whose result only its own block's `match` and that match's arm bindings
consume (`Some(x)`, `Some(_)`, `None`, `_`) no longer builds an `Option` in a
stack slot: it is one unsigned compare whose flag the arms branch on, and the
`Some` arm loads the element straight from the buffer. A `try_get` whose
`Option` is used as a value is unchanged. `get`, `s[i]` and `set` were already
an inline guard and a direct load.

**Export ABI v2 by-value sequences and strings** (own commit, see #934). The
refusal is lifted for `sink ChunkSeq[T]` (the Rust binding takes a `Vec<T>`
and copies it into a fresh sequence with `seq_from`; the callee frees it) and
`sink String` (the binding takes a `String` and copies it into a buffer of
the library with `with_string_owned`, so the callee may keep it in a returned
handle; the pilot's `note_adopt` tests that). Handles that own sequences stay refused
by value: native code does not free the sequences inside a dropped handle
yet. `docs/maintenance/porting-a-crate.md` and the two refusal lists in
`tests/native_export_v2.rs` and `crates/vesper-port-harness/tests/export.rs`
follow.

**Manuscript.** One sentence in the `ChunkSeq` bullet of chapter 13 states
when native code frees sequences and strings.

**Not done.** Header-load hoisting (item 2) and push-loop capacity hints
(item 5). Growth already doubles (minimum four elements, `vesper_seq_grow`).
Hoisting is blocked by how locals are lowered rather than by sequences: every
native local is mirrored in a stack slot and reloaded at each use
(`ir_graph_at`), so the handle itself is reloaded each iteration and Cranelift
at its default opt level cannot move the `len`/`data` loads. That belongs with
lane 1 (opt level) and lane 3 (bounds hoisting).

## Tests

- `tests/native_seq_drop.rs`: every exit kind (normal, early `return` from a
  loop, rebinding in a loop, assignment, a `Fail` exit caught by the caller,
  a `return` from inside a handled block, `finish`), moves (by value into a
  callee, moved then dropped, returned, through an aggregate, branches, moved
  inside a handled block), strings (loop locals, by-value parameters, early
  return), and borrowed `in`/`inout` sequences. Each program runs in the
  evaluator, the JIT with the optimizer on and off (asserting that teardown
  freed nothing and no drop was stray), an executable, and a C-ABI library
  whose `live_allocations()` must return to its start after 100 rounds. A
  C driver passes by-value sequences and strings across the export ABI. The
  checker refusals that make drops safe are pinned: use after a move or a
  by-value call, a slice bound to a name (E1521) or returned (E1520).
- `tests/native_seq_access.rs`: `match` on `try_get` with `Some`/`None`
  first, `Some(_)`, wildcard only, slices, `u8` elements, aggregate
  elements, and an `Option` kept as a value, across all backends.
- Existing suites rerun locally (release): `seq`, `strings`,
  `native_export_v2`, `native_export_cli`, `vesper_std`, `fip`, `loops`,
  `native_handlers`, `backends`, `native_batch`, `native_suspend_abi`,
  `source_native_suspension`.
- Under valgrind, a test program that leaked 118 blocks as an executable
  leaks none, with no invalid accesses, and the JIT run is clean.
- Ports, built from their PR branches with this compiler
  (`VESPER_PORT_COMPILER`): `cargo test --release --features vesper-port`
  passes for Quarry (#930) and Aura (#928).

**Workaround removed locally.** Quarry keeps every scratch sequence in fields
of its `Q` handle because locals leaked. Replacing one (`q.aux2` in
`execute_q`, which has eight early `return q` exits) with a local
`ChunkSeq[u64]` makes `tests/port_memory.rs` fail with the old compiler
("leaked over 200 cycles", 402 live allocations) and pass with this one. The
port branch was not changed.

## Stage 1 guard

No edits to `src/verified_codegen/**`, `src/ir.rs`, `ir_opt::optimize`,
`lean/**`, `bootstrap/**` or `packages/vesper-compiler/**`. The verified
subset has no sequences or strings, so neither drops nor fused reads reach
the verified backend. Digests: see the PR (the `bootstrap-stage1` workflow run
on this branch).

## Port ratios

Setup: 4-vCPU cloud x86-64 container, Linux, Rust 1.97 `--release` (debug
info on for profiling), each port built from its branch with the `vesper`
release binary of `main` (before) or of this branch (after) through
`VESPER_PORT_COMPILER`. Before and after were measured back to back in the
same session.

| Port | Benchmark | Before | After |
| --- | --- | ---: | ---: |
| Quarry (#930) | `planner_abba driver`, median of 9 ABBA epochs | 4.79x (+37949 bp) | 4.40x (+34023 bp) |
| Aura (#928) | `abba compare 15 200`, one-text | 3.84x | 3.68x |
| Aura | reorder | 5.25x | 4.59x |
| Aura | encode-audit | 4.25x | 3.97x |
| Ignis (#929) | plan, 64 passes | 8.5x | 6.9x |
| Ignis | plan, 256 passes | 10.1x | 8.2x |
| Ignis | build, 64 / 256 passes | 5.1x / 6.0x | 4.2x / 5.2x |
| Ignis | lower, 64 / 256 passes | 4.8x / 8.4x | 4.5x / 8.1x |

An earlier run of the same Quarry driver gave 4.87x before and 4.33x after;
the epochs range over about 1x, so read the Quarry and Aura rows as roughly
a 10% gain. Ignis has no committed benchmark: the rows come from a local
timing loop following its report's recipe (8 textures with 4 mips and a
buffer, direct and compute queues, alternating write/read passes, 200
iterations; median of 3 runs). Its graph differs from the report's, so its
absolute times are not comparable with that table.

Instruction counts (callgrind, same workload before and after):

| Port | Workload | Before | After | Allocator share before → after |
| --- | --- | ---: | ---: | --- |
| Quarry | `window 20` | 88.95 M | 84.64 M (-4.9%) | 23.2% → 24.4% (unchanged count) |
| Aura | `measure 20` | 1,745 M | 1,544 M (-11.5%) | 2.4% → 2.7% (unchanged count) |
| Ignis | 64 passes x 20 | 932 M | 818 M (-12.3%) | 16.8 M → 13.8 M instructions |

`malloc`/`free`/`realloc` cost 23% of Quarry's instructions and 2-3% of
Aura's. Quarry's and Aura's allocator work did not change: both ports keep
their scratch sequences in handle fields to avoid the old leak, so there are
no locals to free. About a fifth of Quarry's allocator time is
`vesper_seq_grow`; most of the rest is the Rust side of `src/port.rs`
encoding and decoding plans (lane 4). `try_get` plus its `Option` match is
inlined, so callgrind cannot attribute it. The instruction savings above,
which come from the fused reads, measure it. Ignis does use local sequences,
and its allocator instructions fell by 18%.

## What is left

- Free the elements of a dropped `ChunkSeq` of sequences, strings or boxes
  (today only its own buffer), then free the sequences inside a dropped
  handle, which would lift the last by-value refusal of the export ABI.
- Drop temporaries that are never bound (`f().len()`), destructured
  bindings, and closure captures.
- Ports can now move their scratch fields back into locals (Quarry `aux*`,
  `bits`). That is port work, not done here.
- Header-load hoisting and capacity hints, after lane 1 lands the opt level
  and the local mirroring is revisited.
