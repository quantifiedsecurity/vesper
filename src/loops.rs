//! Loops and early exits (manuscript ch. 2.2 `LoopExpr`, `ForExpr`,
//! `BreakExpr`, `ReturnExpr` and `LetStmt` with `mut`; port wave P0 lane 4).
//!
//! * `loop { body }` repeats its body until a `break`. Its type is the join
//!   of its `break` values, or `Never` when no `break` leaves it.
//! * `for pat in a..b { body }` and `for pat in a..=b { body }` iterate an
//!   integer interval in ascending order. The bounds are evaluated once,
//!   before the first iteration; the increment never overflows. Its type is
//!   `Unit`, and a `break` inside it takes no value.
//! * `break` and `break value` leave the innermost loop of the enclosing
//!   function, closure or handled body.
//! * `return` and `return value` leave the enclosing function or closure,
//!   including from inside a handled body (which abandons the handler).
//! * `let mut` declares an assignable binding.
//!
//! The checker (`loops/check.rs`) types them and enforces the mode rules: a
//! value moved in a loop body must not be live across the back edge, and a
//! `break` or `return` must not drop a live `linear` value. The Place IR
//! (`loops/ir.rs`) lowers them to a cyclic CFG whose loop headers start with
//! [`crate::ir::Op::Loop`], so backends without loop support (the verified
//! backend, Wasm, eBPF, batch JIT) refuse them through their typed
//! unsupported-operation paths. The evaluator and the native Cranelift
//! backend run the CFG directly in constant stack.

/// The Lean extractor's typed refusal for loops and early exits.
pub const LEAN_REFUSAL: &str = "Lean extraction does not model loop, for, break or return yet; \
    write the iteration as a tail-recursive function";

/// The Wasm backend's typed refusal ([`crate::wasm_backend::Unsupported::Operation`]).
pub const WASM_REFUSAL: &str = "loop, for, break and return are not lowered to Wasm yet; \
    write the iteration as a tail-recursive function";

/// The marker a loop-header block starts with, and the non-local return
/// from a handled body (see [`crate::ir::Op::Loop`] and
/// [`crate::ir::Op::Exit`]).
pub const LOOP_MARKER: &str = "loop";
