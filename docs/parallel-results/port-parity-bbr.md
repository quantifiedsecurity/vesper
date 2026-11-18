# Port parity lane 2: BBR (#950)

Baseline: `main` at cee8b1a. The crate is `crates/vesper-warp-bbr`. Rust is
still the default build, the `vesper-port` feature is still non-default, and
no Rust code was deleted.

**Result.** On `examples/ack_path_abba.rs` the Vesper build went from 11.6x
to about 5.4x slower than the Rust build, and from 7.9x to 3.5x Rust's
instruction count. The port does not meet the parity bar. Almost all of what
is left is the native code the compiler generates for the Vesper functions,
which only the compiler can fix (gaps 1 to 3 below).

## What changed in the port

1. **Export ABI v3** (`// @export abi 3` in `types.ves`, the first part of
   the unit). In the Vesper build, the public `Bbrv3` now holds the binding's
   mirror `port::repr::Bbrv3`, and every call lends it to Vesper in place.
   Before, the binding converted all 70 words in and out on each call.
   - The fields are defined once, in a `bbrv3_fields!` macro. The Rust build
     gets `Bbrv3` itself, exactly as before. The Vesper build gets
     `Bbrv3Value`, the Rust value of the Vesper `Bbrv3`. The binding's
     `rust_types` now points at the new `port_types` module, which re-exports
     the crate's types under the Vesper names (`Bbrv3Value as Bbrv3`).
   - The 19 field getters of the Vesper build read mirror cells
     (`bbr/forward.rs`). The full-word ones stay `const fn`. `state`,
     `min_rtt_ns`, `full_bw_reached`, `in_recovery` and `last_rate_sample`
     read a `Word`/`Words` cell, and `Word::get` is not `const`, so in the
     Vesper build those five are plain `fn`. The P1 port already made the
     forwarded `pacing::*` functions non-`const`, and no caller in the
     workspace uses these getters in a const context. `can_send` stays
     `const fn` in both builds, because `vesper-warp` needs that.
   - `Debug` for the Vesper `Bbrv3` formats the Rust value, so it prints the
     same text as the Rust build. `rejected_inputs_leave_state_unchanged`
     compares that text.
   - The filters (`CycleMax`, `WindowedMax`) convert to their 2- and 6-word
     mirrors around their own calls. Inside `Bbrv3` they are lent in place
     with the controller.
   - The unit tests read and write controller fields by name. Those accesses
     now go through `b.fields()` and `b.fields_mut()`. In the Rust build these
     return the struct itself. In the Vesper build, `fields_mut()` is a guard
     that converts the mirror to the Rust value and stores it back when it
     drops. This is a mechanical rewrite of 60 field accesses, and every
     assertion is unchanged.
2. **Handler-free `div` and `pow2`** (`arith.ves`). `div` checks `b == 0`
   explicitly. The checker then knows the divisor is non-zero, so `a / b`
   needs no `Fail[RuntimeError]` handler. `pow2` uses the total
   `rotate_left(1, n)` for `n < 64`. Both return 0 exactly where the old
   handlers did, and the inliner now inlines both.
3. **`mul_wide`** (manuscript §13 integer kinds) replaces the hand-written
   32-bit-limb `mul_full`, at every call site (`arith.ves`, `control.ves`,
   `pacing.ves`).
4. **`Fail` rows** (#946 register-status ABI, #943 keep-writes). The
   entry points `bbrv3_record_send`, `ack_check`, `ack_check_totals`,
   `check_time`, `bbrv3_on_packet_lost`, `bbrv3_on_ecn_ce`,
   `bbrv3_on_persistent_congestion`, `bbrv3_on_app_limited` and
   `bbrv3_on_spurious_loss_detected` raise `Fail[BbrFault]` instead of
   returning `Result[.., BbrFault]`. The generated binding still returns
   `Result<_, BbrFault>`. Every check still runs before the first write, so
   under keep-writes a rejected input still leaves the controller unchanged,
   and the rejection tests check that. `bbrv3_new` keeps `Result`, because it
   runs once per flow.
5. **The empty ACK progress is computed once** (`forward.rs`
   `empty_progress`, a `OnceLock` of `port::ack_progress_new()`). Building
   the 21-word constant in Vesper cost about 200 instructions on every ACK
   (gap 2). The value still comes from Vesper.

No other file changed. `vesper fmt --check` passes on `vesper/src`, and the
divergence edits still apply once each.

### Tried and dropped

- **One call per one-packet ACK** (`bbrv3_ack_one`: the checks and the step
  together). It took 77.93 M instructions against 77.83 M without it, so it
  gained nothing. The call has to rebuild the progress constant inside Vesper
  (gap 2), and that costs what the two saved crossings save. It was removed.

## Measurements

Setup: cloud x86-64 container, 4 vCPU Intel Xeon @ 2.10 GHz, Linux 6.18,
release profile, Rust 1.97 (the default toolchain here; clippy was run with
1.100). The ports were built by the in-tree `vesper` from cee8b1a with
`optimize = true` (the `vesper build --lib` default). All five ABBA runs
below ran back to back in one session on the same binaries:
`ack_path_abba abba <rust> <vesper> 16 200000`, which is 16 epochs of
200,000 steps, with the Rust build as arm A.

| Step | Vesper/Rust median | 95% interval of the median ratio | Instructions (callgrind) | vs Rust |
| --- | --- | --- | --- | --- |
| `main` (cee8b1a) | 11.6x | 10.34x to 11.77x | 172.52 M | 7.85x |
| + ABI v3, mirror as state (1) | 6.9x | 4.91x to 7.20x | 87.41 M | 3.98x |
| + handler-free `div`/`pow2`, `mul_wide` in `mul_full` (2, 3) | 6.5x | 4.75x to 8.18x | 85.76 M | 3.90x |
| + `Fail` rows, cached empty progress, `mul_wide` at the call sites (3, 4, 5) | 5.3x / 5.5x (two runs) | 5.21x to 5.92x / 4.27x to 6.10x | 77.83 M | 3.54x |
| Rust | 1x | | 21.97 M | 1x |

The instruction counts are for `ack_path_abba measure 20000`: 5,000 warm-up
steps, then 20,000 measured steps. They are deterministic.

An earlier run in the same session, with 16 epochs, gave 10.58x to 12.84x for
`main`, 6.43x to 6.87x for v3, and 5.03x to 6.78x for v3 plus step 2.

How the last row's 7.9 M instructions split up, measured per function:

- The cached empty progress saves 5.1 M (`vesper_bbr_ack_progress_new` is
  gone).
- The `Fail` rows on `record_send` and `ack_check` save 1.6 M.
- The remaining `Fail` rows and `mul_wide` at the call sites save 1.2 M.

The wall-clock gain is smaller than the instruction gain. The removed
conversions were simple, parallel loads and stores, while what is left is
dependent arithmetic with stack round trips (gap 1).

## Profile of what is left

Callgrind of the final build, with local symbols named by a throw-away
compiler patch that was not committed. The patch does not change the code:
the instruction count was the same.

| Share | What | Port or compiler |
| --- | --- | --- |
| 84.8% (66.0 M) | Vesper code. The top items are `update_model_and_state` 14.3%, `finish_ack` 11.8%, `bbrv3_ack_step` 8.9%, `set_cwnd` 8.6%, `bbrv3_record_send` 6.0%, `update_ack_aggregation` 5.8%, `ack_check` 4.4%, `bw_volume` 3.6%, `schedule_edt_ns` 3.5%, `adapt_long_term_model` 3.1%, `quantization_budget` 2.7%, `mul_gain` 2.6% | compiler (gaps 1 and 3) |
| 8.5% (6.6 M) | Rust glue: `on_ack` (163 instructions per ACK), the binding wrappers, and the `Result<(), BbrFault>` reads | port boundary (gap 4) |
| 4.9% (3.8 M) | the benchmark's own `Flow::step` | same as Rust (3.8 M) |
| 1.0% | `memcpy` | binding |

Rust's whole run is 22.0 M instructions, and 18.2 M of that is the BBR logic
(the rest is `Flow::step`). The Vesper code does the same work in 66.0 M,
which is 3.6x. Before this lane, the binding's `Abi::get`/`Abi::put` copies
alone took 84.0 M. They are gone.

## Compiler gaps

Each repro is a standalone file, built the way the harness builds ports:
`vesper build --lib --lib-name X --target x86_64-unknown-linux-gnu X.ves -o X.o`,
then `objdump -d X.o`.

### 1. Scalar locals live in stack slots, zeroed twice (largest)

```vesper
fn pick(a: u64, b: u64) -> u64 {
    if a > b {
        a
    } else {
        b
    }
}

pub fn pace(bw: u64, gain: u64, floor: u64) -> u64 {
    let rate = wrapping_mul(bw, gain);
    let scaled = rate >> 3;
    let low = pick(scaled, floor);
    let high = wrapping_add(low, 1);
    high
}
```

`vesper_slots_pace` is 41 instructions for about 6 instructions of work:

- Every local and parameter, including the inlined `pick`'s, gets an 8-byte
  slot.
- The prologue stores zero to each slot twice
  (`movq $0x0,(%rsp)` twice, then the same for `0x8(%rsp)`, ...).
- Each parameter is spilled to its slot and reloaded before use.
- Each result is stored to a slot that nothing reads.

BBR's hot functions have the same shape at scale.
`set_pacing_rate_with_gain` starts with 23 zero stores to 11 slots before
its first instruction of work. This is the #944 report's "scalar locals live
in stack slots", and it is now the main cost of the run (most of the 84.8%).

The fix is in native lowering: keep scalar locals in SSA values (Cranelift
`Variable`s) unless their address is taken, and drop the zero-initialization
of slots that are written before they are read.

### 2. Building a constant copy struct goes through per-field slots

```vesper
copy struct Inner {
    a: u64,
    flag: bool,
    when: Option[u64],
    b: u64
}

copy struct Outer {
    inner: Inner,
    seen: bool,
    x: u64,
    y: u64,
    z: u64
}

pub fn outer_new() -> Outer {
    Outer {
        inner: Inner {
            a: 0,
            flag: false,
            when: Option::None,
            b: 0
        },
        seen: false,
        x: 0,
        y: 0,
        z: 0
    }
}
```

`vesper_empty_outer_new` is 87 instructions to write 9 zero words. Each
nested aggregate is built in its own zeroed temporary and then copied word
by word into the next one, and finally into the result. BBR's
`ack_progress_new` (21 words, with a nested `RateSample`) cost about 200
instructions per ACK. That was 5.96% of the run before the port cached it.
The same pattern runs wherever the port writes a struct literal or a
`let mut rs = p.rs` copy (for example `finish_ack` and `record_send`'s
`PacketDeliveryState` result).

The fix is to build aggregates in their final place (the result pointer or
the destination slot), and to fold constant aggregates.

### 3. Small arithmetic helpers stay calls

After the inliner, the calls left in the Vesper code are:

- `mul_gain`, 23 call sites
- `bw_volume`, 14
- `product_gt`, 10
- `quantization_budget` and `div_wide_sat`, 9 each
- `start_probe_bw_down`, 9
- and others

`mul_gain` is `if x == infinity() { infinity() } else { mul_div_sat(x, gain, 1000) }`.
With `mul_div_sat`'s fast path inlined into it, it is over the 48-unit
small-callee budget. The #944 lane found no gain from raising the budget
while gap 1 made every inlined local cost a slot. Once gap 1 is fixed,
inlining these will pay off. They are 2.6% (`mul_gain`) and 3.6%
(`bw_volume`) of the run as functions, plus their call overhead in the
callers.

The repro is BBR's `arith.ves` itself: `mul_gain` and `bw_volume` over
`mul_div_sat`.

### 4. Option and Result arguments still convert at the boundary

`on_ack`'s Rust glue spends 163 instructions per ACK, for these reasons:

- `bbrv3_ack_step` takes `Option[RttSample]` and `Option[AckedPacket]`, and
  v3 converts those (#940: "Tuples and copy enums still convert").
- Each packet's `AckedPacket` and `PacketDeliveryState` is converted from
  the public Rust event types with `from_value`.
- Each `Result<(), BbrFault>` is decoded.

The public API takes Rust values, so some conversion is inherent. A mirror
for `Option[T]` of a mirrored `T` (tag word plus payload mirror) would remove
the Option part. This is 8.5% of the run, including the binding wrappers.

### Not a gap any more

- `div`/`pow2`: the checker's non-zero-divisor fact and `rotate_left` made
  them handler-free (#944's "callees with handlers").
- `mul_full`: `mul_wide` replaced it.
- The `Bbrv3` copies: ABI v3 replaced them.
- The `Fail` across `inout` workaround (E0402): #943 and #946 fixed it.

## Tests

- `cargo test -p vesper-warp-bbr` and
  `cargo test -p vesper-warp-bbr --features vesper-port` pass, in both debug
  and release. That covers 36 unit tests, plus the `divergence`,
  `lean_model` (static audit, since `VESPER_LEAN` is unset here),
  `properties`, `simulator` and `zero_alloc` tests.
- The divergence check passes: with `--features vesper-port-divergent`, the
  three named tests fail and `inflight_at_loss_solves_for_the_threshold_crossing`
  passes.
- `cargo +1.100.0 clippy --all-targets -p vesper-warp-bbr -- -D warnings`
  passes, with and without `--features vesper-port`.
- `cargo fmt --check -p vesper-warp-bbr` and `vesper fmt --check` on
  `vesper/src` pass.
- macOS, Windows, Bazel and the Lean differential run in CI on the PR.

## Parity

The port does **not** meet the bar. The bar is an ABBA 95% interval that
includes 1.0x or lies below 1.05x. This lane's final intervals are 5.21x to
5.92x and 4.27x to 6.10x. Rust stays the default build, and nothing was
deleted.
