# Self-hosting SH4 lane 6: native scalars, operators and control flow

Claim #1280. Stacks on #1259 (SH3 lane 1 part 1, `@vesper/place-ir`), which
stacks on #1232. Prompt: `docs/agent-prompts/self-host-sh4/06-codegen-scalar.md`
(plan PR #1293). Rust stays the default compiler; nothing here changes a
default build, edits the Rust lowering or deletes Rust code.

## What changed (part 1: the decisions)

The lane's emission and swap wait on lane 1 (#1275), which lends the Rust
`FunctionBuilder` to Vesper. Part 1 is the work that starts now: the parts of
`Codegen::ir_graph_at` and `Codegen::expr_at` (src/native.rs) that read the
Place IR and decide, before any Cranelift instruction is emitted.

- `packages/vesper-codegen-scalar` (`@vesper/codegen-scalar`), over
  `@vesper/place-ir`, `@vesper/check-data` and `@vesper/check-layout`:
  - `blocks.ves`: the block layout of one graph. The order blocks are lowered
    in (petgraph's `toposort` of the reached blocks, or a reverse
    `DfsPostOrder` from the entry when they have a cycle, then the unreached
    blocks in ascending order), the blocks on a cycle (`tarjan_scc` plus
    self loops), and the blocks the walk seals itself (every predecessor
    already lowered).
  - `lib.ves`: for every `Op::Constant`, `Op::Unary` and `Op::Binary`, the
    `expr_at` arm that lowers it, the operand's integer kind and Cranelift
    lane, and the run-time checks: `Fail[ArithOverflow]` (checked arithmetic
    with the exact overflow test `checked_arithmetic` uses, negation,
    narrowing bounds, shift range) and `Fail[RuntimeError]` (divisor). Also
    `Op::Loop`, `Op::Exit` and the terminators, with the `Never` operands
    that become default values.
  - `codegen_scalar_decisions`: the harness entry, one text dump per program.
- `tests/self_host_codegen_scalar.rs`: the Rust half (`decisions`) and the
  differentials below. `decisions` uses the guards of `expr_at`, `binary`,
  `checked_arithmetic`, `convert`, `shift` and `bit_builtin` with the
  compiler's own predicates (`numeric::conversion`,
  `integer_widths::bit_builtin`, `capability_effects::bit_intrinsic`,
  `strings::is_text`, `Layout::of`, ...) and the same petgraph calls as
  `ir_graph_at`.
- BUILD.bazel: data and a `long` timeout for the new test.

### Why the block order needs petgraph's exact behaviour

`ir_graph_at` lowers blocks in the order petgraph returns, and that order
decides the emitted CLIF. It is not just any topological order:

- A `DiGraphMap` keeps one adjacency list per node, with `(neighbor,
  direction)` entries in `add_edge` order. `toposort` visits the outgoing
  entries in list order.
- `ir_graph_at` removes the unreached blocks with `remove_node`, which
  `swap_remove`s the node from the node `IndexMap` and each entry naming it
  from the other lists. Removing a block that jumps to a reached block moves
  the last entry of that block's list into the hole, which changes the order
  `toposort` visits its successors in.

`blocks.ves` keeps the same lists and the same `swap_remove`s, so it matches
on graphs with unreached blocks (blocks after a `break` or `return`) as well.

## Results

Every corpus world (Cargo corpus), as the native path compiles it: with
optimize on, after `ir_opt`, the inliner, drop elaboration and bounds
hoisting; with optimize off, after `fip_reuse_pass` and drop elaboration
(584 programs, optimized test build, `CODEGEN_SCALAR_STEP=1`).

| Check | Programs | Differences |
|---|--:|--:|
| Vesper decisions = Rust decisions (34,684 functions, 1.65 M lines) | 584 | 0 |
| Operator mutants (below; 2.17 M lines, 62,993 error decisions) | 438 | 0 |
| Programs the format cannot encode | 0 | |

The clean corpus has no `i128`/`u128` values, bit builtins, rotations, float
`min`/`max` or `&&`/`||` left in the IR, so the second differential uses
**operator mutants**: every 4th program, three times, with every `Unary`
and `Binary` operator renamed round robin through every family of
`expr_at`'s arms (and names that are none of them), and every scalar value
retyped round robin through all twelve integer kinds, both floats, `String`
and `bool`. The mutants need not type check, since the decisions read only
names and types. They reach every arm except the two noted under "Findings".

Decisions the full corpus run exercises (Rust and Vesper agree on each):

| Decision | Corpus | Mutants |
|---|--:|--:|
| `const int` / `bool` / `string` / `f64` / `f32` | 282,079 / 46,907 / 13,714 / 982 / 202 | 366,063 / 55,812 / 13,692 / 81 / 306 |
| `binary icmp` / `fcmp` | 99,212 / 422 | 42,434 / 14 |
| `binary iadd` / `isub` / `imul` | 50,693 / 14,557 / 10,798 | 7,698 / 6,787 / 13,284 |
| `binary udiv` / `sdiv-guarded` / `urem` / `srem` / `wide-divide` | 1,179 / 24 / 361 / 25 / 0 | 5,282 / 5,280 / 4,065 / 5,115 / 1,538 |
| `binary` float `fadd` / `fsub` / `fmul` / `fdiv` / `fmod` | 558 / 443 / 937 / 102 / 8 | 0 / 0 / 1 / 1 / 2 |
| `binary checked` (overflow test chosen) | 1,217 | 1,269 |
| `binary divisor` (`DivisionByZero` check) | 128 | 61 |
| `binary shift` / `bitop` / `bitwise` / `bit` builtin | 9,343 / 16,192 / 195 / 0 | 23,030 / 23,103 / 9,853 / 10,708 |
| `binary str-compare` / `equal` / `logic` / `float-math` | 72 / 60 / 0 / 0 | 8,443 / 11 / 18,321 / 11,403 |
| `unary convert` / `not` / `bnot` / `fneg` / `ineg` / `float-unary` / `bit` | 28,988 / 7,521 / 415 / 133 / 14 / 1,350 / 0 | 18,949 / 2,857 / 2,376 / 388 / 3,370 / 13,602 / 8,700 |
| `binary error` / `unary error` (Rust's `Err` paths) | 0 / 0 | 56,847 / 5,828 |
| `loop` / `exit` | 21,600 / 12,954 | 24,528 / 10,875 |
| terminators `jump` / `branch` / `match` / `return` / `trap` | 623,935 / 113,408 / 40,907 / 34,684 / 76,089 | |

### Time

The decisions of all 584 programs, best of five, optimized test build, one
4-core cloud machine; the encoding is excluded, as SH3 measures passes:

| | Time | Ratio |
|---|--:|--:|
| Rust reference (`decisions` in the test) | 1.39 s | 1.00x |
| `@vesper/codegen-scalar` (`codegen_scalar_decisions`) | 0.91 s | 0.65x |

The Rust side here is the test's reference, which formats strings; it is not
the time `Codegen` spends deciding, which is folded into emission. The
scoreboard row (emission alone and the whole compile, ABBA and callgrind)
comes with part 2, when the Vesper decisions drive emission.

## Stage 1 guard

Part 1 adds a Vesper package, a test, a report and BUILD.bazel entries. It
does not touch src/** at all (so not `src/verified_codegen/**`, `src/ir.rs`,
`ir_opt::optimize` or src/native.rs), nor lean/**, bootstrap/** or
packages/vesper-compiler/**. Nothing installs a hook, so the default
compiler's objects are byte-identical to the base branch's. The Bootstrap
Stage 1 run on the branch is linked from the PR.

## Findings

1. **`@vesper/check-layout` has no `i128`/`u128` layout** (gap, minimal
   repro: `Layout::of(&world, &Type::named("i128"))` is a 16-byte scalar in
   Rust; `layout_of` on the same type returns `NONE32`, a layout failure). The package predates #1174 (SH2
   lane 2). It surfaced in the mutants as `error layout` where Rust decided
   `icmp`. This package decides the wide names before asking the layout, as
   `Layout::build` does, so it does not depend on the gap. The fix belongs in
   `packages/vesper-check-layout` (not this lane's path): add the two-word
   scalar before the one-word check.
2. **The `widen_u64` arm of `expr_at`'s `Unary` is unreachable.**
   `numeric::conversion("widen_u64")` is `(Widen, U64)`, and the conversion
   arm comes first, so the later `WIDEN_INTRINSIC` arm never runs (the
   mutants send `widen_u64` through `Unary` 1 in 18 times and always reach
   `convert widen`). Both arms emit the operand unchanged, so this is dead
   code, not a miscompile. The port keeps the arm, as Rust does.
3. **Not in the check-data input**: `Codegen::flushes` reads the function's
   `eff [ConstDenorm]` spans (`self.denorm`), which the harness input does not
   carry, so the float flush-to-zero decision is not compared here. It joins
   with emission (and lane 11 owns the float arms).
4. **Value-dependent decisions** wait for emission: `wide_shift_const` needs
   the shift amount to be a known constant (`constant_of` on the Cranelift
   value), and `coerce`'s narrow move versus mask depends on the optimize
   flag and the value's lane at that point.

## What is left

- Part 2, after lane 1's first PR: emission for the graph walk and the
  expressions through the lent builder, compared as per-function CLIF and
  object bytes through lane 1's harness, with optimize on and off.
- Part 3: the swap through the `scalar` hook, the full suite on the hybrid
  build, and the SH4 scoreboard row (emission alone and the whole compile,
  ABBA and callgrind).
