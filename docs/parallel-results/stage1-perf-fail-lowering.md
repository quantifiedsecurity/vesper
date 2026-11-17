# Stage 1 perf lane 5: Fail across stationary `inout` (keep-writes)

Claim: #935. Prompt: `docs/agent-prompts/stage1-perf/05-fail-lowering.md`.
Baseline: `332fbdb` (main after plan #938).

This report covers the **language change** (prompt steps 1-4). The native
register-status Fail lowering (steps 5-6) waits for lane 1 (#931, inlining)
and lane 4 (#934 / PR #940, export ABI v3), and will follow in a separate PR
that closes #935.

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
- Steps 5-6 (port measurements and register-status native lowering) follow
  once lanes 1 and 4 merge.
