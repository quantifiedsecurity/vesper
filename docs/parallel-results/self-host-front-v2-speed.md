# Self-hosting SH1 lane 10: front end v2 lexer speed and editor mode

> The [manuscript](../manuscript/README.md) is the sole specification.
> Confirm live ownership on the issue board before acting.

Claim: #1153. Lane prompt:
[10-front-v2-speed.md](../agent-prompts/self-host-sh1/10-front-v2-speed.md).
Base: `main` at `e29e63d` (the SH1 plan, #1158).

This lane lands in two parts. Part 1 (this report's first half) is the lexer:
speed, the table moves of implementation step 1, and the explicit-limit
entry points that the fmt port (lane 11, #1154) asked for. Part 2 is the
editor mode (steps 3 and 4) and adds its own sections here.

## Part 1: what changed

All changes are in `packages/vesper-front` and the two front end v2 tests.
The Rust front end (`crates/vesper-syntax`) is unchanged and stays the
default; `parser.rs` and `lexer.rs` are not touched, so there is nothing to
mirror.

### Lexer (`src/lex.ves`, `src/words.ves`)

- **One word per token start.** The loop reads the eight bytes at the
  token start once (`word_at`, a `try_read_u64_le`). The first byte, the
  next two bytes of punctuation and the identifier prefix all come from
  that word, so no token reads a byte through `try_get`.
- **A fixed dispatch table.** `BYTE_INFO: [u64; 256]` gives each first byte
  its class (whitespace, `/`, lower, upper, digit, `"`, punctuation, `.`)
  and, for punctuation, the one-byte kind and the two two-byte tokens it can
  start as (second byte, kind) pairs. It replaces the `is_space`/range
  tests and the 26-arm `punct` match. The table is generated from the
  `#[token]` rules; a `const` array is laid out once in read-only data.
- **SWAR runs.** Whitespace and identifier runs are classified eight bytes
  at a time on that word (`non_space`, `non_ident`, then
  `trailing_zeros_u64`). Only a run longer than eight bytes falls back to
  the `find_not_in` byte-class scan. Most identifiers and almost all
  whitespace end inside the first word.
- **Keywords by perfect hash.** `keyword_word` looks the identifier's
  word up in `KEYWORDS: [u64; 32]` at `(w * M) >> 59`: one multiply, one
  load and one compare, instead of a chain of `is(.., "fn".as_bytes())`
  byte loops. Contextual words (`lower_word_of`) are a `match` on the same
  word, read once per new symbol.
- **The common kinds in the loop.** Identifiers, punctuation and whitespace
  are read inside the `lex_tokens` loop; numbers, strings, comments, `.`
  and errors go to `next_token_rest`. The single-call `next_token` was over
  the inliner's single-site budget (426 > 240), so every token paid a call
  with its `(kind, end)` tuple returned through memory.
- **Interning in the same pass.** Identifiers are interned as they are
  pushed instead of in a second `classify` pass. The interner is an open
  addressing table of two-word slots (a tag word with the symbol, its
  contextual word, its capped length and 16 hash bits, then the first eight
  bytes), so a hit costs one hash multiply and two loads from one cache line.
  It starts at 256 slots and doubles at half load, instead of sizing the
  table to the source length (a 1 MiB source no longer zeroes 4 MiB). Longer
  identifiers compare their tails eight bytes at a time.
- **Fewer token rows.** A token's end is the next token's start (the table
  covers the source without gaps), so the `end` row is gone (`token_end`).
  The contextual word moved into the high byte of the symbol row
  (`token_sym`, `token_word`). The lexer pushes two rows per token, or three
  with symbols, instead of five.

### Tables in `src/parse.ves`

The binary precedence table is `PRECEDENCE: [u64; 128]` by token kind
instead of a 24-arm `match`. Token and node kinds stay functions
(`k_lower()`): a `const` cannot be named from another file of the package
(compiler gap 1), and `match` arms cannot name constants either.

### Explicit limits (for lane 11)

- `lex_with_limits(in b, inout d, max_source_bytes, max_tokens,
  max_diagnostics) -> Tokens` is `lexer::lex_with_limits`. When Rust returns
  `Err(LexLimit)` (a limit outside `1..=HARD_*`, or a source over
  `max_source_bytes`), the table has no rows, `rejected` is set and `d` gets
  one E0003 at 0..0 whose message is the `LexLimit` with 0 retained bytes.
- `parse_tables_with_limits(in source, max_source_bytes, max_tokens,
  max_diagnostics) -> ChunkSeq[u64]` is `parser::parse_with_limits` with
  `max_depth` at `HARD_PARSE_DEPTH`, in the layout of `parse_tables`. An
  `Err` gives no tokens or nodes and that one diagnostic.
- The parser keeps `max_diag` and `max_depth` in `P`, so its diagnostic and
  depth limits follow the arguments.
- `parse_tables_with_packed_limits` is the same call with the token and
  diagnostic limits packed into one word, for the test's native host call
  (compiler gap 8).

### Tests

- `explicit_limits_match_parse_with_limits` (new) compares
  `parse_tables_with_limits` with `parse_with_limits` on every 17th
  repository file and six limit cases (300 lexical errors, 200 parse
  errors, 70,000-token sources, a source over 1 MiB, the empty source),
  under 12 limit sets: the defaults, the formatter's hard limits with 1 and
  1,024 diagnostics, a one-token cap, 7 tokens and 3 diagnostics, a 100-byte
  and a 0-byte source budget, and each limit one past its range. A changed
  `LexLimitKind` name fails it.
- The existing differential tests (repository sources, parser test corpus,
  edge cases and limits, 3,000 fuzzed sources, the backends digest and the
  leak test) pass unchanged.
- `tests/self_host_front_v2_bench.rs` gains `lex_abba_against_logos` (lex
  only, 15 ABBA rounds) and `lex_once_for_callgrind`.

## Part 1: numbers

Corpus: the 899 repository `.ves` files at the branch head (4.09 MB,
1,354,749 tokens with trivia, 263,443 nodes), on a 4-core x86-64 cloud host,
release build, medians of `abba_against_the_rust_parser` (7 rounds) and
`lex_abba_against_logos` (15 rounds). Wall clock on these hosts moves by up
to 30% between runs. Base numbers are the base package measured with the
same bench binary on the same corpus.

| Phase | Rust | Vesper before | Vesper after | Before / Rust | After / Rust |
|---|---|---|---|---|---|
| Lex (`lexer::lex` vs `lex_count`) | 14.2-15.2 ms | 44.1-44.8 ms | 22.9-24.4 ms | 3.12-3.18x | 1.61-1.68x |
| Lex without symbols (`lex_raw_count`) | same | 27.0-27.3 ms | 15.4-16.7 ms | 1.90-1.94x | 1.08-1.11x |
| Parse (`parse` vs `parse_counts`) | 285-306 ms | 108 ms | 88 ms | 0.38x | 0.29x |

Instructions (callgrind, `Ir`): the package built with `vesper emit --emit
object --entry lex_count` (and `lex_raw_count`), linked into a C harness that
calls the entry once per corpus file; the harness alone is 2.7 M and is
subtracted. Rust is `vesper_syntax::lexer::lex` inclusive in
`lex_once_for_callgrind`.

| Lexer | Ir | / Rust |
|---|---|---|
| Rust (logos) | 83.6 M | |
| Vesper `lex_count`, before | 601 M | 7.2x |
| Vesper `lex_count`, after | 315 M | 3.8x |
| Vesper `lex_raw_count`, before | 399 M | 4.8x |
| Vesper `lex_raw_count`, after | 218 M | 2.6x |

Where the instructions went, from the per-instruction profile of the
optimized object (`callgrind --dump-instr=yes` against `objdump -d`):

- The raw lexer's loop is about 140 instructions per token against
  logos's 62. About 15 of them per token are the `Option` of
  `try_read_u64_le` stored to and reloaded from a stack slot (gap 5); the
  SWAR class tests are about 30 for whitespace and 40 for identifiers; the
  two row pushes are about 10 each (header, length and capacity reloads).
  The time ratio (1.1x) is lower than the instruction ratio because these
  are independent, well-predicted instructions, while logos's DFA is a chain
  of dependent table loads.
- Classification (interning and contextual words, which logos does not do;
  the Rust parser compares text instead) costs about 7.5 ms, 100 M
  instructions for 310,000 identifier tokens: about 165 per `intern` call,
  most of them saves, spills and the long-identifier view setup, and about
  50 in the loop. Replacing a six-way `||` of `==` with a bit-mask test in
  `named_kind` alone took the full lexer from 25.1 to 22.9 ms (gap 4).

### Acceptance

Lexer time at or below 1.0x of Rust is met for the lexer that does logos's
work (1.08-1.11x, within this host's noise of parity by wall clock, 2.6x by
instructions) and not for the lexer that also interns every identifier
(1.61-1.68x). The gap is the profile above and the repros below. The whole
Vesper parse, which uses the interned lexer, is 0.29x of the Rust parse.

## Stage 1 guard

No file under `src/verified_codegen/**`, `src/ir.rs`, `ir_opt`, `lean/**`,
`bootstrap/**` or `packages/vesper-compiler/**` changed. The
`bootstrap-stage1` workflow run on this branch (`bootstrap/run.sh stage1
--reproduce --outcome`) is linked from the PR.

## Compiler gaps

1. **A `const` cannot be named from another file of its package.** Token
   and node kinds stay functions.

   ```text
   // src/k.ves
   pub const K_A: u8 = 3;
   // src/lib.ves
   use self::k::{K_A};        // E0111: module self::k has no item 'K_A'
   pub fn f(x: u8) -> bool { x == K_A }   // without the use: E0201 unknown name
   ```

2. **A call's narrow result kind does not bound an array index.** A `u8`
   binding indexes a `[T; 256]` without a check, but a call that returns
   `u8` raises `Fail[RuntimeError]` unless masked.

   ```text
   const T: [u64; 256] = [0; 256];
   pub fn f(w: u64) -> u64 { T[wrapping_u8(w)] }   // E0401: unhandled effect 'Fail'
   pub fn g(w: u64) -> u64 { T[w & 255] }          // accepted
   ```

3. **A shift by a variable amount raises.** `x >> s` with a non-constant
   `s` is checked (E0401 in a total function), so total code writes
   `wrapping_shr_u64(x, s)`. Known; listed for the census.

4. **`||` of comparisons materializes every boolean.** Each `k == c` arm of
   a six-way `||` became a compare, a `sete`, a zero extension, a test and
   a branch (about 25 instructions on the common path); the bit-mask form is
   four.

   ```text
   fn named(k: u8) -> bool { k == 3 || k == 4 || k == 16 || k == 22 || k == 24 || k == 25 }
   fn named_mask(k: u8) -> bool { k < 32 && (wrapping_shr_u64(54591512, widen_u32(k)) & 1) != 0 }
   ```

5. **`Option` results of `try_*` builtins go through the stack.** The tag
   and payload of `b.try_read_u64_le(at)` are stored to a stack slot and
   reloaded before the `match`, about ten instructions on every read even
   when the `Some` arm is the only live path.

   ```text
   fn word(in b: Slice[u8], at: u64) -> u64 {
       match b.try_read_u64_le(at) { Option::Some(w) => w, Option::None => 0 }
   }
   ```

6. **A large single-call helper stays a call, and its tuple result goes
   through memory.** `next_token` (cost 426 after its own inlining) stayed a
   call from the lexer loop: a prologue saving five callee-saved registers
   and a `(u8, u64)` result written to and read from the stack, about 40
   instructions per token. The fix was moving the hot arms into the loop by
   hand. A hot/cold split of over-budget single-site callees, or tuple
   results in registers, would do it automatically.

7. **Slice views in an inlined helper are set up before the branch that
   needs them.** With `same_bytes(in b, s1, s2, n) = b[s1..s1 + n].eq(in
   b[s2..s2 + n])` inlined behind `if n > 8`, both views' bounds and
   pointers were computed on every `intern` call (about 40 instructions),
   although almost every identifier is at most 8 bytes. The tail compare is
   now a word loop without views.

8. **Native host calls take at most three words.** `native::WordFunction`
   refuses `parse_tables_with_limits` (`in Str` plus three `u64`s: "a word
   function takes at most three words"), so the test calls a packed-limits
   shim. This widens SH0 gap 7 (host calls with `Str` arguments).

9. **`vesper fmt` moves a trailing comment to the next line.** `1, // copy`
   inside an array or `95 => 21, // _` in a `match` is formatted as the
   element, then `// copy` on its own line, so the comment reads as the next
   element's. The generated tables put each comment above its element.

## What is left

- Part 2: the editor mode (recovery sites, `BLOCK`, `ERROR_NODE`,
  `INCOMPLETE`, header notes and macro sources) and its differential test.
- The rest of the lexer gap is the interning cost and gaps 5 and 6 above.
