# Stage 1 performance lane 6: vectorized byte scans

Claim: #936. Prompt: `docs/agent-prompts/stage1-perf/06-simd-bytes.md`.
Baseline: `main` at b02714f, after lane 3 (#933, PR #947) merged.

## What changed

- **Three byte scans** on `ChunkSeq[u8]` and `Slice[u8]` (`src/seq.rs`,
  `SeqOp::SumBe16`, `SeqOp::BytesEq`, `SeqOp::FindByte`). All three are total
  and only read their operands.
  - `s.sum_be16() -> u64` is the sum, wrapping at 2^64, of the big-endian
    16-bit words `s[2k] << 8 | s[2k + 1]`. An odd last byte counts as
    `s[n - 1] << 8`. This is exactly warp-tcp's `sum_bytes` (the Rust
    reference in `src/reference/wire.rs`), so folding it gives the RFC 1071
    checksum.
  - `s.eq(in t) -> bool` is true when the byte slice `t` has the same length
    and bytes. `t` may be a view of `s`'s own sequence, since neither is
    written.
  - `s.find_byte(b) -> Option[u64]` is the index of the first byte equal to
    `b`.
  - Any element type other than `u8` is `E1525`, as for lane 3's wide reads.
- **Typing** next to lane 3's in `src/seq/check.rs`, the `in` pass of `eq`'s
  operand in `src/seq/ir.rs` (one line), and **reference semantics** in
  `src/seq/eval.rs`. The Lean model refuses the scans (`src/seq/lean.rs`), as
  it does lane 3's builtins.
- **Native lowering** in `src/seq/native_bytes.rs`. With `optimize` on and an
  x86-64 or AArch64 target, each scan runs a 128-bit vector loop, 16 bytes per
  step, then a scalar loop for the last `len % 16` bytes. Without `optimize`,
  or on other targets, only the scalar loop runs. The vector code uses only
  instructions that SSE2 and NEON have (vector loads, `band`, `ushr`, `ishl`,
  `iadd`, `icmp`, `vhigh_bits`, `ctz`), so cross-compiled objects with
  baseline flags work too.
  - `sum_be16` loads eight little-endian 16-bit lanes per step. The low byte
    of a lane is an even (high-order) byte and the high byte an odd one, so
    the sum is `256 * evens + odds`. Evens and odds add in 16-bit lanes for
    blocks of 256 steps (at most `256 * 255 = 65,280` per lane, no overflow),
    then widen to 32-bit and 64-bit lanes. The 64-bit totals cannot wrap
    before the scalar sum would.
  - `eq` compares 16-byte chunks with a lane `icmp` and `vhigh_bits` and
    stops at the first difference. Different lengths are `false` at once.
  - `find_byte` compares each chunk against a splatted needle; the first hit
    is the chunk offset plus `ctz` of the `vhigh_bits` mask.
- **Manuscript:** one new bullet, "Byte scans", in section 13 after lane 3's
  block-operations bullet.
- No syntax was added, and nothing of #587 (source `VVec`) or
  `src/native_vector.rs` was touched: the scans needed no shared helpers from
  it.

## Tests

`tests/native_simd_bytes.rs`, 6 tests. Each program runs in the evaluator,
the JIT with and without `optimize`, and an AOT executable (where `cc`
links), and every result is checked against a Rust model:

- `sum_be16` over every length 0..=256 at 17 start offsets (every alignment
  of a vector step) of a pseudo-random buffer, and over long buffers (70,001
  and 4,112 bytes of `0xff`, 9,000 random bytes) that cross the 4,096-byte
  block where the vector sum widens.
- `eq` over every length and offset: equal views, shifted views, views one
  byte shorter, and a flipped byte at the first, a middle and the last
  position (so the mismatch lands in every vector lane and in the tail), a
  copy, and a view of the sequence itself.
- `find_byte` over every length and offset: an absent byte, a natural hit,
  and a needle planted at the first, a middle and the last position.
- Optimized and unoptimized objects for `x86_64-unknown-linux-gnu` and
  `aarch64-unknown-linux-gnu`: the optimized ones contain `i8x16`, `i16x8`
  and `vhigh_bits`, the unoptimized ones no vectors. This runs on every CI
  host, so both vector lowerings compile on all four platforms.
- Type errors: a non-byte element type, wrong arity, a wrong needle type.

A deliberately wrong weight in the vector sum (`<< 7`) fails both sum tests,
so the differential tests see the vector path.

## Stage 1 guard

- Nothing under `src/verified_codegen/**`, `src/ir.rs`, `ir_opt::optimize`,
  `lean/**`, `bootstrap/**` or `packages/vesper-compiler/**` changed.
- The scans enter a program only when its source calls them, and the Stage 1
  corpus does not. The vector path also needs `optimize`, which
  `bootstrap/stage1.rs` turns off on both backends.
- The `bootstrap-stage1` workflow run on this branch is linked from the PR
  body.

## Port measurements

Setup: Linux x86-64 cloud container, 4 vCPU, release builds, the ports built
by this branch's compiler. Numbers are callgrind instruction counts (`Ir`),
which are deterministic; wall-clock numbers on this shared host move too much
(see lane 3's report). Every Vesper arm printed the same digests as Rust.

The local rewrites are not pushed (the ports keep their sources and Rust stays
the default build):

- **warp-tcp, wide reads (lane 3's rewrite, redone here for the checksum
  only):** `sum_range_raw` reads 8 bytes at a time with `read_u64_be` and adds
  the two 32-bit halves, then finishes with the byte-pair loop.
- **warp-tcp, `sum_be16`:** `sum_range` becomes
  `wrapping_add(acc, sum_view(in bytes[from..end]))`, where `sum_view` calls
  `s.sum_be16()`. The raw sum equals the Rust `sum_bytes` exactly. The
  warp-tcp port's tests pass with it.
- **Prism, `find_byte`:** the decoder's `find(in s, lo, hi, b)` calls
  `find_byte` on `s[lo..min(hi, len)]` and keeps the old meaning past the end
  (`byte` reads 0 there). Prism's port tests pass with it.

| Port, workload | Rust Ir | Vesper Ir | ratio |
|---|---|---|---|
| warp-tcp codec, 64 requests: port as on `main` | 33.9 M | 259.0 M | 7.63x |
| warp-tcp codec: wide-read checksum (lane 3) | 33.9 M | 83.2 M | 2.45x |
| warp-tcp codec: `sum_be16` checksum (this lane) | 33.9 M | 40.4 M | **1.19x** |
| warp-tcp transfer, 4 transfers: port as on `main` | 8.33 M | 46.3 M | 5.56x |
| warp-tcp transfer: wide-read checksum | 8.33 M | 37.9 M | 4.55x |
| warp-tcp transfer: `sum_be16` checksum | 8.33 M | 35.9 M | **4.31x** |
| Prism `tests/malformed.rs`: port as on `main` | 101.7 M | 264.5 M | 2.60x |
| Prism `tests/malformed.rs`: `find_byte` in `find` | 101.7 M | 253.2 M | **2.49x** |

The checksum was 92% of the codec's instructions (lane 3's report), and
`sum_be16` takes it to 1.19x of Rust. Transfer moves less because its
checksums are small next to the connection logic; lane 3's other rewrites
(`copy_into` with `try_copy_from`, `rd16`/`wr16` with the wide accesses) took
it to 2.2x, and they add to this. Prism's header lines are short (tens of
bytes), so the scan saves little per call: most of its time is the
byte-at-a-time state machine in `scan_head` and the per-header validation
loops, which `find_byte` does not replace.

## What is left

- The ports adopting the scans (warp-tcp's `sum_range`, Prism's `find`) is
  port follow-up work. This lane measured local rewrites only.
- Prism's `scan_head` looks for CR and LF one byte at a time across two
  buffers. A two-needle scan (`find_either(a, b)`) or a CRLF-CRLF search would
  let it skip ahead; that needs a new builtin or a restructured scanner.
- `eq` could also compare case-folded ASCII (Prism's `eq_lit(..., fold)`),
  which is another builtin.
- Wider vectors (AVX2) need Cranelift support for 256-bit types, which 0.125
  does not have.
- The Lean model of the scans (refused today, like lane 3's builtins).
