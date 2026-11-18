# Stage 1 perf lane 5: Fail across stationary `inout` (keep-writes)

Claim: #935. Prompt: `docs/agent-prompts/stage1-perf/05-fail-lowering.md`.
Baseline: `332fbdb` (main after plan #938).

Two PRs. The **language change** (prompt steps 1-4) is #943, merged
2026-11-17. The **native lowering and port measurements** (steps 5-6) are
the second PR, which closes #935. It was based on `cf07cdf` (main after #943,
with lane 1's inlining #944 and lane 4's export ABI v3 #940 in).

## Owner decision

From the owner's language design review (2026-11-17): an abortable effect
(`res(epsilon) = affine`: `Fail`, `Cancel`, any non-linear effect or open row,
never chosen by effect name) may leave a call while a *stationary* `inout`
argument is live. The caller's place keeps the last value the callee wrote
before the abort, as with a Rust `&mut` (option A). Roll-back (copy-in/copy-out,
option B) is rejected: it cannot snapshot affine/linear/pinned places and
copies on every call.

## What changed

### Checker (`src/check.rs`)

- `check_abort_frame_for` no longer raises E0402 for an active `inout`
  parameter or `inout` argument loan. It still does when the place has an
  open move-out hole (a torn value would reach the caller).
- Stationarity, `out` slots and parameter refinements: the production
  checker has no `~>` typestate transitions, `out` parameters or refinement
  types, so every `inout` is stationary and those conditions are vacuous
  today. The manuscript states them for when those features land.
- Linear ownership is unchanged on owned bindings ([T-Let]/E0301/E0402): the
  frame that owns a linear value still may not let an abort unwind it.
- New: an abort caught by a lexical `handle` in the same frame does not
  unwind bindings declared outside that handler. An outer linear value that
  is live at such a checkpoint is allowed, and the handler then requires the
  body not to change its consumption state (new E0402 "linear value 'tx' is
  consumed inside a handled body that may abort before consuming it"). This
  is what lets the owner of `tx: WriteTxn` call `append(inout tx, ...)` inside
  `handle Fail` and consume `tx` afterwards. It also fixes the scoping gap the
  review found, where a `Fail` caught inside `fn f(inout x: T)` was rejected.
- Fact havoc: after an abortable handler, the code that follows joins the
  normal edge with the catch edge, which may skip writes the body made after
  its checkpoint or keep a write an `inout` callee made before failing. The
  checker now forgets path facts (integer values, non-zero) about every place
  the handled body wrote. Before this, `100 / x` after such a handler was
  accepted without `Fail[RuntimeError]` on a fact only the normal edge
  established. Handler clauses capture snapshots at handler entry, so their
  facts are unchanged.

### IR ownership check (`src/ir_check.rs`)

The Place IR flow check repeated the old rule: E0410 "abortable effect
crosses a mutable temporary loan", and "may abandon linear place" for a
handler body's external captures. The loan diagnostic is removed (the loaned
place stays in the frame). A handler body's external captures are not
abandoned by an abort: one that escapes the handler is checked on the
enclosing frame's `Handle` instruction, as before. Linear temporaries and
linear `sink` siblings stay protected (E0410).

### Lean Tier-1 extraction (`src/lean.rs`, coordinated on #593)

`restore_states` restored `inout` places only after a successful bind, which
modelled roll-back. A function that takes `inout` parameters and may abort now
takes a trailing `$sync : optParam (Σ_io → m Unit) (fun _ => pure ())`. It
calls `$sync` with its current `inout` values before every action that may
abort. A direct caller passes a `$sync` that writes those values into its
borrowed places (in fresh shadows) and forwards them to its enclosing handler
frames and its own `$sync`. So a handler that catches the abort observes the
callee's last write, and instantiating `m := ExceptT E (StateM Σ_io)` with
`$sync := set` gives `Σ_io → Except E (Ret × Σ_io) × Σ_io`. The default keeps
normal-return contracts and function values unchanged. An abortable call
through a *function value* with `inout` parameters is refused rather than
modelled with roll-back.

### Manuscript

- §4.3: `PostCall` split into `PostCall_ok` and `PostCall_abort` (keep-writes,
  stationary condition, whole-place condition, refinements at every abortable
  checkpoint, facts havocked, `out` uninitialized, non-stationary needs a
  linear row or an internal handler, uniform across abortable effects), plus
  a "current implementation" note.
- §4.2 resumption frame condition and §5.2 worked derivation: the frame
  condition is scoped to owned obligations and handler states; `inout tx:
  WriteTxn` may cross `?`.
- §9.2 Tier 1 (`<ch-8-2>`): the `$sync` encoding and its `ExceptT E (StateM
  Σ_io)` reading.

## Evidence

Tests in `tests/inout_abort.rs`:

- evaluator and Cranelift JIT agree that the caller sees the callee's last
  write: one frame, two fallible frames, a projected struct field (sibling
  field untouched), a sibling argument that aborts before entry (no write),
  a handler inside the callee, a non-`Fail` abortable effect, checked
  arithmetic overflow, and a linear `inout` whose owner handles the abort;
- an AOT executable (`vesper build`) exits with the same value;
- the extracted Lean module, compiled by Lean 4.35.0, evaluates the
  extracted `main` to the evaluator's value, and the `ExceptT Int64 (StateM
  Int64)` model of `bump`/`outer` keeps the write on `.error`;
- checker accept/reject cases for each rule above.

Existing tests that asserted the old E0402/E0410 inout rejections were
updated to assert acceptance (`checked_arithmetic`, `diagnostic_causes`,
`ir_diagnostic_causes`, `seq`). The Chroma fallback test now uses a fixture
that still fails (`abort_with_live_txn.ves`): `abort_across_inout.ves`, the
manuscript broker showcase, now type-checks.

## Stage 1 guard

No edits to `src/verified_codegen/**`, `src/ir.rs`, `ir_opt::optimize`,
`lean/**`, `bootstrap/**` or `packages/vesper-compiler/**`. The checker
change only accepts more programs; it does not change how an accepted program
lowers, so the corpus artifacts cannot change. See the PR for the
bootstrap-stage1 workflow run on this branch.

## Remaining gaps

- The self-hosted Stage 1 checker (`packages/vesper-compiler`,
  `c_abort_frame`/`c_abort_loans` in `check_modes.ves`) still rejects
  `inout` across abortable effects. This lane may not edit it. It is stricter
  than the seed, never more permissive; it needs the same relaxation in a
  Stage 1 checker lane.
- `return`/`break` across an active `inout` loan (`src/loops/check.rs`
  `jump_loans`) still raise E0402. They are not abortable effects and were
  not part of the decision.
- Handler operation clauses that write a borrowed operation argument before
  an abort stay a Lean extraction refusal (`guard_abort`).
- See also "What is left" under the native lowering below.

# Part 2: native lowering (steps 5-6)

## What changed (`src/native.rs`, `src/native_export.rs`, `src/native_session.rs`)

Before this PR, every call to a function whose row holds only `Fail`
instances (`fail_only`) went like this. The caller allocated a failure record
on the stack and zeroed every word of it. The callee stored a tag and the
payload only when it failed. After the call, the caller loaded the tag word
and compared it once per `Fail` instance. So a successful call cost
`record words` stores, one load and `n` compares. Small aggregate results,
such as an `Option[u64]` or a small `Result`, went through a result pointer
into a caller buffer that was also zeroed, and the callee copied the value
into it.

`NativeAbi` now plans two internal conventions per compilation:

- **Register status.** A `fail_only` function returns its status as a last
  `i64` value: 0 means success, and `k` means it raised the `k`-th `Fail` of
  its row. The hidden pointer now points to caller storage for the error
  payload only, at the same offset 8 as in a record. The callee writes that
  storage only when it fails, so the caller no longer zeroes it.
  - On a successful call the caller does one register test, and the rest of
    the success path stores nothing.
  - With several `Fail` instances, a single `status != 0` test guards the
    per-instance dispatch.
- **Register result.** A function with an empty row whose result is a copy
  aggregate of one or two words returns those words in `i64` registers. A
  small `Result`, `Option` or pair counts if it holds no function value and
  is not a `fip`/`Box::new_in_caller` slot.
  - The callee builds the value in a local stack slot and returns its words.
  - The caller stores them into uninitialized storage.
  - Two `i64` returns fit `rax`/`rdx` on x86-64 and `x0`/`x1` on AArch64.
    On Windows x64, Cranelift's fastcall returns them its own way. Both
    sides are Cranelift code, so the result is correct either way.

Both conventions have the same scope:

- Only optimized builds use them (`optimize = true`: the JIT, `vesper build`,
  and `vesper build --lib`, which the ports use). `--no-opt` builds and the
  Stage 1 corpus, which is built unoptimized, emit exactly what they did
  before.
- Boundary symbols keep the documented ABI. These are the program entry, the
  library exports (export ABI v1/v2/v3 and lane 4's glue), and
  capability-ABI functions (`vesper.native.capability-abi/1`). A foreign
  caller still passes a zeroed failure record and reads its status word. An
  exported function that calls an internal `fail_only` function converts the
  status register into its own record write once.
- Session (suspending) entries and `native_batch` keep the default ABI
  (`DEFAULT_ABI`).

The verified backend's own Fail lowering is untouched.

## Tests

`tests/native_fail_status.rs` (11 tests). The evaluator, the unoptimized JIT
(record ABI) and the optimized JIT (register ABI) must agree on every case.
The callees are recursive, so the inliner keeps the calls.

- One `Fail` on success and failure paths through several frames.
- Several `Fail`s in one row (`[Fail[ArithOverflow], Fail[RuntimeError]]`),
  where each instance is picked by its status.
- Payloads of three words (struct), two words (enum) and one byte (`u8`).
- An aggregate result with a status, and an `f64` result with a status.
- Nested handlers, and `Fail` inside a `for` loop, both caught per iteration
  and escaping the loop.
- A status call inside a status callee.
- Keep-writes across status frames (`inout`).
- Primitive failures: checked overflow and division by zero.
- Small aggregate results in registers (`Option[i64]`, a two-field struct).
- `status_abi_is_used...` and `small_aggregate_results_return_in_registers`
  check the emitted signatures: two returns in the optimized object, one at
  the entry, one everywhere in unoptimized objects.
- `exports_keep_the_failure_record_at_the_boundary` (Unix) links a library
  object with a C caller. The caller zeroes a record, and reads the status
  word and payload of two exports. One export calls the status-ABI internal
  function and the other export, which uses the record ABI.

Local runs with this change, all green: `backends`, `checked_arithmetic`,
`floats`, `freestanding`, `inout_abort`, `integer_widths`, `lockstep`,
`native_bytes`, `native_export_v2`, `native_export_v3`, `native_inline`,
`native_spans`, `native_suspend_abi`, `numeric`, `regions`, `seq`,
`strings`, `tiled`, `vesper_std`, `native_coverage_cli`,
`capability_verifiers`, `native_verifiers_cli`, `fip`, `fuzz_cli` and
`doc_cli`, plus `cargo +1.100.0 clippy --all-targets -p vesper`. The BBR and
Aura port test suites (`--features vesper-port`) pass when built by this
compiler, including with the local Fail rewrites described below.

## Port measurements

Setup: 4 vCPU Intel Xeon @ 2.10 GHz cloud container, Linux, Rust 1.97
`--release`. Ports were compiled through `VESPER_PORT_COMPILER` by two release
`vesper` binaries: **base** (`main` at `2736dcd`, before this PR and #943)
and **new** (this branch). Wall-clock ABBA numbers in this container are
noisy, with per-epoch ratios spreading by about ±15%. Callgrind instruction
counts are exact, so they are given too.

### What `Result` returns cost today (step 5)

No port has a `Fail` row today. E0402 forced every mutating entry point to
return its error as a value. So the register-status convention alone cannot
move any port's ratio; only the register-result convention reaches today's
code. To measure the cost of the `Result` shape, one hot entry point per
port was rewritten to raise `Fail` across `inout`, as a local experiment
that was not pushed.

- **BBR, `bbrv3_record_send`** (called on every send, 30,146 calls in the
  callgrind run).
  - The rewrite takes `inout b: Bbrv3` and returns `PacketDeliveryState eff
    [Fail[BbrFault]]` instead of `Result[PacketDeliveryState, BbrFault]`.
    The generated Rust binding still returns `Result<_, BbrFault>`, so
    `src/bbr/forward.rs` did not change. All BBR port tests pass, including
    the rejection tests, because the checks still precede every write.
  - Vesper body: 258 → 157 instructions per call (−39%).
  - The binding wrapper, inclusive: 1,678 → 1,581 instructions per call.
    Building the `Result` costs about 100 instructions a call.
  - The rest of the wrapper is the export ABI copying `Bbrv3` in and out:
    `Abi::get` plus `Abi::put` take 35% of the whole run.
- **Aura, the patch loop of `do_recorder_apply`.**
  - `apply_patch` raises `Fail[BackendFault]` across `inout s: RState`, and
    the loop catches it in a local handler. `apply_patch` is not inlined
    (its row differs from its caller's), so it takes the register-status
    ABI.
  - −4,600 instructions out of 780 M. Aura's patches are few per commit,
    and its time goes to copying and rebuilding recorder state.
- **Quarry.** Not rewritten. Its entry points carry a sticky error on the
  `Q` handle (`q.err`, `q.msg`, about 100 `err != 0` checks), not
  `Result`-valued `inout` functions. Keep-writes is exactly what the sticky
  error needs, so `fail` could raise. But every function on the path would
  then need a `Fail` row, and about a third of its time is `malloc`, `free` and `realloc`.

### Ratios before and after (step 6)

| Port, workload | Rust vs base | Rust vs new | new / base | Instructions base → new |
| --- | --- | --- | --- | --- |
| BBR ack/loss, 12 epochs × 200k steps | 11.4x to 12.4x (95% interval) | 9.2x to 12.1x | 0.983 median (quartiles 0.973 to 1.017, 16 epochs) | 207.19 M → 206.44 M (−0.36%); Rust 26.36 M |
| BBR with the `record_send` Fail rewrite | | | 0.995 vs new (noise) | 206.44 M → 203.52 M (−1.4%) |
| Aura one-text, 15 epochs × 200 ops | 2.03x | 2.11x | noise | 780.117 M → 780.117 M (+12) |
| Aura reorder | 2.41x | 2.43x | noise | (same run) |
| Aura encode-audit | 1.71x | 1.75x | noise | (same run) |
| Aura with the patch-loop Fail rewrite | | | | 780.117 M → 780.113 M |
| Quarry parse+plan (PR #930 branch `92fe7d4`) | 3.29x (median) | 3.17x | 1.003 (+25 bp) | 54.860 M → 54.860 M (−271); Rust 19.47 M |

BBR's measured instruction count is 7.8x Rust's. Its wall-clock ratio is
higher, because the port's binding moves far more memory per step.

The Aura and Quarry ratios are well below their P1 reports (3.8x to 5.2x
and 4.3x to 4.9x). That gain came from lanes 1 to 4 (inlining, drops, bounds
hoisting, ABI v3) and was already on `main` in the base compiler. Between
base and new, every port is within noise: the instruction counts move by
less than 0.4%.

The conclusion for the ports is that the Fail ABI is not their bottleneck.
Allowing `Fail` across `inout` removes a shape restriction and about 100
instructions per `Result` built at an export. The big remaining costs are
the export ABI's struct copies in BBR (adopting `// @export abi 3` is the
port follow-up named in lane 4) and allocation in Quarry and Aura.

### E0402 cases

- **Changed, now expressible with `Fail`.**
  - BBR: 7 `inout` entry points return `Result[.., BbrFault]` because of
    E0402: `bbrv3_record_send`, `ack_check`, `bbrv3_on_packet_lost`,
    `bbrv3_on_ecn_ce`, `bbrv3_on_persistent_congestion`,
    `bbrv3_on_app_limited` and `bbrv3_on_spurious_loss_detected`.
  - Aura: 18 `inout` functions return `Option[BackendFault]`. Among them are
    `apply_patch`, the nine `patch_*` functions, `allocate`, `record`,
    `validate`, `validate_final`, `do_recorder_apply` and
    `do_recorder_set_user_state`.
  - Quarry: the sticky `Q.err` protocol, which exists because of E0402 (its
    `lib.ves` says so).
  - The ports keep their current shape. Rewriting them is port work, and by
    the numbers above it buys little until the copies go.
- **Remaining E0402.** These are:
  - moved-out holes in an `inout` place;
  - owned linear bindings whose frame would unwind (unchanged by design);
  - `return`/`break` across an active `inout` loan;
  - the self-hosted Stage 1 checker, which still rejects every case above
    (see Part 1).

## Stage 1 guard

- No edits to `src/verified_codegen/**`, `src/ir.rs`, `ir_opt::optimize`,
  `lean/**`, `bootstrap/**` or `packages/vesper-compiler/**`.
- In `src/native.rs` only the Fail-lowering hunks changed: the signature,
  call site, return and failure exit, and the planning. In
  `src/native_export.rs` only `compile_exports` changed.
- Both conventions are off unless `optimize` is on, and `bootstrap/stage1.rs`
  builds the corpus with `optimize` off on both backends. So no Cranelift or
  verified artifact digest can change.
- The bootstrap-stage1 workflow run on this branch is linked on the PR.

## What is left

- The native backend refuses a handler body whose final expression is an
  untyped `Option::None`, even with an annotated `let`. The error is "native
  layout is not yet defined for _". This is pre-existing (the base compiler
  fails the same way). The Aura experiment worked around it with a typed
  local.
- Register-status functions with a two-word aggregate result still use a
  result pointer. Three return values would exceed x86-64's two return
  registers, and Cranelift would then use a return area anyway.
- Executable objects (`vesper build`) use both conventions, but they keep
  every call, because lane 1 does not inline in objects. A status callee
  there is a real call and benefits most.
