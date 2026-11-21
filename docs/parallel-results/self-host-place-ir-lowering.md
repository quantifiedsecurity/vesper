# Self-hosting SH3 lane 13: Place IR lowering

Claim #1252. Prompt: `docs/agent-prompts/self-host-sh3/13-place-ir-lowering.md`.
Stacks on SH3 lane 1's first PR (#1259, head `59e4e02`, itself on #1232) and
on core typing part 2 (#1237, head `f0ded8b`), merged together on this
branch; `origin/main` was `7636681` when the lane started. Rust stays the
default lowering; nothing here changes a default build or deletes Rust
code.

The lane lands in parts. This report grows with each one.

| Part | What |
| --- | --- |
| 1 (this PR) | `lower_world` and `thread_tokens` for every expression kind except closures, handlers and payload operations; the differential |
| 2 | closures and handlers (nested functions, captures, handler clauses) |
| 3 | sequence, tiled, vector and `Box` operations (`seq::resolve` and the other resolvers, with a structured payload kind) |
| 4 | the swap: the hybrid `ir_lower` module through lane 1's `lower` hook, the full suite on the hybrid build, the scoreboard row |

## Part 1

### What changed

| Area | Files |
| --- | --- |
| Package | `packages/vesper-place-ir-lower` (`@vesper/place-ir-lower`): `src/ast.ves` (the input tables), `src/lower.ves` (the lowering, `thread_tokens` and the program rows), `src/lib.ves` (entries), README |
| Input encoder | `tests/common/ir_lower_words.rs` |
| Test | `tests/self_host_ir_lower.rs` |
| Bazel data and timeout | `BUILD.bazel` |

No Rust source changed.

**The port.** `src/lower.ves` follows `ir::Lower` statement by statement:
`emit` (span types through `numeric::default_type`, the `Read` field-type
fallback, the fallible test), `place` and `argument`, `rename_pattern`
(fresh `$slot{n}_{x}` names), literals, names and unit constructors,
tuples, struct literals, fields and projections, indexing, unary and
binary operators (with `&&`/`||` as an `if`, and borrowed string
comparisons), assignment, blocks with `let` and the `Never` cut-off, calls
(world functions and methods, effect operations through capability
labels, constructors, the wrapping, bitwise, conversion, rotation and
float intrinsics, indirect calls with the callee type's modes), `?`, `if`,
`match`, and the loop lowering of `src/loops/ir.rs` (`loop`, `for` over
ranges with the step block for `continue`, `break`, `continue`,
`return`). `thread_tokens` and `state_action` run in `flush`, which writes
each function's rows in block order.

One function is lowered at a time into working tables, because lowering
appends to blocks out of order: blocks, instructions tagged with their
block over one scratch operand pool, the `names` scopes as an undo trail
on a symbol-indexed table, and `locals` as a symbol-indexed table. Value
ids, slot names and block numbers come from the same counters in the same
order as in Rust, so the output is `Debug`-equal, not just equivalent.
Header maps that Rust keeps in `BTreeMap`s (`locals`, `bounds`) are sorted
by the names' bytes.

**The input.** check-data's names, types and world (which carry
`world.types`, `partial_ops` and `overflow_ops`), then the function table,
effect operation signatures and every body as post-order expression and
pattern rows (format in `tests/common/ir_lower_words.rs`). Two things are
read off Rust in this part and written into the input per call:

- which calls and index expressions `seq::resolve`,
  `tiled::resolve_ir_call`, `VectorOp::resolve` or the `Box` builtins route
  to a payload operation (the spans of the Rust lowering's `Seq`, `Tiled`,
  `Vector` and `Fip` instructions). Those are written as foreign
  expressions. Closures and handlers are written in full (parameters,
  bodies, clauses, moves), but the port does not lower them yet, so it
  skips a function that reaches either;
- the intrinsic name class of a call of a plain name (the name tables of
  `vesper-core::numeric` and `capability_effects`).

Part 2 lowers the closures and handlers; part 3 removes the foreign
expressions.

### The differential

Corpus: lane 4's, 290 worlds on this base (Cargo, Linux). Every function
the port lowers is compared with Rust's `FunctionIr` (`{:#?}` text), and
`ir_check::validate` must report the same diagnostics on the port's
functions and on Rust's same functions.

| Worlds | Functions lowered | Functions skipped | Instructions compared | Functions differing | IR check differing |
| --: | --: | --: | --: | --: | --: |
| 290 | 10,426 | 5,002 | 178,614 | 0 | 0 |

The skipped functions, by the first construct the port does not lower
yet: 161 reach a closure or handler, 4,841 a payload operation. The
unoptimized build runs the whole differential in 403 s (Bazel's `long`
timeout is 900 s).

The harness can fail: dropping `overflow_ops` from the fallible test makes
`tests/fixtures/freestanding/checked_sum.ves` and
`tests/fixtures/integer_widths/kinds.ves` (and more) differ at once. The
test also asserts a coverage floor (1,000 functions), so a port that
skipped everything cannot pass.

### Numbers

Over the functions the port lowers, every corpus world (the other
functions keep their signatures and lose their bodies, so both sides lower
the same set). Port-compiler test build, ABBA, best of six, one machine.

| Step | Time | vs `ir::lower_world` |
| --- | --: | --: |
| `ir::lower_world` (Rust) | 290 ms | 1.00x |
| Vesper entry (read the input, lower, write words) | 136 ms | 0.47x |
| Vesper read alone | 23 ms | 0.08x |
| **Vesper lowering alone** (entry minus read) | **114 ms** | **0.39x** |
| Encode (Rust, `lower_input`) | 210 ms | 0.72x |
| Decode (Rust, `decode_program`) | 123 ms | 0.42x |
| Whole swap (encode + entry + decode) | 469 ms | 1.62x |

The lowering itself runs in 0.39x of Rust's time: Rust clones a `String`
for every name, `Place` and alias map entry and allocates a `Type` tree per
instruction, where the port moves `u32` symbols and hash-consed type ids.
The word encoding costs more than the lowering, as lane 1 measured; ABI v4
input and lane 1's tables are how that goes away (part 4 measures the
hybrid build).

Callgrind counts (lowering alone and the whole corpus) follow in a later
commit on this PR.

### Compiler gaps

1. **Re-initializing a moved owned local inside a loop is refused.**
   Rotating two buffers (`let t = a; a = b; b = t;`) in a loop is E0303
   (`use of moved place`) and E0312 (`moved in a previous iteration`), even
   though both are assigned again before the next iteration. `swap(inout
   a, inout b)` works, and the merge sort uses it. Repro:

   ```vesper
   fn main() -> u64 {
       let mut a: ChunkSeq[u64] = ChunkSeq::new[u64]();
       let mut b: ChunkSeq[u64] = ChunkSeq::new[u64]();
       for i in 0..3 {
           let t = a;
           a = b;
           b = t;
       }
       a.len()
   }
   ```

2. **`effect` cannot be a method or local name** (E0002). Known: SH3 lane 5
   (#1244) makes it contextual. The port names the accessor `effect_decl`.

3. **No imported constants on this base.** The tag, opcode and kind
   constants repeat in `ast.ves` and `lower.ves` and mirror
   `@vesper/place-ir`'s. #1239 (value enums and importable constants) has
   since merged; the port switches when this branch merges `main`.

### Stage 1 guard

`src/ir.rs`, `src/loops/ir.rs`, `src/seq/ir.rs`, `ir_opt::optimize`,
`src/verified_codegen/**`, `lean/**`, `bootstrap/**` and
`packages/vesper-compiler/**` are unchanged (this part changes no Rust
source at all), and nothing installs a `lower` hook. The bootstrap-stage1
workflow runs on this branch; its digests are in the PR.

### What is left

Parts 2 to 4 above. The skipped functions are the coverage still to come.
