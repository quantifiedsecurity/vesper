# Stage 1 performance lane 3: byte loops

Part 1 (PR #942) added bounds-check hoisting and `mul_wide`. Part 2 (this
update) adds the block and fixed-width sequence builtins. Part 2 is described
first; the part 1 report follows unchanged except for "What is left".

## Part 2: copy, fill and fixed-width builtins

Baseline: `main` at 2368d88, after lane 2 (#932, PR #945) merged.

### What changed

- **New methods** on `ChunkSeq[T]` and `Slice[T]` (`src/seq.rs`,
  `SeqOp::CopyFrom`, `SeqOp::Fill`, `SeqOp::Wide`):
  - `s.copy_from(at, in src)` copies all of `src` to `s[at..at + src.len()]`;
    `try_copy_from` is its total form (`bool`). The element type must be copy.
    `src` cannot be a view of `s`'s own sequence (`E0301`, as for `extend`),
    but disjoint views of one sequence passed side by side
    (`f(inout s[0..k], in s[k..n])`) can be copied between.
  - `s.fill(v)` sets every element. Total.
  - `ChunkSeq::extend_from_slice` is another name for `extend`.
  - On `ChunkSeq[u8]` and `Slice[u8]`: `read_u16_le`, `read_u16_be`,
    `read_u32_le`, `read_u32_be`, `read_u64_le`, `read_u64_be`, the matching
    `write_*(at, v)`, and their total forms `try_read_*` (an `Option`) and
    `try_write_*` (a `bool`). Any other element type is `E1525`.
  - The checked forms check the whole interval once and raise
    `Fail[RuntimeError]` `IndexOutOfBounds` before writing anything, so a short
    buffer is never partly written. The total forms write nothing either.
- **Typing** in `src/seq/check.rs`, the source operand's `in` pass in
  `src/seq/ir.rs` (a one-line change), reference semantics in
  `src/seq/eval.rs`. The Lean model refuses the new operations
  (`src/seq/lean.rs`), as it does `inout` slices.
- **Native lowering** in the new `src/seq/native_bytes.rs`, a child module of
  `src/seq/native.rs` (whose hunk is the module declaration and a three-line
  dispatch). Copies are one bounds check and a `memmove` call; `fill` of bytes
  is a `memset` call, and other element types use a store loop. Wide reads and
  writes are one bounds check, one unaligned load or store of 2, 4 or 8 bytes
  and a `bswap` for big-endian. A freestanding image (no C library) falls back
  to byte loops. `extend` now also uses `memmove` instead of a byte loop.
- **`@vesper-std/bytes`** calls the builtins: each `read_*` and `put_*` is one
  `try_` call, and each `push_*` appends zero bytes and writes them with one
  `try_write_*`. The API and the `@vesper-std` tests are unchanged and pass.
- **Manuscript:** one new bullet in section 13 (sequences) specifies the
  methods, their single check and their overlap rule.
- **Tests:** `tests/native_bytes_builtins.rs`, 7 differential tests
  (evaluator, JIT with and without `optimize`, AOT executable): every width and
  byte order at every offset, including slices that start at odd bytes;
  writes read back; the total forms at and past the end, including the offset
  `u64::MAX`; checked forms that fault before writing, with the writes of the
  earlier calls kept; copies on sequences and slices, an empty copy at the
  end, a copy between disjoint views of one sequence; `fill` of bytes and of
  words; and the type errors.

### Stage 1 guard (part 2)

Nothing under `src/verified_codegen/**`, `src/ir.rs`, `ir_opt::optimize`,
`lean/**`, `bootstrap/**` or `packages/vesper-compiler/**` changed. The new
methods enter a program only when its source calls them, and the Stage 1
corpus does not. The `extend` lowering change is in the Cranelift path only.
The `bootstrap-stage1` run on this branch is linked from the PR body.

### Port measurements (part 2)

Same host and method as part 1: callgrind instruction counts (`Ir`), release
builds. "Before" is the unchanged port compiled by this branch, which includes
lanes 1, 2, 4 and 5. "After" rewrites the hot byte loops to the builtins in a
local experiment that is not pushed. The outputs are identical: the warp-tcp
worker printed the same results for Rust, before and after, and the 67
warp-tcp tests and Prism's `malformed` tests pass with the rewrite.

| Port, workload | Rust Ir | Vesper Ir before | Vesper Ir after | ratio before → after |
|---|---|---|---|---|
| warp-tcp codec, 64 requests | 33.9 M | 259.0 M | 75.1 M | 7.6x → 2.2x |
| warp-tcp transfer, 4 transfers | 8.74 M | 46.8 M | 19.0 M | 5.4x → 2.2x |
| Prism `tests/malformed.rs` | 101.8 M | 264.5 M | 252.7 M | 2.6x → 2.5x |

The rewrites:

- **warp-tcp** (`segment.ves`, `options.ves`, `conn_input.ves`): the checksum
  `sum_range_raw` reads 8 bytes at a time with `read_u64_be` and adds the two
  32-bit halves (RFC 1071: the folded one's complement sum is the same), then
  `read_u16_be` for the tail. `rd16`/`rd32`/`wr16`/`wr32` use
  `try_read_u16_be` and friends, keeping the old partial behavior at the end
  of the buffer. `copy_into` uses `try_copy_from` when the source interval is
  in bounds. The checksum was 92% of the codec's instructions.
- **Prism** (`decoder/src/syntax.ves`, `response/src/lib.ves`): `push_range`,
  the header copies of the response writer and `push_text` use
  `extend_from_slice` instead of a `push` per byte. Prism spends most of its
  time in byte-at-a-time scanning (`byte(in s, i)` in parser loops), which a
  copy cannot replace; lane 6's `find_byte` is the builtin for that.
- **BBR** has no byte loops. Its 64x64→128 multiply moved to `mul_wide` in
  part 1 (no measurable change, because its 32-bit fast paths almost never
  reach the full multiply), so it was not re-measured.

## Part 1: bounds-check hoisting and `mul_wide`

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

- Lane 6 (#936) builds the vectorized byte builtins (`sum_be16`, slice `eq`,
  `find_byte`) on part 2's lowering module, `src/seq/native_bytes.rs`.
- The ports adopting the builtins is port follow-up work; this lane only
  measured local rewrites.
- Hoisting extensions: strided indexes (`c + m*i`), accesses through `Str`
  (`try_byte_at`), and `inout` slice arguments to calls in the loop (callees
  cannot resize a slice). The last one is safe but not yet allowed.
- The Lean model of the new builtins (refused today).
- The native register model (above), outside this lane.
