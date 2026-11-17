# Port wave P1 lane 13: vesper-aura

Claim: [#910](https://github.com/robot-head/vesper/issues/910). The keyed view
runtime of `crates/vesper-aura` (validation, planning, the session state
machine, the recording backend and the SimAura queries) now runs as the Vesper
Loom package `@vesper/aura` in `crates/vesper-aura/vesper/`, linked through
the export ABI. The Rust implementation is deleted. `vesper-aura-gtk` is
unchanged and passes against the port.

## API inventory (re-measured before porting)

Public types, all kept with the same names, fields and derives:

- view.rs: `Key`, `Kind`, `Property`, `Value`, `Event`, `HandlerKey`, `Node`
  (`new`, `property`, `on`, `children`), `ViewTree::new`, `Limits` (+
  `Default`), `ViewError`.
- identity.rs: `UiThread::claim_current`, `WrongThread`, `WidgetId`,
  `SlotId`, `SubscriptionId`.
- plan.rs: `Patch`, `Transaction` (`patches`, `base_revision`,
  `next_revision`).
- recording.rs: `BackendError`, `RecordedEvent`, `WidgetSnapshot`,
  `RecordingSnapshot`, `RecordingBackend` (`new`, `snapshot`,
  `reject_next_at`, `set_user_state`, `widget`, `slot`, `dispatch`, `Backend`
  impl).
- session.rs: `Backend` (trait), `Session<B>` (`new`, `revision`, `limits`,
  `widget`, `slot`, `subscription`, `backend`, `backend_mut`, `prepare`,
  `commit`, `request_close`, `resolve_close`, `shutdown`), `PreparedUpdate`
  (`transaction`, `tree`, `into_tree`), `CloseDecision`, and the error and
  report types (`InitError`/`InitFailure`, `PrepareError`/`PrepareFailure`,
  `CommitError`/`CommitFailure`/`CommitReport`, `CloseAction`,
  `CloseOutcome`, `CloseRequestError`, `CloseError`/`CloseFailure`,
  `ShutdownError`/`ShutdownFailure`).
- sim_aura.rs: `NativeUiBackend`, `layout_flags`, `NodeOp`
  (`property_slot`), `SlotValue`, `AuraSlotTable`, `AuditTarget`,
  `WcagViolation`, `WcagReport::is_compliant`, `SimAuraError`,
  `SimulatedEvent`, `SimAura` (`new`, `with_wcag_gate`, `backend_kind`,
  `snapshot`, `focused`, `dispatched_events`, `audit_wcag`,
  `focusable_order`, `focus_next`, `focus_prev`, `trigger_event`, `Backend`
  impl), `encode_view_tree`, `audit_view_tree`.

Tests: 27 integration tests (`tests/reconcile.rs` 11, `tests/validation_close.rs`
10, `tests/sim_aura.rs` 6), one `#[cfg(test)]` unit test in session.rs, and 9
doctests (the `compile_fail` `Send`/`Sync` checks and the examples).

Must stay Rust: the `Backend` trait and every generic over it, the `UiThread`
token and its `ThreadId` check, the `!Send`/`!Sync` markers, and owned public
values (`ViewTree`, `Value`, `String`s) handed back to callers.

## What was ported

| Rust module | Vesper module(s) | What moved |
| --- | --- | --- |
| view.rs | `view.ves`, `util.ves` | Tree validation: keys, parents, reachability, cycles, per-kind property/event rules, every `Limits` budget, teardown fit. |
| identity.rs | `session.ves` | Widget/slot/subscription serial allocation and exhaustion. The process-wide session counter stays in Rust (atomic). |
| plan.rs | `plan.ves` | Keyed diff: create, destroy, reorder (`MoveChild`), property and handler changes, disconnect-before-reconnect order, patch budget. |
| session.rs | `session.ves` | Reconciler state: revision, reservations, commit checks (foreign/stale/closed), adopt, close state machine. |
| recording.rs | `recording.ves`, `apply.ves`, `widgets.ves`, `slots.ves` | Staged apply with all-or-nothing publish, every `BackendError::Invalid` check, injected rejection, user state, dispatch, last-batch recording. |
| sim_aura.rs | `sim.ves` | Focus order, wrap-around, event dispatch decisions, WCAG 4.1.2 audit of trees and mounted state, `NodeOp` topology encoding. |

`lib.ves` holds every type and the 40 exported functions (the export ABI
exports only root-module functions, and types defined in submodules get
mangled binding names). `codes.ves` lists the numeric codes shared with
`src/ported/codec.rs`. 3,966 lines of Vesper replace 1,814 lines of Rust;
the Rust side keeps 1,357 lines of wrapper and codec plus the 1,770-line
generated binding (`port/binding.rs`, 129 `unsafe` occurrences, all in
generated code; the crate's own code has none).

## What stays Rust and why

- `src/ported/session.rs`: `Session<B>` is generic over `Backend`, and
  `dyn Backend` inside Vesper waits for F7b, so Vesper returns a recorded plan
  (`Prepared`) and the wrapper calls `backend.apply` and then `adopt`.
- `src/ported/recording.rs`: keeps the thread token and a copied
  `RecordingSnapshot`, refreshed after each change, so `snapshot`, `widget`
  and `slot` can still lend references.
- `src/ported/sim_aura.rs`: the focused widget and the dispatched-event log
  are plain owned state; every decision comes from Vesper.
- `src/ported/codec.rs`: flattens `ViewTree` and `Transaction` into words plus
  one text buffer, and rebuilds `Patch`, `Value` and snapshots. Slot values in
  a plan point at the input node and property; Rust clones the value from the
  caller's tree, so strings are never copied through the port twice.
- `identity.rs` keeps `UiThread` (a `ThreadId`) and the atomic session
  counter.

Language constraints that shaped the port: `Fail` cannot be raised while an
`inout` parameter is live (E0402), so every error is a value
(`Option[ViewFault]`, `Option[BackendFault]`); native code does not yet free a
dropped local `ChunkSeq`, so every allocation lives in a handle that is
returned or passed `inout` (a counting test checks this, below); `vesper fmt`
caps a file at 8,192 tokens, so the port is a package of eleven modules.

## Unit tests

The one unit test, `exhausted_ids_and_revision_fail_without_mounted_changes`,
forced private counters. It is translated into `src/ported/session.rs` and
forces them through an exported test hook (`reconciler_force_counters`). A new
unit test, `port_frees_every_allocation_it_makes`, runs mount, update,
dispatch, rejection, close and shutdown cycles on `RecordingBackend` and on a
WCAG-gated `SimAura` and checks that the port's live-allocation count returns
to its start. The two unit tests share a lock because the count is
process-wide.

All 28 tests and the 9 doctests passed against both builds before deletion,
and pass against the port after it.

## Divergence checks

`build.rs` applies five edits under the `vesper-port-divergent` feature, one
per ported module, and `BUILD.bazel` builds each as its own
`vesper_port_object`. `scripts/test-port-harness.sh` asserts that each one
fails a named test while another test in the same binary still passes. Each
edit was also built alone to check that it is the one the named test catches.

| Module | Edit | Caught by |
| --- | --- | --- |
| view.rs | one child over the limit is accepted | `budgets_fail_before_backend_and_accepted_tree_remains_unmountable` |
| plan.rs | `MoveChild` reports its positions swapped | `keyed_insert_remove_reorder_and_stale_handle_rejection` |
| session.rs | a stale plan passes the revision check | `stale_and_foreign_plans_retain_input_and_do_not_call_backend` |
| recording.rs | an injected rejection fires one patch late | `initial_mount_rejection_never_publishes_widgets_slots_or_registrations` |
| sim_aura.rs | `Input` drops out of the focus order | `sim_aura_focus_events_atomic_wcag_gate_and_close_decision` |

## Benchmarks

None. Aura is not on the lane rules' hot-path list and the crate had no
benchmark, so no ABBA comparison was required, and no performance numbers are
claimed.

## Platforms run

- Linux (cloud container), Cargo, Rust 1.97: `cargo test -p vesper-aura`, the
  vesper-aura section of `scripts/test-port-harness.sh` (five divergent
  builds), and `cargo +1.100.0 clippy --all-targets -p vesper-aura -p
  vesper-aura-gtk -- -D warnings`, with and without `vesper-port-divergent`.
- Linux, `vesper-aura-gtk` with `--features gtk` under
  `dbus-run-session -- xvfb-run -a` (`GSK_RENDERER=cairo`), against both
  builds before deletion and against the port after it.
- Bazel (BuildBuddy), macOS AArch64 and Windows x86_64 MSVC, and the
  `aura-gtk.yml` workflow: through CI on the PR.

## Rust code deleted

Yes. `src/reference/` (the frozen Rust implementation, 1,814 lines) and the
`vesper-port` feature are gone; the port is the only build, and
`vesper-port-divergent` is the only feature.
