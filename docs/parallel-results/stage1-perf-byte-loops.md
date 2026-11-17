# Stage 1 performance lane 3, part 1: bounds-check hoisting and `mul_wide`

Claim: #933. Prompt: `docs/agent-prompts/stage1-perf/03-byte-loops.md`.
Baseline: `main` at 22b7296 (a descendant of 943d901).

This PR is the first half of the lane. The prompt and the coordinator asked
for the bounds-check hoisting and the 64x64->128 multiply now, and the new
sequence builtins (`copy_from`, `extend_from_slice`, `fill`, wide reads and
writes) after lane 2 (#932) merges, because both lanes edit
`src/seq/native.rs`. The builtins and the `@vesper-std/bytes` switch are not in
this PR.

## What changed

- **`src/bounds_hoist.rs` (new).** A Place IR pass on the native path. It runs
  only when `optimize` is on, after `ir_opt::optimize` and drop elaboration,
  in `native::compile_entry`, `native_export` (the `vesper build --lib` path
  the ports use) and `native_session`. For a `for i in lo..hi` (or `..=`)
  loop that indexes a sequence or slice with `s[i]`, `get`, `set`,
  `try_get` or `try_set` at `i` or at `c + i` (`c` loop-invariant), and that
  cannot change the length of `s`, it versions the loop: the preheader
  evaluates one guard, the range is empty or `c + hi <= s.len()` with no
  overflow, for every hoisted `s`. If the guard holds, a copy of the loop runs
  with the checks removed (`SeqOp::Hoisted`). If it fails, the original
  checked loop runs, so a short sequence faults at the same iteration with the
  same prior effects.
- The pass is conservative. It skips loops with closures, handlers, indirect
  calls or Box drop/reuse bookkeeping, loops entered other than through the
  header, and loops whose values escape. A receiver qualifies only when no
  instruction in the loop writes, rebinds or moves its root, or lends it as
  anything other than `in`, except element accesses, which never resize.
- **New ops.** `SeqOp::BoundsGuard { inclusive }` and
  `SeqOp::Hoisted(Access)` (`src/seq.rs`). Their Cranelift lowering is in
  `src/bounds_hoist/native.rs`, a child module of `src/seq/native.rs`, so the
  hunk in lane 2's file is the module declaration and a three-line dispatch.
  The evaluator gives them reference semantics (`src/seq/eval.rs`: a hoisted
  access is the checked access, and it errors if its guard was wrong). Lean
  extraction refuses them (`src/seq/lean.rs`). They never come from source.
- **The fast copy keeps the original instruction value ids.** Only block
  parameters get fresh ids. The native backend keeps each IR value in one
  frontend variable and reads every variable before each instruction, so every
  block takes the whole variable set as block parameters. A first version
  gave the copy fresh ids. That doubled the loop-carried variables, and the
  hoisted loop came out 25% *slower* than the checked one. With shared ids it
  is 15-20% faster (micro benchmark below). The two copies are disjoint paths,
  and the pass checks that no loop value is used outside the loop, so every
  use still sees its own copy's definition. State tokens stay consistent:
  `ir_check::evidence` is clean on the versioned IR, and the tests check it.
- **`mul_wide(a: u64, b: u64) -> (u64, u64)`** is the full product as
  `(low, high)`, as in Rust's `u64::widening_mul`. It is a reserved prelude
  function (`src/integer_widths.rs`), like the byte-order functions. Its
  source body works on 32-bit halves, so the evaluator, Lean and every backend
  agree. `native::call_function` lowers a call to it to one `imul` and one
  `umulhi`. A module that defines `mul_wide` gets `E0100` (`src/check.rs`,
  next to the reserved `wrapping_*` check).
- **Manuscript.** One sentence in the integer-kinds paragraph specifies
  `mul_wide`. Hoisting changes no semantics, so the manuscript has no text for
  it.
- **Tests.** `tests/native_bytes.rs` has 8 tests. Each program runs in the
  evaluator, the JIT with and without `optimize`, and an AOT executable. They
  cover slice sums, short sequences that fault (fallback), `try_get` past the
  end with the prior iterations' effects kept, `try_set`/`set` loops, `..=`,
  nested loops, offset copies (`dst[at + k] = src[from + k]`, including a
  wrapping offset of `u64::MAX`), a loop that pushes onto the sequence it
  indexes (not versioned), `mul_wide` against `u128`, and the reserved name.

## Stage 1 guard

- Nothing under `src/verified_codegen/**`, `src/ir.rs`, `ir_opt::optimize`,
  `lean/**`, `bootstrap/**` or `packages/vesper-compiler/**` changed.
- The pass runs only under `optimize` on the native path. `bootstrap/stage1.rs`
  builds the corpus with `optimize` off on both backends, so the pass never
  runs on it. `mul_wide` enters a module only if the module mentions it, and
  the corpus does not.
- The `bootstrap-stage1` workflow, run on this branch with
  `workflow_dispatch`, gives the verified artifact digests: see the PR body.

## Port measurements

Setup: Linux x86_64 cloud container, 4 vCPU, release builds, Rust 1.97.
"Before" is this branch with the pass switched off, using a local-only
environment switch that is not in the PR, and with the port sources
unchanged. That is the same code `main` generates. The wall-clock ABBA numbers
on this shared host were noisy: the Rust arm's median moved by up to 60%
between runs. So the main numbers are instruction counts (callgrind, `Ir`),
which are deterministic.

| Port, workload | Rust Ir | Vesper Ir before | Vesper Ir after | ratio before → after |
|---|---|---|---|---|
| warp-tcp codec, 64 requests | 33.9 M | 713.28 M | 713.29 M | 21.0x → 21.0x |
| warp-tcp transfer, 4 transfers | 8.74 M | 101.19 M | 97.92 M | 11.6x → 11.2x |
| Prism `tests/malformed.rs` | 101.8 M | 510.13 M | 510.13 M | 5.0x → 5.0x |
| BBR, 25,000 ack-path steps (`mul_full` → `mul_wide`) | n/a | 301.44 M | 301.42 M | no change |

Wall clock, for reference (`abba drive`, 12 epochs): warp-tcp codec was 13.99x
before and 8.27x after, but the Rust median moved from 814 to 1,337 ns in the
same runs. Transfer was 10.71x before and 10.89x after. BBR
`ack_path_abba abba` gave 95% intervals of 15.7-18.3x before and 14.3-19.2x
after. None of these wall-clock differences is outside the noise.

Micro benchmark (JIT, 200 sums over a 1,000,000-byte `Slice[u8]`,
`for i in 0..s.len() { acc = wrapping_add(acc, widen_u64(s[i])) }`): about
650-700 ms checked, 560-600 ms hoisted.

Why the ports barely moved:

- **The ports read bytes through helper functions.** warp-tcp uses `rd8`,
  `rd16` and `rd32`, and Prism uses `byte(in s, i)` and `Str::try_byte_at`.
  The pass sees only accesses in the loop body itself, so it needs lane 1's
  inlining before it can reach them. Prism's binary was byte-identical with
  and without the pass. warp-tcp's payload `copy_into` loop is the one that
  hoists: that is the 3% on transfer.
- **warp-tcp's checksum is a `loop` with a stride-2 index, not a counted
  `for`.** The codec spends 92% of its instructions there. As a local
  experiment, rewriting it as a byte-wise `for i in from..to` loop made it
  hoistable, but it then cost 1,270 M Ir checked and 1,252 M hoisted, against
  713 M for the original. Per-byte work in generated code costs far more than
  the bounds check. That loop needs the wide reads (`read_u16_be`) and SIMD
  lanes.
- **BBR rarely reaches `mul_full`.** Its 32-bit fast paths in `mul_div_sat`
  and `product_gt` handle almost every step. The ABI copies the BBR report
  measured (28%) are lane 4's work.
- **The per-iteration cost of generated loops is in the native backend's
  register model, not the bounds checks.** Every IR value is a frontend
  variable, and `ir_graph_at` reads all of them before each instruction, so
  each block carries the whole set as block parameters. Locals live in stack
  slots, reloaded every iteration. In the micro benchmark the checked loop
  body is about 25 instructions, plus dozens of block-parameter moves per back
  edge. The compare and branch the pass removes are 2 of them. Reading only
  the operands an instruction uses, or promoting scalar locals to SSA values,
  would help every port more than any one byte builtin. That is in
  `src/native.rs` (`ir_graph_at`), outside this lane, so it is reported here
  for lane 1 or a follow-up.

## What is left

- After lane 2 (#932) merges: `copy_from`, `ChunkSeq::extend_from_slice`,
  `fill`, `read_*`/`write_*` (u16/u32/u64, le/be) with one bounds check each,
  lowered to `memcpy`/`memmove` and unaligned loads with a byte swap. Then
  `@vesper-std/bytes` switches to them, and the ports are re-measured with
  their hot loops rewritten (warp-tcp's checksum and copies, Prism's byte
  reads).
- Hoisting extensions: strided indexes (`c + m*i`), accesses through `Str`
  (`try_byte_at`), and `inout` slice arguments to calls in the loop (callees
  cannot resize a slice). The last one is safe but not yet allowed.
- The native register model (above), outside this lane.
