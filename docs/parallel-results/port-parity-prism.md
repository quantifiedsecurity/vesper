# Port parity wave, lane 3: Prism adopts the byte builtins and SIMD scans

Claim: #951. Branch: `claude/project-thread-sa8xnh`, from `main` at
`cee8b1a` (the plan, #958; a descendant of the required `69c79b9`).

The Vesper build of vesper-prism goes from **5.3-6.6x** slower than Rust to
**2.1-3.4x** on a new wall-clock ABBA benchmark, and from 2.60x to 1.84x in
instructions on `tests/malformed.rs`. It does not meet the parity bar. Rust
stays the default build and no Rust code was deleted.

## What changed

All edits are inside `crates/vesper-prism/`, plus one `Cargo.lock` line for the
benchmark's `vesper-bench` dev-dependency.

- **Benchmark.** `examples/abba.rs` and `bench-abba.sh` (the Aura pattern: the
  example is built once per build and run as two processes, Rust, port, port,
  Rust per epoch). One operation is a pipelined batch of 32 requests (GET,
  POST and PUT with bodies, 5-7 fields each, 6 KiB in all), in four workloads:
  `decode` (one `Decoder`, drain every request), `route` (`Target::parse` plus
  `Router::resolve` against 14 routes), `encode` (one `ResponseBuilder` with
  three fields and a body per request) and `connection` (receive the batch,
  issue every request, submit responses, write the output). It prints the
  median Vesper/Rust ratio with a distribution-free 95% interval of the median
  (order statistics 4 and 12 of 15 epochs).
- **`find_byte`.** `find` in the decoder and the target parser calls
  `find_byte` on `s[lo..min(hi, len)]`; `count` in the target parser is a loop
  of `find`s.
- **Bulk copies.** `push_range`, the response field values, the response
  field copies in `finish` and `push_text` use `extend`. A head that lies in
  the front slice of the decoder's queue is parsed in place, and one that
  wraps is copied with two `extend`s instead of a `queued` call per byte.
  Target segments and query parts are decoded straight into the result, and a
  part without `%` (and without `+` where `+` is a space) is one `extend`.
- **Slice `eq`.** Case-sensitive literals (`HTTP/1.1`, `OPTIONS`, `HEAD`) and
  router key and method comparisons use `eq`.
- **`scan_head`.** Outside a CR, the scan skips the run of bytes that are
  neither CR nor LF with two `find_byte`s (CR, then LF before it), up to the
  end of the current slice or the head limit. The state machine and its error
  order are unchanged.
- **Hot loops without helper calls.** Loops read with `try_get` directly
  instead of `byte(in s, i)`, because a function with a slice parameter is
  never inlined (gap 1). `tchar` and the origin-form byte class are two
  64-bit masks tested with `rotate_right`: the comparison-chain version grew
  past the inliner's size limit (gap 5).
- **Fewer comparisons and allocations in `parse_head`.** A field name is
  compared only with the literals of its length; the name's token check and
  its lowercase copy are one pass; the head's six sequences are sized once.
  The response encoder gates its reserved-name checks the same way.
- **Adapter.** `forward/decoder.rs` converts ASCII method, target and name
  bytes with one copy instead of a `char` push per byte.

The decoder's Host grammar, the router's matching, the connection state
machine and the target's UTF-8 check are unchanged apart from the shared
helpers above.

## Setup

Cloud container, 4 vCPU Intel Xeon @ 2.30GHz, Linux 6.18, Rust 1.97.0 stable
for the measurements (`--release`), the port built by the harness with
`optimize = true`. ABBA: 15 epochs, 200 operations per run, 50 warm-up
operations. Instruction counts: `valgrind --tool=callgrind`, whole test
binary (`--test-threads=1`) or `abba measure 20`.

Most intermediate steps were built with a local, uncommitted compiler change
that only renamed local symbols (`vesper_function_N` to
`vesper_function_N_<name>`) for the profiles. The final row is the unchanged
compiler. One profiling build also raised the inliner's program growth budget
tenfold; it changed nothing (90,612,163 against 90,612,147 instructions), and
it was reverted before any row below except "byte classes as masks".

## Results

Final check, after review fixed the `connection` workload (it stopped at the
connection's 16-request in-flight limit and handled only half the batch): one
ABBA run of `main` and one of this branch, back to back, same machine, the
same fixed benchmark.

| Build | decode | route | encode | connection |
|---|---|---|---|---|
| `main` (`cee8b1a`) | 5.28x (4.98-5.48) | 5.26x (5.14-5.34) | 6.55x (6.44-6.68) | 5.30x (5.12-5.38) |
| this branch | **2.14x (1.99-2.17)** | **3.43x (3.40-3.50)** | **3.43x (3.36-3.55)** | **2.42x (2.38-2.47)** |

Rust took about 45 µs (decode), 9.2 µs (route), 9.7 µs (encode) and 52 µs
(connection, the whole batch) per operation; the branch took 94, 32, 34 and
126 µs. Encode came out at 3.04x in the per-lever run below and 3.43x here,
so run-to-run spread on this container is about 10%.

### One lever at a time

Wall clock, Vesper/Rust median ratio and its 95% interval, one ABBA run per
step on the same machine. The `connection` column here covers only the
first 16 requests of each batch (the bug above):

| Step | decode | route | encode | connection |
|---|---|---|---|---|
| `main` (`cee8b1a`) | 4.99x (4.95-5.05) | 5.14x (5.05-5.24) | 6.10x (6.01-6.20) | 5.10x (4.98-5.16) |
| `find_byte` in `find` | 4.42x (4.38-4.49) | 5.13x (4.93-5.24) | 6.08x (5.93-6.18) | 4.57x (4.52-4.65) |
| bulk copies (`extend`, head in place) | 3.39x (2.67-3.51) | 4.96x (4.01-5.20) | 3.82x (3.12-3.96) | 3.52x (3.09-3.66) |
| slice `eq` for literals | 3.40x (2.83-3.60) | 5.15x (4.91-5.24) | 3.93x (3.80-3.99) | 3.61x (3.52-3.80) |
| `scan_head` skips with `find_byte` | 3.00x (2.90-3.03) | 5.19x (4.91-5.23) | 4.03x (3.94-4.08) | 3.16x (3.03-3.20) |
| decoder/response loops without `byte()` calls | 2.70x (2.60-2.84) | 5.16x (4.99-5.75) | 3.56x (3.40-3.78) | 2.92x (2.83-3.39) |
| target and router (`find_byte`, in-place decode, `eq`) | 2.76x (2.68-3.35) | 3.69x (3.66-4.78) | 3.57x (3.48-4.25) | 2.96x (2.89-3.51) |
| length-gated names, fused name pass, sized heads | 2.27x (2.20-2.37) | 3.69x (3.61-3.84) | 3.36x (3.32-3.43) | 2.53x (2.36-2.61) |
| byte classes as masks | 2.09x (2.07-2.18) | 3.44x (3.37-3.50) | 3.16x (3.08-3.26) | 2.39x (2.34-2.44) |
| ASCII copy in the adapter (final, unchanged compiler) | **2.01x (1.97-2.07)** | **3.42x (3.31-3.44)** | **3.04x (2.98-3.11)** | **2.29x (2.28-2.34)** |

Instructions (Rust: `tests/malformed.rs` 101.7 M, `abba measure 20` 41.8 M):

| Step | `malformed.rs` | ratio | `abba measure 20` | ratio |
|---|---|---|---|---|
| `main` | 264.5 M | 2.60x | 195.4 M | 4.67x |
| `find_byte` in `find` | 252.9 M | 2.49x | 178.0 M | 4.26x |
| bulk copies | 215.7 M | 2.12x | 134.6 M | 3.22x |
| slice `eq` | 213.5 M | 2.10x | 134.3 M | 3.21x |
| `scan_head` | 198.4 M | 1.95x | 117.5 M | 2.81x |
| loops without `byte()` | 190.9 M | 1.88x | 104.7 M | 2.50x |
| target and router | 190.0 M | 1.87x | 99.8 M | 2.39x |
| names, sized heads | 191.2 M | 1.88x | 90.6 M | 2.17x |
| masks | 188.4 M | 1.85x | 85.1 M | 2.04x |
| adapter (final) | 187.6 M | **1.84x** | 83.7 M | **2.00x** |

`malformed.rs` is mostly short, mutated, rejected inputs, so the copy and scan
levers matter less there than on well-formed batches. Sizing heads up front
cost it about 1 M (more allocation for heads that fail later).

Every step passed all of the crate's tests on the Vesper build
(`cargo test --release -p vesper-prism --features vesper-port`, including the
`malformed.rs` outcome digest that both builds must reproduce).

## What is left

Profile of the final Vesper code (`abba measure 20`, 85.1 M instructions,
named symbols), top costs by self instructions:

| Cost | Share | Port or compiler |
|---|---|---|
| malloc, free, realloc, `vesper_seq_grow`/`new`/`free`, memcpy | 23.8% | Mostly port design, partly the ABI crate (gap 6) |
| Helpers with slice parameters that stay calls (`find`, `first_byte`, `first_or`, `line_break`, `eq_lit`, `trim_ows`, `byte`, `valid_label`, `same`) | 23.5% | Compiler (gap 1) |
| `parse_head` itself: the field-value control check and the per-field bookkeeping | 12.1% | Compiler (gap 2) |
| `response_plan_header`: token, control and lowercase loops | 7.5% | Compiler (gaps 2, 3) |
| `push_token_lower`: the name's token check and lowercase copy | 7.2% | Compiler (gaps 2, 3) |
| The Rust adapter's `next_request` (copying head fields out) | 4.0% | Port (the Rust reference does similar copies) |
| `eq_lit` with `fold` (`named`) | 3.2% | Compiler (gap 3) |
| `scan_head`, `line_break`, `first_or` | 6.0% (inside the rows above) | Compiler (gap 4) |

For comparison, the Rust build spends 7.4 M instructions on allocation in the
same run, against 20.2 M for the port.

## Compiler gaps

1. **The inliner never inlines a function with a slice parameter.**
   `src/ir_inline.rs` (`inlinable`) refuses any callee with an `in`/`inout`
   `Slice` parameter, and a view can only be written as a call argument, so
   every use of a builtin on a sub-range (`s[lo..hi].find_byte(b)`, `eq` on a
   range) needs a one-line helper that then costs a full call. Prism's helpers
   all take `in s: Slice[u8]`. Their self cost is 23.5% of the port's
   instructions; inlining would remove the call overhead (argument spills,
   frame, result copy), not all of it. Repro:

   ```vesper
   fn byte(in s: Slice[u8], i: u64) -> u8 {
       match s.try_get(i) {
           Option::Some(b) => b,
           Option::None => 0,
       }
   }

   fn first(in s: Slice[u8], b: u8) -> Option[u64] {
       s.find_byte(b)
   }

   pub fn spaces(in s: Slice[u8]) -> u64 {
       let mut n: u64 = 0;
       for i in 0..s.len() {
           if byte(in s, i) == 32 {
               n = wrapping_add(n, 1);
           }
       }
       match first(in s[1..s.len()], 10) {
           Option::Some(i) => wrapping_add(n, i),
           Option::None => n,
       }
   }
   ```

   Built with `vesper build --lib` (optimized), `spaces` still calls both
   `byte` and `first` (checked with `objdump -dr` on the object). Before this
   lane, `byte` alone was 21% of the port's instructions.

2. **No byte-class scan.** HTTP checks "every byte is a `tchar`", "no control
   byte except tab", "every byte is an origin-form character" and "all ASCII"
   (before UTF-8 validation). Each is a per-byte loop in generated code. A
   builtin such as
   `s.find_not_in(lo_mask, hi_mask)` (the first byte whose bit is clear in a
   128-bit class, bytes >= 128 by a flag), run on the same 128-bit vectors as
   `find_byte`, would replace the loops in `parse_head` (12.1%),
   `push_token_lower` and `response_plan_header` (14.7% together, with gap 3),
   `valid_target` and `valid_origin` (3%). Repro: the field-value check in
   `parse_head`:

   ```vesper
   pub fn value_ok(in s: Slice[u8]) -> bool {
       for i in 0..s.len() {
           let b = match s.try_get(i) {
               Option::Some(b) => b,
               Option::None => 0,
           };
           if (b < 32 && b != 9) || b == 127 {
               return false;
           }
       }
       true
   }
   ```

3. **No ASCII case-folding compare or copy.** `eq_lit(..., fold)` (3.2%) and
   the lowercase copies of field names (inside `push_token_lower` and
   `response_plan_header`) are per-byte loops. `s.eq_ignore_ascii_case(in t)`
   and `out.extend_lowercase(in s)` would make both one vector pass. Repro:
   `eq_lit` in `crates/vesper-prism/vesper/decoder/src/syntax.ves` with
   `fold = true`.

4. **No two-needle scan.** `scan_head` needs the first CR or LF. With
   `find_byte` it runs two scans per line (`line_break`: CR, then LF before
   it). `scan_head`, `line_break` and `first_or` are 6.0% together; a
   `find_either(a, b)` (or a CRLF search) would halve the scanning and remove
   the second call. Repro: `line_break` in
   `crates/vesper-prism/vesper/decoder/src/lib.ves`.

5. **The inliner's size limit counts a predicate after its callees are
   inlined into it.** `token(b)` written as `is_alnum(b) || tchar_symbol(b)`
   with a nine-term `||` chain grew past `small_callee` (48) once `is_alnum`
   and `tchar_symbol` were inlined into it, so `token` itself became a call
   (5.9% of the run) at its several call sites. The port now uses 64-bit
   masks. Short-circuit chains lowering to one block per term make small
   predicates look large. Repro:

   ```vesper
   fn is_alnum(b: u8) -> bool {
       (b >= 48 && b <= 57) || (b >= 65 && b <= 90) || (b >= 97 && b <= 122)
   }
   fn symbol(b: u8) -> bool {
       (b == 33) || (b >= 35 && b <= 39) || (b == 42) || (b == 43) || (b == 45)
           || (b == 46) || (b >= 94 && b <= 96) || (b == 124) || (b == 126)
   }
   pub fn token(b: u8) -> bool {
       is_alnum(b) || symbol(b)
   }
   ```

   In the same object as gap 1's repro, with `token` called from a loop and
   twice from `token_pair(a, b) = token(a) && token(b)`, every call site
   still calls `token`.

6. **Allocation, partly outside the port.** The port allocates 2.7x the
   instructions Rust does. Each parsed head is six `ChunkSeq`s (freed when the
   next head replaces it), and the adapter copies them out through
   `SeqRef::to_vec`, which reads one element at a time
   (`crates/vesper-port-abi/src/seq.rs`; there is no `as_slice` for `u8`
   sequences). A port follow-up could return field ranges into the input
   instead of copies, which changes the decoder binding; a `SeqRef<u8>`
   slice view belongs to the ABI crate, which this lane may not edit.

## Parity

Not met. Every port test passes on the Vesper build locally (CI covers the
four platforms), but the ABBA ratios are 2.1x (decode), 3.4x (route), 3.0-3.4x
(encode) and 2.4x (connection), far from the 1.05x bar. Rust stays the
default build.
