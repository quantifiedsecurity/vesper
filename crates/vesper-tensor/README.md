# vesper-tensor

Bounded host tensor planning, the paged K/V cache, the decoder kernels and
greedy CPU inference for small Llama-architecture models (manuscript §28).
The crate documentation (`src/lib.rs`) describes the modules.

**Frozen for the Vesper port** (port wave P1 lane 8, claim
[#905](https://github.com/robot-head/vesper/issues/905)): the Rust code
takes bug fixes only. Two packages build it:

- `vesper-tensor-seed` (`seed/`) is the Rust build. The compiler links it,
  and so does every other dependent until the port reaches performance
  parity.
- `vesper-tensor` is the port-backed build: the element and block formats,
  the quantizers, the row decoder of `Matrix`, sampling and the FP32 kernels
  forward to the Loom package `@vesper/tensor` in `vesper/` (`src/port.rs`).

`port/test-port.sh` runs every test against both packages and checks that a
deliberately divergent port fails. The lane report is
`docs/parallel-results/port-p1-tensor-formats.md`.
