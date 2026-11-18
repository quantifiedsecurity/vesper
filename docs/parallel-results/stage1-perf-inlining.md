# Stage 1 performance lane 1: inlining and Cranelift `opt_level=speed` (#931)

Baseline: `main` at 943d901, with `main` at 10dc20a merged in before the PR.
The [wave plan](../agent-prompts/stage1-perf/README.md) has the rules; the
gaps come from the P1 port reports.

## What changed

Optimized native builds (`optimize = true`) on the JIT, library-export
(`vesper build --lib`, which every port uses) and session paths now:

1. **Inline small Vesper functions.** `src/ir_inline.rs` is a Place IR
   inliner. It runs after `ir_opt::optimize` and before
   `fip::elaborate_non_fip_drops`, and is never called from
   `ir_opt::optimize`, so the verified backend's input does not change.
   It walks the call graph bottom-up (Tarjan SCCs, callees first) and
   replaces a direct call when the callee:
   * is not recursive, not generic and not `fip`;
   * has at most 48 IR instructions, or has one call site in the program and
     at most 240;
   * has an empty effect row, or exactly the caller's row with only
     unlabeled `Fail[E]` instances (a raise in the inlined body performs the
     caller's `Fail[E]` capability, which is what the re-raise after the
     call did);
   * has no closures, handlers, `?`, indirect calls, tiled or `Box`
     operations, and no `Box`, function or slice parameter types.

   Each caller may grow by 2,000 instructions and the program by 40,000.
   Callers whose row names a capability effect (`ConstTime`, `Realtime`,
   `DetFloat`) keep their calls, and so do callers holding `Box` values:
   `fip::elaborate_non_fip_drops` places `Box` drops per block, and
   splitting a block at an inlined call let it free a list that had been
   moved into a pending argument (caught by `bounded_generics` in CI). `inout` arguments, and borrowed aggregate
   or sequence `in` arguments, are not copied: the callee's reads, writes
   and argument places of `p.f` become the caller's `place.f`. By-value
   parameters are bound to renamed locals (`name$inlN`). Callee values and
   blocks are renumbered; `return` and the final value jump to a
   continuation block whose parameter is the call's result. State tokens
   are re-threaded with `ir::thread_tokens`.
2. **Read only an instruction's operands.** `ir_graph_at` used to read
   every IR value of the graph from its frontend variable before every
   instruction and terminator. Cranelift's SSA builder then gives every
   block a parameter for every value, so the cost grows with values times
   blocks, and inlining (more values, more blocks) made the BBR port
   **slower**: 694 to 1,075 ns per step in a first measurement, before this
   fix. Optimized builds now read only the instruction's operands (and still
   read everything where `Box` drops are emitted). This is a `Codegen`
   field, `operands_only`, set from `optimize`.
3. **Cranelift `opt_level=speed`** for optimized JIT, library and session
   modules. Unoptimized builds keep the default `none`.

Not changed:

* **Executable objects** (`native::object*`, `vesper build`) run the
  operand-only reads but keep every call and `opt_level=none`. The
  integration-B verifiers report and budget the call graph of exactly the
  objects `vesper build` emits (`stack ... via ct_main -> select`), recompile
  emitted functions with `object_isa_for`, and require an observed build to
  be byte-identical to a plain one. Inlining there needs verifier-aware
  boundaries; see "What is left".
* `native_batch` and the bench JIT (`bench_commands.rs`) never run the
  optimizer (`runtime.optimize = false` is deliberate there: the benchmark
  counts evaluator steps on the unoptimized IR and cross-checks the JIT
  against it), so they were left alone.

## Stage 1 guard

No file under `src/verified_codegen/**`, `src/ir.rs`, `src/ir_opt.rs`,
`lean/**`, `bootstrap/**` or `packages/vesper-compiler/**` changed. Every new
behaviour is behind `optimize`, and `bootstrap/stage1.rs` builds the corpus
with `optimize` off on both backends.

Evidence: `bootstrap/run.sh stage1 --outcome` on `main` (10dc20a) and on this
branch, in the same cloud container, wrote fresh locks whose every program
entry (11 `cranelift_artifact` digests, 11 `programs.verified.artifact`
digests, results and sources) is byte-identical; only `seed_binary` and
`frontend_sources` differ, as they do for any change to the compiler's
sources. Both runs report the same stale fields against the committed
`bootstrap/stage1.lock` (`seed_binary`, `frontend_sources` and every
program's `cranelift_artifact` and `verified`), so that staleness is already
on `main` and is not from this PR. The `bootstrap-stage1` workflow run on the
branch (run 44177732985) passed reproducibility, the ratchet and the strict
cross-backend gate with the same stale list.

## Tests

`tests/native_inline.rs` (14 tests) runs each program through the evaluator
(optimized and not), the JIT with `optimize` on and off, and x86-64 and
AArch64 objects: nested calls, early `return`, `inout` scalars and fields,
borrowed and returned aggregates in a loop, a by-value copy that must not
alias, loops in callees, `Fail[RuntimeError]` rows inlined into the same row,
checked-arithmetic `Fail[ArithOverflow]` reaching the handler through two
inlined levels, a pure callee inside a `Fail` row, generic instances, and
`ChunkSeq` helpers taking the sequence `inout` and `in`. IR-level tests
check that recursion, mutual recursion, generic callees, closure bodies and
over-budget callees stay calls, that `Budget::none()` does nothing, and that
the token evidence (`ir_check::evidence`) is clean after inlining.

Existing suites run locally with the change: backends, checked_arithmetic,
effect_optimization, fip, floats, integer_widths, loops, native_batch,
native_export_v2, native_export_cli, native_handlers, native_spans,
packages, regions_backends, rewrite_runtime, seq, source_native_suspension,
strings, capability_verifiers, native_coverage_cli, native_verifiers_cli,
vector, freestanding, doc_cli, ebpf_cli, ffi_lowering. `native::jit` defaults
to `optimize = true`, so every JIT test in them goes through the inliner.

## Port measurements

Setup: Linux cloud container, 4 vCPU, release profile, Rust 1.97. Each port
was built from its branch (BBR and warp-tcp from `main`, Quarry from PR #930)
before and with this change applied locally; nothing was pushed to port
branches and no default build changed. Wall-clock numbers come from each
port's own ABBA driver and are noisy on this shared host (the same binary
varied by about 20% between runs); the callgrind instruction counts are
deterministic. "Operands only" is the build with this change but the inliner
and `opt_level=speed` switched off.

### BBR (`examples/ack_path_abba.rs`, 12 epochs x 200,000 steps)

| Build | Vesper/Rust, median of epochs | Instructions, 25,000 steps (callgrind) |
| --- | --- | --- |
| before | 14.1x (95% interval 12.6x to 16.1x) | 301.4 M |
| operands only | 10.9x | 238.4 M |
| + `opt_level=speed` | 11.6x | 232.6 M |
| + inlining | 10.3x | 216.2 M |
| this PR (both) | 9.2x to 10.1x (two runs; 95% intervals 7.9x to 11.1x and 9.5x to 11.3x) | 207.9 M |
| Rust | 1x | 26.4 M |

Of the 207.9 M, 82.8 M are the binding's `Abi::get`/`Abi::put` copies of
`Bbrv3` and `RateSample` (lane 4's gap); the Vesper code itself went from
217.5 M to 125.1 M instructions (-42%). Calls into Vesper functions over the
callgrind window went from 3.84 M to 0.99 M. The inliner replaced 241 call
sites of 80 functions in the BBR unit (`max` x26, `sat_add` x23,
`infinity` x22, `min` x18, `sat_sub` x11, ...). Raising the small-callee
budget to 96 or 160 did not measurably help, so it stays at 48.

### Quarry planner (PR #930, `examples/planner_abba.rs`, 9 epochs)

| Build | Vesper/Rust, median regression | Instructions, 20 passes (callgrind) |
| --- | --- | --- |
| before | 4.78x | 89.0 M (4.57x) |
| operands only | 3.09x | 59.9 M (3.08x) |
| + `opt_level=speed` | 3.44x | 57.6 M (2.96x) |
| + inlining | 3.31x | 57.4 M (2.95x) |
| this PR (both) | 3.33x | 54.9 M (2.82x) |
| Rust | 1x | 19.5 M |

### warp-tcp (`examples/abba.rs`, 12 epochs, `drive codec` and `drive transfer`)

| Build | codec, Vesper/Rust median | transfer, Vesper/Rust median |
| --- | --- | --- |
| before | 12.61x | 10.51x |
| operands only | 6.15x | 7.51x |
| + `opt_level=speed` | 4.29x | 5.31x |
| + inlining | 6.49x | 5.00x |
| this PR (both) | 4.24x | 5.97x |

Both workloads' digests agree between the builds in every run.

### Share of the change

On the deterministic BBR and Quarry instruction counts, the operand-only
reads give about two thirds of the gain, inlining most of the rest, and
`opt_level=speed` the smallest part (2% to 4% on its own). On warp-tcp's
byte loops, `opt_level=speed` matters more (codec 6.2x to 4.3x). All ports
stay slower than Rust, so Rust stays the default build of every port.

## What is left

* ~~**Executable objects.**~~ Done in #959, see below.
* **Scalar locals live in stack slots.** Every scalar local gets an 8-byte
  slot that is kept in sync with its variable, and inlined callees add their
  locals to the caller. This is now the largest per-call cost left in the
  BBR profile after the ABI copies.
* **Callees with handlers** (`div` and `pow2` in BBR wrap `a / b` and
  `1 << n` in a local `handle Fail[..]`) stay calls. Inlining a handled body
  needs the handler's scope kept intact in the caller.
* **Generic callees** stay calls; inlining them needs type substitution in
  the copied IR.
* The ABI copies (lane 4), sequence access and drops (lane 2) and byte loops
  (lane 3) remain the larger gaps in BBR, Quarry and warp-tcp.

## Follow-up: executable objects (#959)

Optimized executable objects (`vesper build`, `vesper emit --emit object`,
`native::object*`) now run the same inliner and Cranelift `opt_level=speed`
as JIT and library builds. `--no-opt` objects are unchanged.

* **The verified build is the shipped build.** Whether to inline depends only
  on the world and `optimize`, never on the verifier observer, so an object
  built with `--verify-*` is still byte-identical to the plain one
  (`programs_compile_to_identical_code_without_verifier_flags`). The
  verifiers check the code that ships, inlined bodies included.
* **The stack verifier recompiles with the build's ISA.** `EmittedFunction`
  now records `optimized`, and `measure_emitted` builds its ISA with
  `object_isa_optimized`, so frame sizes are measured at the same opt level
  the object used. Windows objects and `emit_capability_entry` (WCSD for
  docs) use the same ISA.
* **Capability functions keep their call graph.** The inliner already skips
  callees with a capability effect and callers whose row names one
  (`ConstTime`, `Realtime`, `DetFloat`), so the derived capability policies
  see the calls they saw before.
* **Policy files.** Inlined callees are still emitted, so policies naming
  them still resolve. A stack path through an inlined helper now ends at the
  caller (`via ct_main` instead of `via ct_main -> select`; the test checks
  both, the long path with `--no-opt`). If a const-time `declassify` or
  `constant_time_callees` entry names a callee the function no longer calls,
  the report says so and suggests `--no-opt`; checking the inlined body can
  only add findings, never hide one.
* **Inline provenance.** `ir_inline::Report::inlined_into` and
  `EmittedFunction::inlined` list every body copied into a function
  (transitively). The realtime verifier adds a helper's declared
  `realtime.effects` to each function holding its copy, so inlining cannot
  turn a forbidden effect into an accepted root (test: `clean_main` with
  `forward: ["Alloc"]` is rejected with and without `--no-opt`). Attested
  `loop_bounds` are not carried over: an inlined loop is then unbounded in
  the caller, which can only reject.
* **Instrument hooks** (`object_instrumented`) see the same inlined code, so
  an inert hook still leaves the bytes unchanged. `vesper fuzz
  --native-coverage` builds its JIT unoptimized and is not affected.
* **Stage 1.** `bootstrap/stage1.rs` builds objects with `optimize` off, so
  the corpus artifacts, the lock and the verified backend do not change. No
  file under `src/verified_codegen/**`, `src/ir.rs`, `src/ir_opt.rs`,
  `lean/**`, `bootstrap/**` or `packages/vesper-compiler/**` changed.
