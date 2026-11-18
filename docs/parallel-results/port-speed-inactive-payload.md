# Port speed lane 4: inactive variant payload words are not zeroed

Claim #1024. Base `72209eb`. Prompt:
[04-inactive-payload.md](../agent-prompts/port-speed/04-inactive-payload.md).

**Result.** The manuscript now says that the words past an enum's active
variant are unspecified, and that no operation reads them. The native
optimize path stops zeroing them. Ignis `check` drops from 45 to 36
instructions, because its 9 zero stores are gone. On the ports the effect
is small: BBR runs 0.40% fewer instructions and Ignis 0.06% fewer. Quarry's
port binary is byte-identical. Rust stays the default build of every port
but Aura, and no port source changed.

## What changed

1. **The rule** (manuscript §16.6, *Values* bullet; flagged in the PR).
   - Enums are padded to their largest variant.
   - The words past the active variant's fields are its *inactive
     payload*. Their contents are unspecified, and no operation observes
     them.
   - Equality, ordering, hashing, `match`, drops and every boundary
     conversion read the tag first, then only the active fields.
   - The v3 paragraph adds that a word block's equality compares the
     converted values.
2. **Readers.** Each reader of a variant's payload words was audited:

   | Reader | Reads the tag first? |
   |---|---|
   | `==` (`emit_equal_inline`) | yes: it compares only the matching variant's fields |
   | Derived `Eq`/`Ord`/`Hash` | not a native reader: they are Vesper source for structs, and an enum's impl is a user `match` |
   | `match` (`pattern_condition`) | yes |
   | Drop glue (`emit_drop_glue`, box drops) | yes: they switch on the tag |
   | v1/v2 `Abi::get`, v2 handle views, `OptionMirror`, `ResultMirror` | yes |
   | v3 `Words<T, N>` cell (`vesper-port-abi`) | **no.** `PartialEq` and `Debug` compared raw words |

   `Words<T, N>` now compares canonical words (`get`, then `put` into
   zeroed words) and prints them in `Debug`. Two raw-equal cells short-cut.
   `bits()` documents that the inactive words are unspecified. This adds a
   `T: Abi` bound to those impls. Every `Words` is built through
   `Words::new`, which already needs that bound.
3. **Construction** (`data_into`, optimize path only). A constructor
   zeroes only the words inside its own fields that no store covers whole,
   which is the word of an `f32` field. Words past the variant's last field
   are left as they are. The non-optimize path (`alloc`) still zeroes
   every word.

## Tests

- `tests/native_inactive_payload.rs` runs every program in the evaluator,
  in the JIT with optimize on and off (no teardown frees, no stray frees),
  as x86-64 and AArch64 objects, and as an executable:
  - narrow variants built where an `Ok` was before: loop-carried values and
    recursive callers. They are checked with `==` against fresh values and
    against the previous value, and with `match`.
  - `ChunkSeq[Check]` slots overwritten from `Ok` to one-word variants.
  - an owning enum (`Two(ChunkSeq, ChunkSeq)`, `One(u64)`, `Nothing`)
    replaced through `inout` and consumed by `sink`.
  - a C caller of a `--lib` build passes a result buffer full of `0xdead`.
    It reads the tag and the active fields correctly, and it sees that the
    9 inactive words still hold its garbage.
- `vesper-port-abi`: `inactive_payload_words_never_decide_equality` checks
  that a mirror with garbage behind a `None` equals the one Rust built,
  `Debug` prints them alike, and a `Some` stays unequal.

## Repro

Built with `vesper build check.ves --lib`, then `objdump -d`, counting the
instructions of `vesper_check_check`. The source is the Ignis `check`
repro of [port-parity-ignis.md](port-parity-ignis.md).

| Repro | main `72209eb` | this branch |
|---|---:|---:|
| Ignis `check` | 45 (9 inactive zero stores) | 36 (none) |

## Ports

Setup:
- Machine: cloud x86-64 container, Linux 6.18, Rust 1.97 `--release`.
- Compilers: release `vesper` at `72209eb`, and the same build with this
  branch's `src/native.rs`. Each was passed as `VESPER_PORT_COMPILER`.
- Instructions: callgrind `Ir`, whole process. Ignis runs `abba measure 20`,
  Quarry `planner_abba window 20` and BBR `ack_path_abba measure 20000`.
  The Rust arm is the same example built without `vesper-port`.

| Port | Rust Ir | Port Ir, main | Port Ir, branch | Change | Ratio, main | Ratio, branch |
|---|---:|---:|---:|---:|---:|---:|
| Ignis | 116.17 M | 165.65 M | 165.55 M | -0.06% | 1.43x | 1.43x |
| Quarry | 19.47 M | 31.09 M | 31.09 M | 0 (identical binary) | 1.60x | 1.60x |
| BBR | 21.97 M | 49.45 M | 49.25 M | -0.40% | 2.25x | 2.24x |

Wall clock was not measured. Changes this small are far inside this host's
run-to-run spread, which is up to 30% for one binary
([port-gaps-rescore.md](port-gaps-rescore.md)).

## Stage 1 guard

No file under `src/verified_codegen/**`, `src/ir.rs`, `ir_opt::optimize`,
`lean/**`, `bootstrap/**` or `packages/vesper-compiler/**` changed. The
bootstrap-stage1 workflow ran on the branch:
[run 44343102154](https://github.com/robot-head/vesper/actions/runs/44343102154). The
construction change is under `operands_only`, the native optimize path.

## What is left

- Only words past the variant's last field are skipped. An `f32` field's
  word is still zeroed, because `Word<f32>` compares its canonical word.
- Wider gaps of the ports are unchanged. Ignis and BBR spend their time
  elsewhere; see the gaps table of
  [port-gaps-rescore.md](port-gaps-rescore.md).
