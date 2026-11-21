# Self-hosting SH4 lane 14: freestanding images in Vesper (part 1)

Claim #1288. Prompt: `docs/agent-prompts/self-host-sh4/14-freestanding.md`.
Package: [`packages/vesper-codegen-freestanding`](../../packages/vesper-codegen-freestanding/README.md)
(`@vesper/codegen-freestanding`). Test: `tests/self_host_freestanding.rs`.

Base: stacked on #1259 (head `8ac0f94`); origin/main was `e73cfd6`. Rust stays
the default build; no Rust file changed.

## What part 1 ports

Everything in the freestanding image builder that needs no Cranelift
emission, so it can land before lane 1's lent builders:

- **Entry stubs** (`src/freestanding/stub.rs`): the assembler (labels,
  fixups, literals, alignment fill), the RISC-V and AArch64 stubs for every
  exit device, `exit_stack_bytes`, and the relocatable object `object::write`
  makes of them. That includes `object`'s string table layout: strings
  sorted by their reversed bytes, largest first, with tail merging
  (`.text.boot` is stored inside `.rela.text.boot`).
- **The static linker** (`src/freestanding/link.rs`): the relocatable ELF64
  reads it makes through the `object` crate (section kinds, symbol sections,
  relocation chains), piece layout, symbol resolution, the five relocation
  types, every typed error, and the output sections and symbols.
- **The trap-table fill** (`resolve_traps`, `patch_trap_table` in
  `src/native_freestanding.rs`).
- **ELF64 executables** (`src/freestanding/elf.rs`): `write_executable` and
  `read_executable`.
- **The linker script** (`src/freestanding/script.rs`).
- **`checks::compare_code`**.

## Results

Every image the freestanding tests build (the 11 programs of
`tests/freestanding.rs`, including the abort fixture, on both targets: 22
images) is rebuilt from its parts by the Vesper package:

| Check | Result |
|---|---|
| Stub objects, 22 images + 4 exit devices x 2 targets x 3 `e_flags` | byte-identical |
| Linked executables with the trap table filled | byte-identical (22/22) |
| Entry, layout, symbol table, trap PCs | equal (22/22) |
| Linker scripts, 22 images + odd stacks, board names and bases | identical |
| `read_executable` on the images, 7 truncations each and a bad header | equal, errors included |
| `compare_code` on every function of `vesper.o` and `runtime.o` | accepts each; rejects a changed byte, a short code and a missing symbol |
| Linker errors (16 edited inputs: no stub, two stubs, no runtime, duplicate symbols, other target, RAM too small, image below RAM, `e_flags`, `ET_EXEC`, bad magic, ELF32, unknown relocation, `R_RISCV_RELAX`, null-symbol relocation, offset past the section) | the same `FreestandingError`, fields included |
| Stub constants (mask immediates, exit stack bytes) | equal to Silicon's model |

`Object` errors whose Rust message is the `object` crate's own text (a file
that is not ELF, or is ELF32) are compared by variant only; every message
the freestanding code writes itself is compared exactly.

The test bites: moving `.data`'s alignment or renaming a linker symbol in
the Vesper linker fails both the image and the error comparisons.

### Cost

Link of all 22 images, 50 rounds (1,100 links), optimized JIT, ABBA wall
clock on this host. The Vesper side is the call on a prebuilt request with
the result copied out of the JIT's sequence; the Rust side is `link::link`
(without the trap fill).

| | Time | Ratio |
|---|--:|--:|
| Rust `link::link` | 17.2 ms | 1.00x |
| Vesper `freestanding_link` | 26.6 ms | 1.55x |

(1.86x before the name lookups used `find_byte`, slice `eq` and a hashed
global table, with the request built inside the timed loop.) Callgrind
counts are not in this part: under valgrind the test spends its time
building the 22 Rust images with Cranelift before the measured loop, and
the run passed 28 minutes. Part 2 caches the objects to measure Ir.

The link is a small part of a freestanding build: the parity bar for this
lane is the whole build with the family swapped, which needs part 2.

## Decisions

- **One request in, one result out.** Every entry takes one `in Slice[u8]`
  (little-endian words and length-prefixed strings) and returns one
  `ChunkSeq[u8]` whose first word is 0 or a `FreestandingError` code with
  its fields (`error.ves`). Word functions take at most three words, and
  the linker needs several objects and trap entries.
- **Objects are parsed in place.** Sections and symbols are rows over the
  request's bytes; only piece data (which relocations patch) and symbol
  names are copied.
- **The Silicon mask immediates are constants** (mstatus.MIE mask 8,
  DAIFSet 0b1111), checked by the test against `stub::riscv_mie_mask` and
  `stub::aarch64_daifset_mask`, which derive them from Silicon's interrupt
  model. Silicon is not ported.

## Compiler gaps

1. **Module constants are not importable on this base.** `use
   self::util::{NONE64}` is E0111 "module has no item". `main` has
   importable consts (#1239), but #1259's base does not. Workaround:
   modules keep their own `const`s and export functions (`machine_of`,
   `pf_rx`, `classify`, `fail_*`). Repro:

   ```text
   // a.ves
   pub const N: u64 = 1;
   // lib.ves
   use self::a::{N};   // E0111 on 8ac0f94
   ```

2. **A conditional slice view cannot be bound.** `let name = if ok { b[i..j] }
   else { b[0..0] };` is E1521; a plain `let name = b[i..j];` is accepted.
   Workaround: pick the bounds first.

3. **`s.as_bytes().len()` on a `Str` argument is E1531** ("a string view is
   a second-class borrow") with E0202 and E0206 cascades; `let n =
   s.as_bytes(); n.len()` is accepted. Repro: `fn f(in s: Str) -> u64 {
   s.as_bytes().len() }`.

4. **Division needs a range fact a `let` shadow does not give.** `let a = if
   a == 0 { 1 } else { a }; v % a` needs `Fail`; `if a <= 1 { return v; }`
   first proves it. Not a bug, but the shadow form is the natural
   transliteration of `a.max(1)`.

## Stage 1 guard

No file under `src/verified_codegen/**`, `src/ir.rs`, `ir_opt`, `lean/**`,
`bootstrap/**` or `packages/vesper-compiler/**` changed, and no Rust file
changed at all. The bootstrap-stage1 workflow passed on this branch at
`377b04f` ([run 193](https://github.com/robot-head/vesper/actions/runs/44862580217)):
the Stage 1 digests equal `bootstrap/stage1.lock`.

## Left

- **Part 2, after lane 1's first PR (#1275):** the runtime object
  (`runtime.rs`: putc/puts/decimal/hex printing, the trap handler and the
  trap table as Cranelift IR), the floating-point check and Silicon's stack
  and interrupt analysis (`checks.rs`), the entry compile in `build_image`,
  CLIF and object comparison, and callgrind numbers.
- **Windows objects** (`src/native_windows.rs`): the `.xdata`/`.pdata`
  writer over Cranelift's `UnwindInfo`, entry handling, the COFF
  `inspect` summary and the linker command lines. The unwind writer reads
  Cranelift's unwind records, so it waits for lane 1 too; `inspect` and
  `link_args` can go first.
- **The swap:** the hybrid module `codegen_freestanding` and its
  `freestanding` hook, the full suite on all four platforms, and the SH4
  scoreboard row.
- A seeded fuzz of the linker over damaged objects; the targeted cases
  above cover each error path.
