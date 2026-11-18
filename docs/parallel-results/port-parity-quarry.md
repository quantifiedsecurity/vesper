# Port parity lane 7: Quarry planner (#955)

Base: `main` at cee8b1a (the port parity plan, #958). The Quarry planner port
now uses the merged Stage 1 work where it applies: local sequences (#945),
block copies (#942, #947) and export ABI v3 (#940). Its Rust-side codec is
also leaner. On `examples/planner_abba.rs` it runs **1.96x** Rust's
instructions, down from 2.54x (-23% instructions). By wall clock it is
**2.2x to 2.4x** slower than Rust, down from 2.9x to 3.1x. It does not meet
the parity bar, so the Rust build stays the default and no Rust code is
deleted (owner's rule).

Every port test passes with and without `--features vesper-port`, including
the leak test `tests/port_memory.rs`. The divergent build's edits still apply
to the source (each pattern occurs once). The public API is unchanged, and so
is the default (Rust) build: `src/port.rs` is compiled only under
`vesper-port`.

## Setup

- Cloud container, 4 vCPU Intel Xeon @ 2.10 GHz, Linux 6.18, x86-64.
- `rustc 1.97.0`, `--release` with debug info for profiling, `optimize = true`
  (the port harness default).
- The `vesper` compiler is the release binary built from cee8b1a
  (`VESPER_PORT_COMPILER`).
- Instructions: `callgrind` of `planner_abba window 20` (one warm-up pass plus
  20 passes over the 21 golden cases, 924 entry calls). The Rust arm is
  **19.47 M** instructions.
- Wall clock: `planner_abba driver RUST PORT`, 9 ABBA epochs of 200 passes,
  with both builds run in one session. The median is the median of the
  epochs' B/A ratios. The interval is the 2nd to 8th order statistic of the
  9 epochs, a distribution-free 96% interval for the median. All arms ran
  back to back in one run. `main` and the final arm were run twice, at the
  start and at the end of that run.

## Ratio before and after each lever

| Step | Change | Instructions | Δ | Instr. ratio | ABBA median (96% interval) |
| --- | --- | ---: | ---: | ---: | --- |
| main | cee8b1a | 49.38 M | | 2.54x | 2.87x (2.77-3.14); repeat 3.13x (2.80-3.42) |
| 1 | `aux*`, `bits`, unused `flags` moved out of `Q` into locals | 48.13 M | -2.5% | 2.47x | 2.86x (2.36-3.06) |
| 2 | executor/planner state (`rowset`, `dtabs`, `decls`, `stats`, `vals`, `counts`) moved out of `Q` into a `D` that only `execute`, `set_rows` and `access_paths` build | 46.57 M | -3.2% | 2.39x | 2.97x (2.51-3.61) |
| 3 | capacity hints for the growing sequences (`words`, `pool`, `ex`, `toks`, `plan`) | 45.82 M | -1.6% | 2.35x | 2.76x (2.69-3.06) |
| 4 | Rust side: one allocation per decoded string (no `to_vec` then lossy copy), sized encoders, `[u64; 16]` limits, error message built only on error | 41.81 M | -8.7% | 2.15x | 2.54x (1.97-2.92) |
| 5 | expression and slot words copied with one `extend` of an `ex` view instead of word-by-word pushes | 41.16 M | -1.6% | 2.11x | 2.49x (2.31-2.87) |
| 6 | lexer: already-lowercase identifiers are not copied, the word scan and per-token limit check are written inline, and limits load with one `extend` | 39.61 M | -3.8% | 2.03x | 2.48x (2.40-2.53) |
| 7 | `// @export abi 3`: `prepare` takes the catalog as `in Slice[CTab]` and `in Slice[CCol]` mirrors, copied in one block each | 38.23 M | -3.5% | 1.96x | 2.19x (2.10-2.68); repeat 2.42x (2.19-2.59) |

Instruction counts are deterministic. The wall-clock epochs vary by about
±15% on this host, so the per-step wall medians are noisy. The total is
clear in both measures: main is 2.9x to 3.1x and the final port is 2.2x to
2.4x.

The plan's starting figure was 3.17x to 3.3x (#946). This run measured main
at 2.87x to 3.13x on the same benchmark, on a different host.

A negative result. Step 3 first sized every sequence of `Q`, including the
small ones (`lim`, `sels`, `binds`, `tabs`, `cols`, `params`, `bcols`,
`aliases`, `rows`, `raw`, `links`). That cost more than it saved: 47.01 M
against 46.57 M. Grows fell from 18,685 to 478, but each up-front data
`malloc` is a larger, slower allocation, and unused capacity is still
allocated and freed. Only the large, predictable sequences are sized now.

## Encode/decode share

The port's Rust side (`src/port.rs`) encodes the inputs, decodes the result
streams and builds the crate's types. It is measured as `port::prepare` plus
`port::optimize`, minus their binding calls (the Vesper code and `Q_drop`):

| | Rust side of the wrappers | Share of the run |
| --- | ---: | ---: |
| main | 9.92 M | 20.1% |
| final | 6.59 M | 17.2% |

This share includes building the `PreparedQuery` (strings, boxed
expressions, the catalog clone), which the Rust build also does. In the Rust
build, `plan::prepare` is 4.0 M of its 19.47 M. What remains of the codec
proper is the plan stream: `Enc::plan` plus `Dec::plan` in `optimize`, and
`Dec::plan` plus the column/projection decode in `prepare`. That is about
2.5 M (6.5%). The plan is a `Box` tree with strings on the Rust side, so a
mirrored node slice would only move the encoding work into Rust. It would
not remove it. The catalog is the input that fits mirrors, and step 7
converted it.

## What is left (profile of the final port)

Top costs in `window 20` (38.23 M). These are exclusive costs, read from a
build with named local symbols (a local, uncommitted compiler patch that
names `vesper_function_N` after its function):

| Cost | Instructions | Share | Port or compiler |
| --- | ---: | ---: | --- |
| glibc `malloc`/`free` and helpers (`_int_free`, `_int_malloc`, `malloc`, `free`, `malloc_consolidate`, `unlink_chunk`) | 14.6 M | 38% | Both builds pay for the Rust types. The Vesper runtime's share is below. |
| `vesper_seq_new` + `vesper_seq_free` + `vesper_seq_grow` inclusive (header `malloc`, data `malloc`, thread-local tracking map) | 8.08 M | 21.1% | compiler gap 1 (main: 13.27 M, 26.9%) |
| `lex::lex` + `lex::byte` | 2.58 M | 6.7% | mostly compiler gaps 5 and 6 |
| `util::identifier` (validate + lowercase) | 2.01 M | 5.2% | compiler gap 6 (per-byte `try_get` through `inout q`) |
| `plan::prepare_q` | 1.18 M | 3.1% | port code |
| `check::find_column`, `util::same`, `util::is_lit` | 2.34 M | 6.1% | linear name lookups over short byte ranges. The port could hash them, but the Rust build does the same linear `BTreeMap`/string compares. Gap 6 makes each byte compare expensive. |
| `check::check_expr`, `parse::*` | ~3.5 M | ~9% | port code, ordinary recursion |

Port changes were tried and kept for each port-removable item above. The rest
needs the compiler.

## Compiler gaps

Each gap has a minimal repro and its measured share of the final run.

1. **Every sequence pays two allocations and a thread-local hash map entry.**
   `vesper_seq_new` allocates a three-word header and the data buffer
   separately. It also inserts the header into a thread-local
   `HashMap<usize, u64>` (`HEADERS`, `src/seq/native_runtime.rs`). Every
   `vesper_seq_grow` looks the header up there, and every `vesper_seq_free`
   removes it. That costs 8.08 M, 21.1% of the run: about 330 instructions to
   create and free an empty sequence, and about 290 per grow. A `Q` has 17
   sequences, so every entry call pays this 17 times before it does any work.
   Possible fixes: keep the header inline in the owner (`{len, cap, data}`
   by value), track headers only in leak-check builds, or use a size-class
   arena for headers.

   ```vesper
   pub struct S { a: ChunkSeq[u64], b: ChunkSeq[u64], c: ChunkSeq[u64] }
   pub fn make() -> S {
       S { a: ChunkSeq::new[u64](), b: ChunkSeq::new[u64](), c: ChunkSeq::new[u64]() }
   }
   // Exported and called in a loop from Rust: 3 header mallocs, 3 map
   // inserts, then 3 map removes and 3 frees on drop, for no data.
   ```

2. **A local struct's sequence fields leak.** A `let` binding of a struct
   with `ChunkSeq` fields is not freed on exit. Only sequence-typed bindings
   are. Step 2 needed `free_d(sink ds: D)`, which moves each field into a
   `let` binding so that it is freed. Without it, `tests/port_memory.rs`
   fails ("leaked over 200 cycles").

   ```vesper
   pub struct D { s: ChunkSeq[u64] }
   pub fn f() -> u64 {
       let mut d = D { s: ChunkSeq::new[u64]() };
       d.s.push(1);
       d.s.len()   // d.s is never freed
   }
   ```

3. **`extend` from a view of a sibling field is refused.**
   `q.words.extend(in q.ex[p..end])` fails with
   `E0301: extend cannot read from the sequence it writes`, although `words`
   and `ex` are different fields. The workaround is a helper,
   `append(inout q.words, in q.ex, p, end)`, which passes the two fields as
   separate arguments. This costs no measurable time, only code. The checker
   should treat distinct field paths as disjoint, as it does for call
   arguments.

   ```vesper
   pub struct Q { a: ChunkSeq[u64], b: ChunkSeq[u64] }
   pub fn f(inout q: Q) { q.a.extend(in q.b[0..1]); }   // E0301
   ```

4. **`@export` directives are read from the first file in path order, not
   from the root module.** `src/main.rs` keeps the text of "file 0 of a
   package" as the root module. But `packages.rs` adds files in sorted path
   order, so in `@vesper/quarry` file 0 is `check.ves`, not `lib.ves`. With
   `// @export abi 3` in `lib.ves`, the library built as
   `vesper.native.export-abi/2` with no error. The directive now sits in
   `check.ves`, with a note. A package whose root is `lib.ves` and that has
   any file sorting before it (`a.ves`) reproduces this. The fix is to read
   directives from the root module's file. If that fix moves the directive's
   home, the committed binding check catches the change.

5. **Small leaf functions stay calls.** `lex::byte` (a `try_get` and a widen)
   still costs 0.29 M as a separate function in the final build, after the
   hottest loop was written inline by hand. `util::bound` (with an `in what:
   Str` parameter) cost 0.84 M before its per-token call was written inline,
   and still costs 0.50 M at its other call sites. `util::sw` and `util::cw`
   show the same pattern. Step 6 got 3.8% partly by writing these inline by
   hand. A repro is any `pub fn g(in s: Slice[u8], i: u64) -> u64 { match
   s.try_get(i) { Option::Some(x) => widen_u64(x), Option::None => 0 } }`
   called in a loop.

6. **Per-element reads of a sequence field through `inout q` reload the
   header.** `identifier` (2.0 M) and `lex` (2.3 M) spend most of their time
   in byte loops over `q.pool` and `sql` with `try_get`. That is about 290
   instructions per identifier of 2 to 10 bytes. Rust's `to_ascii_lowercase`
   plus validation is a few dozen. Hoisting the `{len, data}` load out of
   loops that do not write the sequence (#942 hoists bounds checks over
   slices), or giving `ChunkSeq[u8]` a scan builtin for "all bytes in a
   class", would remove most of it.

   ```vesper
   pub fn lower_ok(inout q: Q, off: u64, n: u64) -> bool {
       for i in 0..n {
           let c = match q.pool.try_get(off + i) { Option::Some(c) => c, Option::None => 0 };
           if c >= 65 && c <= 90 { return false; }
       }
       true
   }
   ```

Gaps 1, 5 and 6 cover most of the remaining distance. With gap 1 fixed, the
port would be at most about 1.55x Rust's instructions (38.23 M - 8.08 M =
30.2 M). That is an upper bound: it assumes sequences cost nothing.

## Parity

Not met. The bar is an ABBA 95% median interval that includes 1.0x or lies
below 1.05x. The final port's intervals are 2.10x-2.68x and 2.19x-2.59x.
`vesper-port` stays non-default, and the Rust build and code are unchanged.
