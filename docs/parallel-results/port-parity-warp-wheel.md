# Port parity lane 1: warp-wheel adopts the Stage 1 speedups (#949)

Base: `main` at cee8b1a (plan PR #958, a descendant of 69c79b9). Prompt:
`docs/agent-prompts/port-parity/01-warp-wheel.md`. Port-only: no compiler,
ABI or harness change. Rust stays the default build and no Rust code was
deleted.

**Result:** the ABBA ratio went from **8.0x** (P1) to **4.98x** on `main`
with no port change (the Stage 1 compiler wave), and to **3.37-3.43x** with
this lane's port changes. The port does **not** meet the parity bar. What is
left is code generation: about half of the port's instructions are stack
slot zeroing, spills and reloads (compiler gaps 1 and 2 below).

## What changed in the port

All in `crates/vesper-warp-wheel/vesper/src/` (the binding in
`port/binding.rs` was re-blessed for the two new private `WheelCore`
fields).

1. **Element reads the inliner accepts.** `at()` took `in s: Slice[u64]`, and
   every call passed `self.e_x.as_slice()`. The inliner refuses any callee
   whose types mention a slice parameter, so the 16% helper of the P1 profile
   was still a call after #944. It now borrows the `ChunkSeq` itself
   (`at(in self.e_deadline, i)`); the `match s.try_get(i)` inside is the fused
   read of #945, and the helper is inlined everywhere.
2. **A total divide.** `div()` wrapped `a / b` in a `Fail[RuntimeError]`
   handler. Written as `if b != 0 { a / b } else { 0 }`, the checker proves
   the division total, so there is no handler and it inlines.
3. **The level without a bit scan.** `place()` computed the wheel level as
   `high_bit(diff) / slot_bits` (a six-step bit search, then a divide). It
   now compares `diff` against the precomputed spans
   `l_span[l] = 2^((l + 1) * slot_bits)` (new field, at most `levels`
   compares, no shift or divide), and it has a single `push_back` call site,
   so the inliner takes `push_back` (a 48-instruction callee with two sites
   stays a call).
4. **Total shifts.** `shl()`/`shr()` wrapped `x << s`/`x >> s` in a
   `Fail[ArithOverflow]` handler, since a variable shift amount may fail even
   behind `s < 64`. They are now written with the total `rotate_left` and
   `rotate_right` (manuscript, integer kinds): `2^s` is `rotate_left(1, s)`,
   `x << s` is `rotate_left(x, s) & -2^s` and `x >> s` is
   `rotate_right(x & -2^s, s)`. No handler, so they inline.
5. **A de Bruijn bit scan.** `low_bit()` (trailing zeros, used by
   `next_work_tick` on every advance step) called `high_bit`. It is now a
   method that multiplies the isolated low bit by a de Bruijn constant and
   looks the top six bits up in `l_ctz` (new field, 64 words, built by the
   constructor from the constant, so the table checks itself). `high_bit` is
   gone.

Levers in the prompt that do not fit this port:

- **`fill`/`copy_from`.** The wheel never clears or moves a block of slots:
  opening a slot or cascading relinks list heads in O(1), and the
  constructor is outside the timed operation. There is no loop for them to
  replace.
- **Export ABI v3.** The only `copy` struct that crosses the boundary is
  `WheelConfig`, once, in the constructor. The per-call values are scalars,
  `Option`/`Result` and tuples, which v3 converts exactly as v2 does.
  `WheelCore` is a handle (it owns sequences), which v3 does not mirror.
- **Scratch in handle fields.** The port keeps none; `drain` returns a fresh
  sequence.

Tried and reverted:

- **One record per entry** (one `ChunkSeq` with 8 words per entry instead of
  seven parallel sequences, like the Rust build's `Vec<Entry>`): **3.83x**
  [3.76, 3.86], slower than the 3.43x of the parallel sequences. The index
  arithmetic and the extra inlined locals cost more than the cache lines save;
  each inlined local is one more stack slot zeroed at entry (gap 1).
- **`bit_shl`/`bit_shr`** (the total `ConstTime` intrinsics) in place of the
  shift handlers: 4.16x [4.12, 4.29] against 4.36x before. Not kept: the
  manuscript does not specify them for plain integers, and the `rotate` form
  (lever 4) is specified and as fast.

## Ratios

Setup: cloud container, 4 vCPU Intel Xeon @ 2.10 GHz, Linux x86_64, Rust
1.97 `--release`, Vesper `optimize = true` (the harness default), the
in-tree compiler of `main` cee8b1a. `tests/abba.rs --ignored`: 30 epochs + 2
warm-up, 8 operations per window (16 per arm per epoch); one operation
inserts 4096 entries at mixed near/mid/far/overflow deadlines into a
4096-entry wheel (1 µs ticks, 64 slots, 4 levels), advances in 64 steps of
1 ms polling everything due, then drains. Each row is one ABBA run, back to
back in one session, with the levers applied cumulatively. "Instructions" is
callgrind's count for one operation of each build (`both_builds_...` test):
the Vesper functions over the Rust reference functions, constructors
excluded.

| Step | Rust µs/op | Vesper µs/op | Vesper/Rust median | 95% interval | Instructions |
|---|---:|---:|---:|---:|---:|
| P1 (#924, Rust 1.97, older compiler) | 328 | 2,601 | 8.00 | [7.83, 8.14] | |
| `main` cee8b1a, no port change | 295 | 1,455 | **4.98** | [4.87, 5.02] | 5.62x |
| 1. `at()` borrows the sequence | 292 | 1,287 | 4.39 | [4.28, 4.50] | 4.61x |
| 2. handler-free `div()` | 292 | 1,250 | 4.36 | [4.29, 4.42] | 4.51x |
| 3. level by span compare, one `push_back` site | 289 | 1,166 | 4.06 | [4.02, 4.10] | 4.41x |
| 4. total shifts by rotation | 292 | 1,171 | 3.99 | [3.92, 4.01] | 4.06x |
| 5. de Bruijn `low_bit()` | 285 | 979 | **3.43** | [3.37, 3.47] | 3.88x |
| 5, re-run on the final commit | 299 | 1,003 | 3.37 | [3.32, 3.42] | |

The `analyze_pairs` 2x gate reports `Regression` at every step.

## What is left

Profile of the final port (callgrind, one operation, exclusive
instructions): `WheelCore::place` 2.0M (15% of the whole process),
`next_work_tick` 1.5M, `sort_chain` 1.4M, `open_tick` 0.9M, `insert` 0.56M,
`unlink` 0.54M, `pop_due` 0.48M. The Rust build runs the same algorithm (the
same cascade, bitmaps and bottom-up merge sort); its whole `poll` is 1.6M.

Classifying every executed Vesper instruction by its disassembly (same run,
9.0M Vesper instructions including the constructor):

| Kind | Share | Port or compiler |
|---|---:|---|
| `movq $0, slot(%rsp)`: zeroing stack slots at entry | 19.4% | compiler (gap 1) |
| store of a register to a stack slot | 14.1% | compiler (gap 2) |
| load of a stack slot into a register | 13.6% | compiler (gap 2) |
| compares and branches (includes doubled bounds checks) | 16.8% | partly compiler (gap 3) |
| call, return and frame setup | 3.2% | mostly gone (levers 1-5) |
| everything else (the work) | 33.0% | |

`place` executes 96 slot-zeroing stores per call, `open_tick` 121 and
`next_work_tick` 113. Without the zeroing, spills and
reloads the port would run about half its current instructions, which puts
it near 2x Rust on instructions. No source change in the port can remove
these: they come from how every local is lowered.

## Compiler gaps

Each repro builds with `vesper build <file> --lib -o lib.a`; read the
result with `objdump -d`.

1. **Every stack slot is zeroed at entry, and twice** (19.4% of the port's
   instructions). Not only sequence slots (whose zero is their drop flag,
   #945) but every scalar local, and each slot gets two identical
   `movq $0x0` stores. Inlining makes it worse, since each inlined local is
   one more slot (the one-record experiment above). Fix: zero only the slots
   whose drop flag is read before a store, and once.

   ~~~vesper
   pub fn mix(a: u64, b: u64) -> u64 {
       let x = wrapping_add(a, b);
       let y = wrapping_mul(x, a);
       let z = y ^ b;
       let w = wrapping_sub(z, x);
       w & y
   }
   ~~~

   `vesper_<lib>_mix` zeroes six slots twice each (12 stores) before it
   reads an argument.

2. **Every local lives in a stack slot** (27.7%: 14.1% stores, 13.6%
   reloads). Each use reloads the slot, so values never stay in registers
   across statements, and a sequence's header (length, data pointer) is
   reloaded at every access. The same `mix` stores `a` and `b` and reloads
   them. Noted as the blocker of header hoisting in
   `stage1-perf-seq-access-drops.md`. Fix: SSA values for locals that are
   not borrowed (Cranelift `Variable`s), slots only for borrowed or
   drop-flagged ones.

3. **A fused `try_get` compares twice.** The fused read (#945) emits
   `cmp len, i; jb load` and then the same `cmp; jb` again on the fall-through
   path, whose second target is a `ud2`. Every element read in the port
   pays one redundant compare and branch.

   ~~~vesper
   pub fn sum3(in s: ChunkSeq[u64], i: u64) -> u64 {
       let a = match s.try_get(i) {
           Option::Some(v) => v,
           Option::None => 0,
       };
       let b = match s.try_get(wrapping_add(i, 1)) {
           Option::Some(v) => v,
           Option::None => 0,
       };
       wrapping_add(a, b)
   }
   ~~~

4. **A variable shift always raises.** The checker knows exact values only,
   not ranges, so `x << s` inside `if s < 64` still needs
   `Fail[ArithOverflow]` (E0401 without a handler), and a function with a
   handler is never inlined. The port works around it with rotations (lever
   4); a range fact from the guard, or specified total `wrapping_shl` /
   `wrapping_shr`, would make the plain operator free.

   ~~~vesper
   fn shl(x: u64, s: u64) -> u64 {
       if s < 64 {
           x << s
       } else {
           0
       }
   }
   ~~~

5. **No bit-count builtins.** The manuscript has no `leading_zeros`,
   `trailing_zeros` or `count_ones`, so the port needs a table and a
   multiply (lever 5) or a compare loop (lever 3) where Rust emits one
   `lzcnt`/`tzcnt`. This needs a spec change plus native lowering to the
   instructions.
6. **The inliner refuses slice parameters.** A helper taking
   `in s: Slice[u64]` is never inlined (`ir_inline.rs`: "no type in it
   mentions ... a slice parameter"), so the idiomatic `at(in s.as_slice(), i)`
   costs a call per element read. Lever 1 avoids it by borrowing the
   `ChunkSeq`; ports whose helpers must take slices cannot.
7. **Two call sites over 48 instructions stay calls.** `push_back` (two
   sites in `place`) was a call until `place` was rewritten to one site
   (lever 3). A budget that counts the instructions a call saves, or a
   larger budget for leaf callees, would cover helpers like it.

## Tests

On the final commit, locally: `port/test-port.sh` (the crate's tests with
and without `--features vesper-port`, `vesper-warp` against the port, and
the divergent port caught in every module) passes. That covers the leak test
(`tests/port_memory.rs`, `port_live_allocations()`) and the allocation audit.
`cargo +1.100.0 clippy --all-targets -p vesper-warp-wheel --features
vesper-port -- -D warnings` is clean. macOS, Windows and Bazel: see the PR's
checks.

## Parity

Not met: the 95% interval is [3.32, 3.42], far above 1.05x. Rust stays the
default build (owner's rule). Most of the remaining gap is compiler gaps 1
and 2; after those, gaps 3-5 are the next costs this port would show.
