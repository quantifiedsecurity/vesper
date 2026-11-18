# Port speed lane 3: `in` parameters in place, and hoisting in loops that call

Claim #1023. Base `72209eb` for the compiler experiments, merged with
`3c075a4` (#1020) before the final measurement. #1020 changed port sources
only, so the compiler at `72209eb` is the compiler at `3c075a4`.
Prompt: [03-params-and-hoisting.md](../agent-prompts/port-speed/03-params-and-hoisting.md).

**Result.** warp-wheel's `sum3` reads its `in s` parameter without a spill:
24 instructions to 20, and no stack slot or frame adjustment left. Hoisting
more than one sequence header in a loop that calls does not pay in any
form this lane tried: 8 rules were measured, and each one ran more
instructions in at least one port. The one-receiver rule stays, and tests
now pin it. On the ports the lane changes almost nothing: Ignis runs
0.009% fewer instructions, and the other six ports run exactly the same
count as on main. No port gets more instructions than on main.

## What changed

- `src/native.rs`, function entry (`in_param_registers`, used by
  `register_locals`). On the optimize path, an `in` parameter whose word
  is a handle (a sequence or a string) stays a frontend SSA variable
  holding the incoming argument. Before, it was copied to a zeroed home
  slot. Aggregate `in` parameters already arrive as pointers and never had
  a slot. A use that needs an address gets a scratch copy
  (`Codegen::address`, the existing register-local path).
  - Why this is sound: an `in` parameter is a shared borrow for the whole
    call. The checker rejects every write to it and every `inout` or `sink`
    use of it, and while the caller lends it as `in`, no other path may
    change it. So the word read at entry is the word at every later read,
    and nothing writes the scratch copy.
  - A name keeps its slot if the body rebinds it, writes through it, moves
    it, lends it other than `in`, or if a closure or handler names it.
  - **Leaf functions only.** When the function calls anything (a call, or
    a sequence operation that may allocate), the parameter keeps its slot.
    Across a call the incoming word needs a callee-saved register, saved
    and restored around the whole body, or a spill. Applied to every
    function, the change made Quarry's planner run 0.07% more
    instructions: one large function (`vesper_function_35`) gained 5
    callee-saved saves and restores per call. Ignis also ran 0.07% more.
- `src/bounds_hoist.rs`: `calls_out(op)` names the ops that may call out of
  the function. `header_hoists` and the entry rule above share it.
  Hoisting behaviour is unchanged.
- Tests: `tests/native_in_params.rs` and `tests/native_hoist_calls.rs`
  (see Tests).

## Repros

Built with `vesper build <file> --lib`, then `objdump -d`, counting
instructions in the emitted function (through `ret`).

| Repro | main | branch | Notes |
|---|---:|---:|---|
| warp-wheel `sum3` | 24 | 20 | Main: `sub $0x10,%rsp`, `movq $0,(%rsp)`, `mov %rdi,(%rsp)`, `add $0x10,%rsp`. The branch has none of these. |

## Hoisting more than one receiver in loops that call

Each variant changes only which receivers a calling loop hoists. Variants
marked "with entry change" also include the entry change applied to all
functions, before it was limited to leaf functions. Instructions are
callgrind `Ir` for the whole process, on the port sources at `72209eb`
(the "before" arm of port-gaps-rescore.md). Each cell is the change
against main.

| Variant | BBR | Quarry | Prism | Ignis | Aura | tcp codec | tcp transfer |
|---|---:|---:|---:|---:|---:|---:|---:|
| main (49.45 M / 31.09 M / 73.07 M / 165.60 M / 327.26 M / 24.30 M / 12.32 M) | | | | | | | |
| entry change in all functions, hoisting as main | 0.00% | +0.07% | 0.00% | +0.07% | -0.02% | 0.00% | -0.01% |
| entry change in all functions, plus top 1 receiver by read count | 0.00% | +0.14% | -0.27% | +1.77% | +0.12% | 0.00% | -0.01% |
| entry change in all functions, plus top 2 by read count | 0.00% | +0.12% | -0.12% | +2.95% | +0.14% | 0.00% | -0.01% |
| entry change in all functions, plus top 3 by read count | 0.00% | +0.12% | -0.11% | +3.97% | +0.54% | 0.00% | -0.01% |
| entry change in all functions, no cap | 0.00% | +0.12% | -0.12% | +6.12% | -0.25% | 0.00% | 0.00% |
| leaf entry change, receivers with more reads than the loop has calls | 0.00% | +0.50% | +1.39% | +2.87% | +0.52% | +0.01% | -0.18% |
| leaf entry change, same rule, at most 2 receivers | 0.00% | +0.50% | +1.39% | +2.87% | +0.52% | +0.01% | -0.17% |
| leaf entry change, receivers with 2 x reads > calls | 0.00% | +0.23% | -0.11% | +5.26% | +0.43% | +0.01% | -0.18% |
| leaf entry change, several receivers only in loops of at most 8 blocks | 0.00% | +0.01% | 0.00% | -0.01% | 0.00% | 0.00% | -0.01% |
| leaf entry change, several receivers only in loops of at most 16 blocks | 0.00% | +0.01% | +0.04% | -0.01% | 0.00% | 0.00% | -0.01% |
| leaf entry change, top 2 receivers in loops of at most 16 blocks | 0.00% | +0.01% | +0.04% | -0.01% | 0.00% | 0.00% | 0.00% |
| leaf entry change, top 2 receivers in loops of at most 32 blocks | 0.00% | +0.02% | +0.03% | -0.01% | -0.09% | 0.00% | 0.00% |
| **leaf entry change, hoisting as main (shipped)** | 0.00% | +0.01%* | 0.00% | -0.01% | 0.00% | 0.00% | -0.01% |

\* The Quarry difference of the shipped variant (+2,230 instructions) is all
in libc's `sscanf`/`strtoul`, which the harness uses to read `/proc`. It is
not in port code. In the final run (below), the two Quarry counts are
identical.

What the variants show:
- The read-count and reads-against-calls rules lose. Ignis's calling loops
  are large outer loops: in `hazard_edges`, a 454-block loop with 10 calls
  and 7 hoistable receivers; in `assemble`, 130 blocks with 12 receivers.
  A hoisted header is live across all of them, inner loops included, so it
  takes a callee-saved register or spills. Ignis's
  `vesper_function_173` alone ran 2.28 M more instructions (+13%) with
  the top-1 rule.
- "More reads than calls" also drops main's single-receiver hoists. Prism
  loses 1.4% under it, so those hoists pay even in loops with many calls.
- In small loops, several receivers are neutral, give or take 0.1%.
  The 32-block variant saves 0.09% on Aura but costs Prism 0.03%.

Callee-saved registers are already what Cranelift's register allocator
picks for values live across calls. Placing hoisted words in them by hand
would mean pinning registers, which Cranelift's frontend does not offer.
Choosing receivers by register pressure would need pressure numbers from
the allocator, which the IR level does not have. Static read counts, the
proxy tried here, lose.

## Ports

Setup:
- Machine: cloud x86-64 container, 4 vCPU, Linux 6.18, Rust 1.97
  `--release`.
- Port sources at `3c075a4` (after the #1020 port rewrites).
- Builds: each port's example was built with `VESPER_PORT_COMPILER` set to
  the release `vesper` of main or of this branch (`optimize = true`).
- Instructions: callgrind `Ir`, whole process, with the commands of
  port-gaps-rescore.md: BBR `ack_path_abba measure 20000`, Quarry
  `planner_abba window 20`, Prism and Ignis `abba measure 20`, Aura
  `abba measure 40`, warp-tcp `abba worker codec` (64 requests) and
  `abba worker transfer` (4 requests).
- The Rust column is copied from port-gaps-rescore.md and was not measured
  again here.

| Port | Rust Ir (rescore) | Vesper Ir, main | Vesper Ir, branch | Change | Ratio, main | Ratio, branch |
|---|---:|---:|---:|---:|---:|---:|
| BBR | 21.97 M | 49,254,538 | 49,254,538 | 0 | 2.24x | 2.24x |
| Quarry | 19.36 M | 30,791,314 | 30,791,314 | 0 | 1.59x | 1.59x |
| Prism | 53.31 M | 65,010,478 | 65,010,478 | 0 | 1.22x | 1.22x |
| Ignis | 116.17 M | 165,597,828 | 165,582,724 | -15,104 (-0.009%) | 1.43x | 1.43x |
| Aura | 362.86 M | 313,347,094 | 313,347,094 | 0 | 0.86x | 0.86x |
| warp-tcp codec | 33.94 M | 24,298,908 | 24,298,908 | 0 | 0.72x | 0.72x |
| warp-tcp transfer | 8.74 M | 12,316,867 | 12,316,867 | 0 | 1.41x | 1.41x |

warp-wheel, measured by wall clock with `tests/abba.rs --ignored` (both
builds in one process), each arm twice, in the order main, branch, main,
branch:

| Arm | Run 1: median Vesper/Rust [95% interval] | Run 2 |
|---|---|---|
| main | 1.824x [1.812, 1.862] | 1.816x [1.782, 1.834] |
| branch | 1.816x [1.787, 1.830] | 1.799x [1.776, 1.828] |

The intervals overlap, so the change is within noise. `sum3` is a repro
written from the port. The port's own hot functions call out (they push
and allocate), so they keep their slots.

The ports barely move because their hot functions call. The leaf rule
leaves those parameters in their slots, and the rule exists because
dropping the slots there cost instructions (Quarry and Ignis, +0.07%).

## Tests

- `tests/native_in_params.rs`:
  - `sum3` emits no stack slot and stores no incoming argument on the
    optimize path. The unoptimized path still stores it.
  - A function that calls keeps its parameter homes, and its leaf callee
    reads in place. Sequences and strings are both lent on to calls.
  - A caller that pushes between calls: each call reads the length at its
    own entry.
- `tests/native_hoist_calls.rs` (plans from `header_hoists`):
  - A calling loop hoists its only receiver.
  - A calling loop with two receivers hoists neither.
  - A loop that calls nothing hoists both.

Every program in both files runs in the evaluator, the JIT with the
optimizer on and off, and as x86-64 and AArch64 objects, and the results
must agree. `native_in_params` also builds and runs an executable.

All 24 `tests/native_*.rs` suites pass locally. `cargo +1.100.0 clippy -p
vesper --lib --test native_in_params --test native_hoist_calls -D warnings`
is clean.

## Stage 1 guard

No file in the guarded set changed: `src/verified_codegen/**`, `src/ir.rs`,
`ir_opt::optimize`, `lean/**`, `bootstrap/**` and
`packages/vesper-compiler/**` are untouched. The PR's bootstrap-stage1 run
holds the digest evidence.

## What is left

- Parameters of functions that call still get a home slot and a zero
  store. A home can skip its zero store when it is written in full at
  once. Measured as an experiment: warp-tcp transfer -0.18%, Aura -0.16%,
  Prism -0.12%, Ignis -0.08%, but Quarry +0.14%. Quarry's
  `vesper_function_35` started using 5 callee-saved registers, because
  Cranelift forwarded more slot loads to the stored values. That variant
  was not shipped. A store-to-load forwarding limit, or a
  rematerialization hint, would let it ship without the Quarry cost.
- More than one hoisted receiver in calling loops needs register-pressure
  information from the allocator, or hoisting into the innermost loop that
  keeps the receiver stable instead of the outermost one. Neither is
  reachable from the IR-level plan as it is.
