# vesper-ignis

GPU-independent render graph and D3D12 enhanced-barrier planner (design
section 19), with multi-backend barrier lowering, the owned command and fence
lifecycle, and a deterministic CPU recording backend.

Frozen since 2026-11-17: the Rust code takes bug fixes only while it is ported to Vesper ([#909](https://github.com/robot-head/vesper/issues/909)).

The crate has two builds of the same API. The default build runs the Rust
implementation in `src/reference/`. With `--features vesper-port` the
planner, the lowering, the lifecycle fence ledger and the recording model run
in the Vesper library in `vesper/src/` (one library split over several files,
joined in the order `build.rs` lists), called through the generated
`port/binding.rs`; `src/port/` converts between the public types and the
Vesper records. `port/check.sh` runs the tests against both builds and checks
that a deliberately divergent Vesper build fails them.
