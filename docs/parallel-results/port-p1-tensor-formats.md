# Port wave P1 lane 8: vesper-tensor formats, quantization and sampling

Claim [#905](https://github.com/robot-head/vesper/issues/905). Base `main` at
69c79b9 (2026-11-18). Plan: `docs/agent-prompts/port-p1/08-tensor-formats.md`.

This is the first part of the lane. It splits the crate into the seed copy
and the port-backed build, and ports the element and block formats, every
quantizer, the row decoder of `Matrix`, sampling and the FP32 kernels that
sampling and decoding use. The tokenizer, the container parsers, the K/V pool
and the shape planner are the second part (see "Not ported yet").

## Builds

`vesper-tensor` is in the compiler's dependency closure (`src/tensor_types`
evaluates source tensor types on the host kernels), so it follows the
seed-copy rule of `docs/maintenance/porting-a-crate.md`:

| Package | Path | What it is | Who links it |
|---|---|---|---|
| `vesper-tensor-seed` | `crates/vesper-tensor/seed` | the Rust build of `../src`, library `vesper_tensor`, no build script | the compiler, `vesper-kinetic`, `vesper-gpu-compute`, `vesper-fusion` |
| `vesper-tensor` | `crates/vesper-tensor` | `build.rs` links the Loom package `vesper/` and sets `cfg(vesper_port)` | its own tests only |

The plan's default would have given the port to the dependents outside the
compiler's closure (`vesper-gpu-compute`, `vesper-fusion`). The owner's parity
rule (2026-11-17) overrides it: a slower port stays non-default, so every
dependent links the seed by rename and nothing changes for them.
`tests/build_kind.rs` checks which build each package is
(`vesper_tensor::IMPLEMENTATION`).

## API inventory (re-measured before porting)

Public modules and the part of each this PR ports:

| Module | Public API | This PR |
|---|---|---|
| `formats` | `QK_K`, `Q4_K_BYTES`, `MXFP4_BLOCK`, `MXFP4_BYTES`, `E2M1_DOUBLED`, `q4_k_scale_min`, `dequantize_q4_k_block`, `check_q4_k_block`, `nearest_int`, `make_qkx2_quants`, `quantize_q4_k`, `quantize_q4_k_block`, `e8m0_half`, `check_mxfp4_block`, `dequantize_mxfp4_block`, `quantize_mxfp4`, `Fp8 {max_finite, decode, decode_exact, is_finite_code, encode_saturating}`, `quantize_fp8` | every function (constants and `Fp8::decode`'s table stay; the table is built from the ported `decode_exact`) |
| `quant` | `BLOCK`, `Q8_0_BYTES`, `Q4_0_BYTES`, `encode_fp16`, `encode_bf16`, `decode_bf16`, `WeightFormat`, `Matrix` (`f32`, `quantized`, `from_bytes`, accessors, `quantize`, `to_f32`, `dequantize_row`, `matvec`, `matvec_with`, `matmul_with`), `dequantize_q8_0_block`, `dequantize_q4_0_block`, `quantize_q8_0`, `quantize_q4_0` | the codecs, the block functions, the quantizers, the payload check of `from_bytes` and the row decoder behind `to_f32`, `dequantize_row`, `matvec*` and `matmul_with` |
| `sampling` | `Rng {next_u32, next_f32}`, `SamplerConfig`, `Sampler {new, config, rng, probabilities, sample}`, `sample_mult`, `sample_top_p` | `Rng`, `Sampler::sample`, `sample_mult`, `sample_top_p` |
| `ops` | `rms_norm`, `rope`, `rope_half`, `silu`, `swiglu`, `swiglu_matvec(_with)`, `softmax`, `argmax`, `add_assign` | `rms_norm`, `silu`, `swiglu`, `softmax`, `argmax`, `add_assign` |
| `tokenizer` | `BOS`, `EOS`, `MAX_PROMPT_BYTES`, `Encoder {new, id, encode}` | second part |
| `gguf`, `safetensors`, `json` | container readers and writers | second part |
| `kv` | `KvPool` and its types | second part |
| `shape` | `Dimensions`, `CheckedShape`, GQA helpers | second part |
| `host` | the small FP32/Q4_0 graph executor, `decode_fp16` | outside the lane's boundary (`src/host.rs`) |
| `attention`, `llama` | paged GQA attention, model loading and inference | second part (pure tensor parts), host glue stays |
| `simd`, `mmap` | `std::arch` kernels, file mapping | stay Rust (F11 #587; memory mapping) |

Unit tests in the crate: `json.rs` (1) and `kv.rs` (3). Neither module is
ported yet, so both run unchanged in both packages.

## What was ported

The Loom package `@vesper/tensor` (`crates/vesper-tensor/vesper`, 1,509
lines, 36 exported functions):

| File | Rust source | Contents |
|---|---|---|
| `lib.ves` | all | the exports, `Fault`, the `Libm` import, and the kernels that call `exp` (softmax, SwiGLU, `Sampler::sample`) |
| `num.ves` | `quant.rs`, `host.rs` | binary16 and BF16 encode/decode, float predicates |
| `ocp.ves` | `formats.rs` | FP8 E4M3FN/E5M2 decode and saturating encode, E8M0, GGML `nearest_int` |
| `qk.ves` | `formats.rs` | Q4_K: scale/min unpacking, dequantization, `make_qkx2_quants`, `quantize_row_q4_K_ref` |
| `rows.ves` | `quant.rs`, `formats.rs` | Q8_0, Q4_0, MXFP4 blocks and quantizers, FP8 matrices, the row decoder, the payload check |
| `sampling.ves` | `sampling.rs`, `ops.rs` | xorshift64*, `sample_mult`, `sample_top_p` (heapsort of packed candidates), argmax, RMSNorm, residual add |

The Rust glue is `src/port.rs`. Each ported Rust function keeps its signature
and has a `#[cfg(vesper_port)]` twin that forwards to the binding
(`port/binding.rs`); the Rust body stays under `#[cfg(not(vesper_port))]` for
the seed.

Decisions:

- **f32 at the boundary.** The export ABI carries `f64` but not `f32`. An
  `f32` crosses as its bit pattern: a `u32` scalar, and `&[f32]` lent as
  `&[u32]` with `bytemuck::cast_slice` (no copy; `u32` slices are lent in
  place). The Vesper side converts with `from_bits_f32`/`to_bits_f32`.
- **Bit-exactness.** Every operation is FP32 in the Rust order, without fused
  multiply-add, so every result equals the Rust build bit for bit, and
  `tests/reference_fixtures.rs` (llama.cpp Q4_K bytes, gguf-py Q8_0/Q4_0,
  ml_dtypes FP8 and MXFP8 codes) passes against the port. No ported path
  uses `sin` or `cos`, so the #893 accuracy gap does not apply.
- **`exp` stays the platform's.** The Rust build's softmax and SiLU call
  `f32::exp` and document no cross-ISA claim. Vesper's `exp_f32` is fdlibm in
  f64 rounded once, which can differ in the last bit and change a sampled
  token. The port imports the Rust `f32::exp` through a `.vsys` library
  (`Libm`, shim in `src/port.rs`), so probabilities stay identical. Moving to
  Vesper's `exp_f32` is a one-line change once the crate fixes its own
  `exp`.
- **Errors.** Vesper raises `Fault {Shape, NonFinite, Limit}`; the glue maps
  each to the `Error` the Rust build returns at that call, with the same
  message. `ArithOverflow` and `RuntimeError` cannot occur on valid calls;
  where the Rust function returns `Result`, they map to `Error::Overflow` and
  `Error::Malformed`, and where it panics on bad input (short slices), the
  port panics too.
- **Row pieces.** The port decodes a row in pieces of 256 values (eight
  Q8_0/Q4_0/MXFP4 blocks, one Q4_K block) per call instead of one block, to
  cut boundary crossings. The crate's contract for `row_chunks` is that every
  piece but the last is a multiple of eight, which keeps chunked SIMD sums
  equal to whole-row sums, and the batching and SIMD tests pass.
- **Top-p candidates.** The Rust build sorts `(f32, u32)` pairs. The port
  packs each candidate as `(!bits(p) << 32) | id`: probabilities are
  non-negative, so ascending words are descending probabilities with ties
  by ascending id, the order the Rust build uses. `Sampler` keeps a
  preallocated `Vec<u64>` in the port build, so `Sampler::sample` still
  allocates nothing (`tests/steady_state_alloc*.rs` pass). The standalone
  `sample_top_p` keeps its `&mut Vec<(f32, u32)>` parameter and borrows a
  word buffer per call.

## What stays in Rust

| File | Reason | Unblocked by |
|---|---|---|
| `src/simd.rs` | `std::arch` kernels | source `VVec` (F11, #587) |
| `src/mmap.rs` | the one `memmap2::Mmap::map` call | a `.vsys` mapping surface (second part) |
| `src/host.rs` | outside the lane's exclusive boundary; `host::decode_fp16` stays Rust, the port has its own copy | the lane that owns `host.rs` |
| `ops::rope`, `ops::rope_half` | `powf`, `sin`, `cos` from the platform library; Vesper's `sin`/`cos` lack Payne-Hanek reduction (#893) and would change bits | a `.vsys` import like `Libm`, or Vesper transcendentals the Rust build adopts |
| `tokenizer`, `gguf`, `safetensors`, `json`, `kv`, `shape`, `attention`, `llama` | not ported yet | the second part of this lane |
| `seed/` | the compiler's copy | self-hosting Stage 2 |

The crate's `unsafe` count is unchanged in the Rust build. The port-backed
build adds the generated binding (`port/binding.rs`, `unsafe` FFI calls) and
one `#[unsafe(no_mangle)]` shim function; the Vesper source has none.

## Unit tests and test changes

- No ported module has `#[cfg(test)]` tests. The four in `json.rs` and
  `kv.rs` run in both packages.
- `tests/model_formats.rs`, `tests/lean_kv.rs` and
  `tests/steady_state_alloc_batch.rs` found their files through
  `CARGO_MANIFEST_DIR`; they now step out of `seed/` when the seed runs them.
- New: `tests/build_kind.rs` (which build each package is) and
  `tests/abba.rs` (the benchmarks, and a non-ignored check that both builds
  produce the same bytes and values).

All 100 tests pass against both packages (`cargo test -p vesper-tensor -p
vesper-tensor-seed`, Linux x86_64).

## Divergence checks

`build.rs` `DIVERGENT` (feature `vesper-port-divergent`) makes one edit per
ported Rust module; `port/test-port.sh` (run by
`scripts/test-port-harness.sh` on macOS and Windows) requires each named test
to fail and an unchanged test of the same file to pass:

| Ported module | Edit | Fails | Still passes |
|---|---|---|---|
| `formats.rs` (Q4_K, `qk.ves`) | wrong high bits of a packed minimum | `q4_k_bytes_equal_llama_cpp` | `gpt2_case_file_is_well_formed` |
| `formats.rs` (FP8, `ocp.ves`) | normal codes get the next exponent | `fp8_channel_block_and_mxfp8_codes_equal_ml_dtypes` | same |
| `quant.rs` (`rows.ves`) | Q4_0 rounds with 8.25 | `q8_0_and_q4_0_weights_equal_gguf_py` | same |
| `sampling.rs` (`sampling.ves`) | the crossing token leaves the nucleus | `sampling_follows_the_distribution_and_the_nucleus` | `rng_matches_run_c_xorshift` |
| `ops.rs` (`lib.ves`) | softmax divides by twice the sum | `silu_swiglu_softmax_argmax` | `rope_matches_f64_rotation_and_rejects_odd_widths` |

Checked locally: the divergent build fails exactly these and passes the
controls. Bazel (Linux) builds the port and runs every test against both
packages, but does not build a divergent object yet: the port-backed crate
links its object through its build script, so a divergent Bazel build needs a
second crate instance. The divergence check runs on macOS and Windows CI.

## Benchmark

`tests/abba.rs`, `vesper-bench` `run_abba`, 20 epochs x 4 operations per
arm, release build, one process holding both builds (`seed` is a
dev-dependency). Cloud container, Intel Xeon @ 2.10 GHz, 4 vCPU, rustc 1.97,
port compiled by the in-tree `vesper` at 69c79b9 (Cranelift, inliner on).

| Workload | Rust median | Vesper median | Vesper/Rust median | 95% interval |
|---|---|---|---|---|
| dequantize Q4_K 64x4096 (`to_f32`) | 262.0 us | 1024.8 us | 3.66x | [3.62, 3.73] |
| dequantize Q8_0 64x4096 (`to_f32`) | 256.2 us | 1026.1 us | 4.00x | [3.98, 4.04] |
| matvec Q8_0 64x4096, scalar kernel | 193.8 us | 1146.1 us | 5.85x | [5.50, 6.10] |
| quantize Q4_K 16x4096 | 5493.5 us | 22665.7 us | 4.13x | [4.09, 4.23] |
| sample, T 0.8, top-p 0.9, vocab 32,000 | 392.5 us | 1748.4 us | 4.67x | [4.46, 4.78] |

Every workload is past the plan's 2x stop rule. Under the owner's parity rule
the port stays non-default and the Rust code stays. Where the time goes, from
the source (not profiled yet): every element read and write is a checked
`s[i]`/`set` with a failure status, every float goes through
`from_bits_f32`/`to_bits_f32`, the decode loops are scalar where the Rust
build auto-vectorizes, and `matvec` adds a boundary crossing per 256 values
and a decoded buffer. BPE encoding is not ported yet, so it has no
benchmark in this part.

## Compiler and tooling gaps found

1. **`.vsys` imports in packages depend on file order.** `build --lib` on a
   Loom package resolves `// @export vsys` libraries only if `lib.ves` sorts
   first among the package's `src/*.ves` files; with a module named `half.ves`
   it fails with `unknown effect 'Ffi'`. The modules here are named to sort
   after `lib`. A `.vsys` library is also visible only in the root module.
2. **No `f32` in the export ABI** (scalars, slices, results).
3. A literal range `for i in 0..4` is `i64`; the port starts ranges at a
   typed `u64_zero()`.
4. `Fail::raise` in a row with several `Fail` types raises the last one, so
   `Fault` is last in every row.

## Platforms run

Locally: Linux x86_64 (both packages, the divergence check, clippy 1.100,
`vesper fmt --check`). CI: the four required checks on the PR.

## Was the Rust code deleted?

No. The port is 3.7x to 5.9x slower than the Rust build, and the compiler
links the seed until Stage 2.

## Not ported yet (second part of the lane)

`tokenizer.rs` (BPE encode, `tokenizer.json`), `gguf.rs`, `safetensors.rs`
and `json.rs` over byte slices, `kv.rs`, `shape.rs`, the pure tensor parts of
`attention.rs` and `llama.rs`, the memory map as a `.vsys` surface, and the
BPE ABBA benchmark.
