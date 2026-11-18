# Port gaps lane 3: in-place borrows (#986)

Base `07e7d4e` (origin/main, 2026-11-18). PR #1004. The manuscript
(`docs/manuscript/`) is the sole specification. Its §4.3 and §13 hunks are
in this PR and flagged in the PR body.

## What changed

| Gap (port report) | Change |
|---|---|
| Quarry gap 3: `q.a.extend(in q.b[..])` refused (E0301) | Builtin receivers and their source views (`extend`, `copy_from`, `try_copy_from`, `push_str`) are compared by field path (`Place::overlaps`), which is the rule call arguments use. `q.a.extend(in q.a[..])` is still refused. |
| Aura gap 3: no in-place shift | `s.copy_within(lo, hi, at)` and `try_copy_within` on sequences and slices of copy elements: one bounds check, then `memmove`. In a freestanding image, a byte loop that runs backwards when the destination is above the source. |
| Aura gap 2: no swap or replace through `inout` (E0305) | `swap(inout a, inout b)` and `replace(inout a, v)` for any type. The places must be disjoint (E0301) and interchangeable (E0403), and both places lose their value facts, as `inout` call arguments do. A declared function of the same name shadows the primitive. The E0305 help now points at them. |
| Aura gap 5: receiver loan blocks argument reads (E0410) | Two-phase borrows: an `inout` receiver loan is reserved while the arguments are evaluated. The seed checker reserves only receivers (`reserved_receivers`). The Place IR check reserves only `inout` loans that some call takes as an argument (`reservable`), and `Flow::activate` refuses an argument loan still live at the call. |

Where it lives:
- `src/seq.rs`: `SeqOp::{CopyWithin, Swap, Replace}`. Swap and Replace share `Op::Seq`, like `SeqOp::Str`, so the Place IR types are unchanged.
- Typing: `src/seq/check.rs`.
- Lowering: `src/seq/ir.rs`.
- Evaluator: `src/seq/eval.rs` and the `Op::Seq` arm of `src/runtime.rs`.
- Native: `src/seq/native_bytes.rs` and `src/seq/native.rs::seq_place_primitive`, used from both the AST path and the IR path of `src/native.rs`.
- Lean: `src/seq/lean.rs` refuses the three new operations, as it does `copy_from`.
- Checkers: `src/check.rs` and `src/ir_check.rs`.

Decision: `copy_within` takes three `u64` operands `(lo, hi, at)` rather than a
`lo..hi` argument. This matches `copy_from(at, in src)` and needs no range
value.

Not edited (Stage 1 guard): `src/verified_codegen/**`, `src/ir.rs`,
`ir_opt::optimize`, `lean/**`, `bootstrap/**` and `packages/vesper-compiler/**`.
The self-hosted checker does not reserve receiver loans and does not know the
new builtins, so it stays stricter. The Lean model in `lean/` is untouched.
Lean extraction already evaluates arguments before the update it emits, and
`tests/two_phase_borrows.rs` checks that.

## Stage 1 guard

Bootstrap Stage 1 workflow on the PR branch:
https://github.com/robot-head/vesper/actions/runs/44259127663. The result is
recorded in the PR body once the run finishes.

## Repros, before and after

Measured with callgrind instruction counts (`Ir`) of `vesper build`
executables built by the PR's compiler, on a 4-core x86-64 Linux container.
Sources are reproduced in the PR thread.

| Repro | Before | After | Ratio |
|---|---:|---:|---:|
| Aura gap 3: 20,000 rounds of `insert_at(3)` and `remove_at(7)` on a 256-element `ChunkSeq[u64]`. Before is the element-by-element loop from `util.ves`; after is `copy_within`. | 181,862,764 | 10,623,807 | 0.058x |
| Aura gap 2: 5,000 commits of a 512-word and 2,000-byte state. Before is `copy_state` back into `st`; after is `swap(inout rec.st, inout rec.stage)`. | 7,342,921 | 3,952,922 | 0.54x |
| Aura gap 5: `s.try_set(i, wrapping_add(<try_get of s>, 1))`, instructions in the emitted function (objdump). Before is the `let` form; after is the two-phase form. | 29 | 26 | no run-time cost |
| Quarry gap 3: `q.a.extend(in q.b[0..1])` | refused (E0301) | accepted, runs (1) | code only |

## Ports

**Aura.** A local rewrite, not committed (lane 9 adopts it):
- `util::insert_at` and `remove_at` use `try_copy_within`.
- `recording::insert_edge`, `remove_edge` and `move_edge` use `try_copy_within`.
- `apply.ves` publishes a commit with `swap(inout rec.st, inout rec.stage)` instead of `copy_state(in rec.stage, inout rec.st)`.

It is 3 files, +7/-60 lines, and `cargo test --release -p vesper-aura` passes
with it.

The Rust baseline needs a historical worktree build, so I compared the port
before and after with the committed `examples/abba.rs` instead:
`abba compare <before> <after> 15 200`, run in A-B-B-A order.

| Workload | Before ns/op | After ns/op | After/before (95% median interval) |
|---|---:|---:|---|
| one-text | 131,449 | 123,615 | 0.97x (0.79x..1.06x) |
| reorder | 189,842 | 171,739 | 0.99x (0.85x..1.05x) |
| encode-audit | 82,069 | 77,845 | 0.95x (0.86x..1.04x) |

Under callgrind, `abba measure 50` (all three workloads with warm-up) takes
412,112,307 instructions before and 400,890,451 after (-2.7%). That matches the
profile shares the parity lane gave: `copy_state` 3.0% (half removed) and
shifts 1.4%. The wall-clock intervals include 1.0x, so on this machine the gain
is within noise. The scoreboard's port/Rust ratios (0.42x-0.85x) would move by
about this much. I did not measure them against the Rust baseline here.

**Quarry** is code only: its gap-3 workaround can now be written directly. I
did not re-measure it.

## Tests

- `tests/borrow_paths.rs`: field-path disjointness, accepted and refused. The swap/replace rules (E0301, E0308, E0300, E0200, E0403), fact invalidation, and shadowing by a declared `swap`. The Aura publish shape is checked differentially in the evaluator, the JIT with optimize on and off, and an executable. Sequences are freed exactly once.
- `tests/native_copy_within.rs`: all 729 `(lo, hi, at)` triples up to one past a 7-element sequence, checked against `slice::copy_within`. Also byte sequences, slice receivers, the insert/remove shapes, checked faults that write nothing, and typing.
- `tests/two_phase_borrows.rs`: accepted reads (differential, including `impl` methods), refused writes, moves and `inout` borrows, the receiver-only rule, and Lean extraction order.

## What is left

- A struct local that holds sequences still leaks in the JIT (lane 2, #985). `swap_and_replace_add_no_leaks` checks that the primitives leave exactly the baseline teardown count.
- Lean extraction of `copy_within`, `swap` and `replace` (the `Array` model can express them).
- The self-hosted checker could adopt field-path disjointness and two-phase reservations after Stage 2.
