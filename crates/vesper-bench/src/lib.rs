//! Executing host benchmark harness, not a compiler/PMU integration. Actual
//! `std::time::Instant` elapsed nanoseconds (units, not a clock resolution or
//! accuracy guarantee) include loop/callback/black_box
//! overhead. Inputs and output capacity are prepared before timing; teardown
//! and disposal run afterward. No zero-allocation/spill, cycle, precision or
//! scheduler-isolation claim is made.
//!
//! [`layout`] randomizes heap offsets, stack padding and (through a process
//! driver) environment size per paired run and tests whether a difference
//! survives; it does not relocate machine code. [`causal`] is a Coz-style
//! virtual-speedup profiler for instrumented host threads. [`wire`] is the
//! 64-byte padding-free `PmuSampleWire` record with explicit availability.
//!
//! Two builds share these sources (port wave P3a lane 5, #1040). This
//! package builds the Rust code by default; under its `vesper-port` feature
//! the build script links the Vesper port in `vesper/` and sets
//! `cfg(vesper_port)`, and the statistics, contracts, wire format and the
//! pure folds of the drivers forward to it. The clock, the callback run
//! loop, layout shims and the causal profiler's threads stay Rust in both.
//! The compiler links the frozen seed copy (`seed/`, `vesper-bench-seed`).
// The seed build stays safe. The port build links the Vesper port through
// the generated binding, which calls the C ABI.
#![cfg_attr(not(vesper_port), forbid(unsafe_code))]
#![cfg_attr(vesper_port, deny(unsafe_code))]
// Under the port, the Rust bodies the public API no longer calls stay
// compiled (they are the seed's code) but unused.
#![cfg_attr(vesper_port, allow(dead_code, unused_imports))]
mod arithmetic;
pub mod causal;
pub mod contract;
pub mod contract_gate;
mod execution;
mod hodges_lehmann;
pub mod layout;
mod signed_rank;

mod paired_difference;
mod signed_rank_confidence;
mod statistics;
mod walsh;
pub mod wire;
pub use arithmetic::*;
pub use execution::*;
pub use signed_rank::*;

pub use paired_difference::*;
pub use signed_rank_confidence::*;

pub use hodges_lehmann::*;
pub use statistics::*;
pub use walsh::*;

// Generated and committed, but kept outside src/ so formatters, which
// check every file under src/, never reflow it (see the porting recipe).
#[cfg(vesper_port)]
#[allow(unsafe_code)]
#[rustfmt::skip]
#[path = "../port/binding.rs"]
mod port_binding;
#[cfg(vesper_port)]
mod port;

/// Which build this is: `"vesper"` under the `vesper-port` feature, `"rust"`
/// for the default build and the seed copy `vesper-bench-seed` that the
/// compiler links.
pub const IMPLEMENTATION: &str = if cfg!(vesper_port) { "vesper" } else { "rust" };

/// Live sequence allocations of the Vesper port (for leak tests); always 0
/// in the Rust build.
#[doc(hidden)]
pub fn port_live_allocations() -> i64 {
    #[cfg(vesper_port)]
    return port_binding::live_allocations();
    #[cfg(not(vesper_port))]
    0
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Error {
    InvalidConfig(&'static str),
    Limit(&'static str),
    ArithmeticOverflow,
    AllocationFailed,
    ClockOverflow,
    ZeroDuration,
    EmptyEvidence,
    NonPositiveEvidence,
    InvalidThreshold,
    InvalidCounterWidth,
    CounterValueOutOfRange,
    TiedObservations,
}
impl std::fmt::Display for Error {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{self:?}")
    }
}
impl std::error::Error for Error {}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum EvidenceRequirement {
    HardwareCycles,
    RetiredInstructions,
    L1DataMisses,
    LlcMisses,
    BranchMispredicts,
    SimdVectorOperations,
    TmaBackendSlots,
    ZeroAllocation,
    Vectorization,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum UnavailableReason {
    NoPmuProvider,
    NoAllocationInstrumentation,
    NoCompilerVectorizationEvidence,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct UnavailableEvidence {
    pub requirement: EvidenceRequirement,
    pub reason: UnavailableReason,
}
/// This backend has no hardware/static evidence provider. A requirement remains
/// inconclusive; caller assertions and wall time never authorize it to pass.
pub fn unavailable_evidence(requirement: EvidenceRequirement) -> UnavailableEvidence {
    #[cfg(vesper_port)]
    return port::unavailable_evidence(requirement);
    #[cfg(not(vesper_port))]
    {
        let reason = match requirement {
            EvidenceRequirement::ZeroAllocation => UnavailableReason::NoAllocationInstrumentation,
            EvidenceRequirement::Vectorization => {
                UnavailableReason::NoCompilerVectorizationEvidence
            }
            _ => UnavailableReason::NoPmuProvider,
        };
        UnavailableEvidence {
            requirement,
            reason,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct EvidenceGate {
    pub decision: Decision,
    pub unavailable: UnavailableEvidence,
}
pub fn evaluate_evidence_requirement(requirement: EvidenceRequirement) -> EvidenceGate {
    #[cfg(vesper_port)]
    return port::evaluate_evidence_requirement(requirement);
    #[cfg(not(vesper_port))]
    {
        EvidenceGate {
            decision: Decision::Inconclusive,
            unavailable: unavailable_evidence(requirement),
        }
    }
}
