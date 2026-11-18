//! The port build's side of the boundary (`cfg(vesper_port)`, feature
//! `vesper-port`): the port-only records of the Vesper library in
//! `vesper/` (port wave P3a lane 5, #1040), and the conversions between
//! them and the crate's public types. The generated binding
//! (`port/binding.rs`) names these as `crate::port::*`.
//!
//! Vesper has no `i128`, so 128-bit values cross as `W` word pairs, and the
//! crate's `&'static str` error messages cross as codes (`LIMITS`,
//! `CONFIG_MESSAGES`). Every exported function raises only its own error
//! type; the port's helpers are total.
use crate::{
    Analysis, BasisPointBounds, Error, InconclusiveReason, MedianInterval, PairedSample,
    PairedSampleConfidenceError, SignedRankConfidenceSegment,
};

pub use crate::causal::ExperimentRun;
pub use crate::contract::{ErrorKind, ParseError};
pub use crate::contract_gate::GateError;
pub use crate::layout::LayoutPlan;
pub use crate::port_binding::*;
pub use crate::wire::PmuSampleWire;
pub use crate::{
    Decision, EvidenceGate, EvidenceRequirement, HodgesLehmannError, MannWhitneyRanks,
    PairedDifferenceError, SignedRankConfidenceError, SignedRankError, SignedRankProbability,
    UnavailableEvidence, UnavailableReason, WalshEstimateError,
};

/// A 128-bit two's-complement value as (high, low) words.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct W {
    pub hi: u64,
    pub lo: u64,
}
impl W {
    pub fn signed(self) -> i128 {
        (((self.hi as u128) << 64) | self.lo as u128) as i128
    }
    pub fn unsigned(self) -> u128 {
        ((self.hi as u128) << 64) | self.lo as u128
    }
}

/// `PairedSample`, whose fields are private.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Sample {
    pub baseline: u64,
    pub candidate: u64,
    pub operations: u64,
}
pub fn sample(s: PairedSample) -> Sample {
    Sample {
        baseline: s.baseline_total(),
        candidate: s.candidate_total(),
        operations: s.operations_per_arm(),
    }
}
pub fn samples(s: &[PairedSample]) -> Vec<Sample> {
    s.iter().map(|&s| sample(s)).collect()
}
/// A sample the port validated (every total and the count are nonzero).
pub fn paired(s: Sample) -> PairedSample {
    PairedSample::from_validated(s.baseline, s.candidate, s.operations)
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Segment {
    pub lower_bounded: bool,
    pub lower_hi: u64,
    pub lower_lo: u64,
    pub lower_inclusive: bool,
    pub upper_bounded: bool,
    pub upper_hi: u64,
    pub upper_lo: u64,
    pub upper_inclusive: bool,
}
fn split(v: Option<i128>) -> (bool, u64, u64) {
    match v {
        Some(v) => (true, ((v as u128) >> 64) as u64, v as u64),
        None => (false, 0, 0),
    }
}
pub fn segment_out(s: Segment) -> SignedRankConfidenceSegment {
    SignedRankConfidenceSegment {
        lower_times_two: s.lower_bounded.then(|| {
            W {
                hi: s.lower_hi,
                lo: s.lower_lo,
            }
            .signed()
        }),
        lower_inclusive: s.lower_inclusive,
        upper_times_two: s.upper_bounded.then(|| {
            W {
                hi: s.upper_hi,
                lo: s.upper_lo,
            }
            .signed()
        }),
        upper_inclusive: s.upper_inclusive,
    }
}
pub fn segments_out(s: Vec<Segment>) -> Vec<SignedRankConfidenceSegment> {
    s.into_iter().map(segment_out).collect()
}
pub fn segments_in(s: &[SignedRankConfidenceSegment]) -> Vec<Segment> {
    s.iter()
        .map(|s| {
            let (lower_bounded, lower_hi, lower_lo) = split(s.lower_times_two);
            let (upper_bounded, upper_hi, upper_lo) = split(s.upper_times_two);
            Segment {
                lower_bounded,
                lower_hi,
                lower_lo,
                lower_inclusive: s.lower_inclusive,
                upper_bounded,
                upper_hi,
                upper_lo,
                upper_inclusive: s.upper_inclusive,
            }
        })
        .collect()
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PairedConfError {
    EmptyEvidence,
    TooManySamples,
    OperationsMismatch { index: usize },
    DifferenceOutOfRange { index: usize },
}
pub fn paired_conf_error(e: PairedConfError) -> PairedSampleConfidenceError {
    match e {
        PairedConfError::EmptyEvidence => PairedSampleConfidenceError::EmptyEvidence,
        PairedConfError::TooManySamples => PairedSampleConfidenceError::TooManySamples,
        PairedConfError::OperationsMismatch { index } => {
            PairedSampleConfidenceError::OperationsMismatch { index }
        }
        PairedConfError::DifferenceOutOfRange { index } => {
            PairedSampleConfidenceError::DifferenceOutOfRange { index }
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct WalshOut {
    pub pairs: usize,
    pub walsh_averages: usize,
    pub estimate_times_four: W,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct HlOut {
    pub estimate_times_four: W,
    pub has_interval: bool,
    pub lower_times_two: W,
    pub upper_times_two: W,
    pub critical_rank: usize,
    pub covered_outcomes: u64,
    pub total_outcomes: u64,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct HlGate {
    pub outcome: u64,
    pub estimate: HlOut,
}

/// The crate's [`Error`] with each message as a code.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PortError {
    InvalidConfig { code: u64 },
    Limit { code: u64 },
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
/// `Error::Limit` messages by code (vesper/src/stats.ves `limit_*`).
const LIMITS: [&str; 6] = [
    "rank sample size",
    "exact rank sample size",
    "epochs",
    "layouts",
    "body calls",
    "elapsed run budget",
];
/// `Error::InvalidConfig` messages by code (vesper/src/plan.ves `invalid`).
const CONFIG_MESSAGES: [&str; 5] = [
    "epochs must be 1..=64",
    "warmup epochs exceed 64",
    "operations must be 1..=65536",
    "total call budget must be 1..=10000000",
    "elapsed budget must be 1ns..=600s",
];
pub fn error(e: PortError) -> Error {
    match e {
        PortError::InvalidConfig { code } => Error::InvalidConfig(CONFIG_MESSAGES[code as usize]),
        PortError::Limit { code } => Error::Limit(LIMITS[code as usize]),
        PortError::ArithmeticOverflow => Error::ArithmeticOverflow,
        PortError::AllocationFailed => Error::AllocationFailed,
        PortError::ClockOverflow => Error::ClockOverflow,
        PortError::ZeroDuration => Error::ZeroDuration,
        PortError::EmptyEvidence => Error::EmptyEvidence,
        PortError::NonPositiveEvidence => Error::NonPositiveEvidence,
        PortError::InvalidThreshold => Error::InvalidThreshold,
        PortError::InvalidCounterWidth => Error::InvalidCounterWidth,
        PortError::CounterValueOutOfRange => Error::CounterValueOutOfRange,
        PortError::TiedObservations => Error::TiedObservations,
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ExactMw {
    pub u_first: u64,
    pub numerator: u64,
    pub denominator: u64,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct AnalysisOut {
    pub decision: Decision,
    pub reason: u64,
    pub median_lower: Sample,
    pub median_upper: Sample,
    pub has_interval: bool,
    pub lower: Sample,
    pub upper: Sample,
    pub lower_rank: usize,
    pub upper_rank: usize,
    pub covered_outcomes: W,
    pub total_outcomes: W,
}
pub fn analysis(a: AnalysisOut) -> Analysis {
    Analysis {
        decision: a.decision,
        reason: match a.reason {
            0 => None,
            1 => Some(InconclusiveReason::InsufficientEpochs),
            _ => Some(InconclusiveReason::IntervalCrossesThreshold),
        },
        median_lower: paired(a.median_lower),
        median_upper: paired(a.median_upper),
        interval: a.has_interval.then(|| MedianInterval {
            lower: paired(a.lower),
            upper: paired(a.upper),
            lower_rank: a.lower_rank,
            upper_rank: a.upper_rank,
            covered_outcomes: a.covered_outcomes.unsigned(),
            total_outcomes: a.total_outcomes.unsigned(),
        }),
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct BpBounds {
    pub floor: W,
    pub ceil: W,
}
pub fn bp_bounds(b: BpBounds) -> BasisPointBounds {
    BasisPointBounds {
        floor: b.floor.signed(),
        ceil: b.ceil.signed(),
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct SurvivalOut {
    pub verdict: u64,
    pub analysis: AnalysisOut,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ContractOut {
    pub zero_alloc: u64,
    pub expect_vectorized: u64,
    pub has_instructions_per_byte: bool,
    pub instructions_coefficient: u64,
    pub instructions_scale: u32,
    pub has_stabilizer_pages: bool,
    pub stabilizer_pages: u64,
    pub has_l1d_misses_per_op: bool,
    pub l1d_misses_per_op: u64,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct GateIn {
    pub has_instructions_per_byte: bool,
    pub instructions_coefficient: u64,
    pub instructions_scale: u32,
    pub has_l1d_misses_per_op: bool,
    pub l1d_misses_per_op: u64,
    pub zero_alloc: bool,
    pub expect_vectorized: bool,
    pub has_stabilizer_pages: bool,
    pub has_instructions: bool,
    pub instructions: u64,
    pub has_l1d_misses: bool,
    pub l1d_misses: u64,
    pub has_input_bytes: bool,
    pub input_bytes: u64,
    pub has_operations: bool,
    pub operations: u64,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct GateOut {
    pub instructions_per_byte: u64,
    pub l1d_misses_per_op: u64,
    pub zero_alloc: u64,
    pub expect_vectorized: u64,
    pub stabilizer_pages: u64,
    pub overall: u64,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum WireErr {
    Length { len: usize },
    BasisPointsOutOfRange { value: u64 },
    LayoutOffsetOutOfRange { value: u64 },
    FieldOverflow { field: u64 },
    InvalidMask { bits: u16 },
}
pub fn wire_error(e: WireErr) -> crate::wire::WireError {
    use crate::wire::{PmuField, WireError};
    match e {
        WireErr::Length { len } => WireError::Length(len),
        WireErr::BasisPointsOutOfRange { value } => WireError::BasisPointsOutOfRange(value),
        WireErr::LayoutOffsetOutOfRange { value } => WireError::LayoutOffsetOutOfRange(value),
        WireErr::FieldOverflow { field } => WireError::FieldOverflow(PmuField::ALL[field as usize]),
        WireErr::InvalidMask { bits } => WireError::InvalidMask(bits),
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CausalErr {
    EmptyEvidence,
    DelayExceedsElapsed { elapsed: u64, delay: u64 },
}

pub fn walsh(w: WalshOut) -> crate::WalshEstimate {
    crate::WalshEstimate {
        pairs: w.pairs,
        walsh_averages: w.walsh_averages,
        estimate_times_four: w.estimate_times_four.signed(),
    }
}

pub fn hl_estimate(h: HlOut) -> crate::HodgesLehmannEstimate {
    crate::HodgesLehmannEstimate {
        estimate_times_four: h.estimate_times_four.signed(),
        interval: h.has_interval.then(|| crate::SignedRankInterval {
            lower_times_two: h.lower_times_two.signed(),
            upper_times_two: h.upper_times_two.signed(),
            critical_rank: h.critical_rank,
            covered_outcomes: h.covered_outcomes,
            total_outcomes: h.total_outcomes,
        }),
    }
}

pub fn hl_gate(g: HlGate) -> crate::PairedDifferenceGate {
    use crate::PairedDifferenceInconclusive as Why;
    let (decision, reason) = match g.outcome {
        0 => (Decision::Pass, None),
        1 => (Decision::Regression, None),
        2 => (Decision::Inconclusive, Some(Why::InsufficientPairs)),
        _ => (Decision::Inconclusive, Some(Why::IntervalCrossesThreshold)),
    };
    crate::PairedDifferenceGate {
        decision,
        reason,
        estimate: hl_estimate(g.estimate),
    }
}

/// Gate outcome codes of vesper/src/ranks.ves `do_classify_*`.
pub fn gate_outcome<R>(code: u64, empty: R, not_separated: R) -> (Decision, Option<R>) {
    match code {
        0 => (Decision::Pass, None),
        1 => (Decision::Regression, None),
        2 => (Decision::Inconclusive, Some(empty)),
        _ => (Decision::Inconclusive, Some(not_separated)),
    }
}

pub fn contract(c: ContractOut) -> crate::contract::BenchContract {
    let flag = |v: u64| match v {
        0 => None,
        1 => Some(false),
        _ => Some(true),
    };
    crate::contract::BenchContract {
        zero_alloc: flag(c.zero_alloc),
        expect_vectorized: flag(c.expect_vectorized),
        max_instructions_per_byte: c.has_instructions_per_byte.then_some(
            crate::contract::Decimal {
                coefficient: c.instructions_coefficient,
                scale: c.instructions_scale,
            },
        ),
        stabilizer_pages: c.has_stabilizer_pages.then_some(c.stabilizer_pages),
        max_l1d_misses_per_op: c.has_l1d_misses_per_op.then_some(c.l1d_misses_per_op),
    }
}

pub fn gate_in(
    contract: &crate::contract::BenchContract,
    sample: &crate::wire::PmuSample,
    counts: crate::contract_gate::Counts,
) -> GateIn {
    use crate::wire::PmuField;
    let limit = contract.max_instructions_per_byte;
    let instructions = sample.get(PmuField::InstructionsRetired);
    let l1d_misses = sample.get(PmuField::L1dReadMisses);
    GateIn {
        has_instructions_per_byte: limit.is_some(),
        instructions_coefficient: limit.map_or(0, |l| l.coefficient),
        instructions_scale: limit.map_or(0, |l| l.scale),
        has_l1d_misses_per_op: contract.max_l1d_misses_per_op.is_some(),
        l1d_misses_per_op: contract.max_l1d_misses_per_op.unwrap_or(0),
        zero_alloc: contract.zero_alloc == Some(true),
        expect_vectorized: contract.expect_vectorized == Some(true),
        has_stabilizer_pages: contract.stabilizer_pages.is_some(),
        has_instructions: instructions.is_some(),
        instructions: instructions.unwrap_or(0),
        has_l1d_misses: l1d_misses.is_some(),
        l1d_misses: l1d_misses.unwrap_or(0),
        has_input_bytes: counts.input_bytes.is_some(),
        input_bytes: counts.input_bytes.unwrap_or(0),
        has_operations: counts.operations.is_some(),
        operations: counts.operations.unwrap_or(0),
    }
}

pub fn gate_report(g: GateOut) -> crate::contract_gate::GateReport {
    use crate::contract_gate::{GateReport, GateStatus, Overall, Unavailable};
    use crate::wire::PmuField;
    let status = |code: u64| match code {
        0 => GateStatus::NotRequested,
        1 => GateStatus::Pass,
        2 => GateStatus::Fail,
        3 => GateStatus::Inconclusive(Unavailable::Counter(PmuField::InstructionsRetired)),
        4 => GateStatus::Inconclusive(Unavailable::Counter(PmuField::L1dReadMisses)),
        5 => GateStatus::Inconclusive(Unavailable::InputBytes),
        6 => GateStatus::Inconclusive(Unavailable::Operations),
        7 => GateStatus::Inconclusive(Unavailable::ZeroInputBytes),
        8 => GateStatus::Inconclusive(Unavailable::ZeroOperations),
        9 => GateStatus::Inconclusive(Unavailable::AllocationEvidence),
        10 => GateStatus::Inconclusive(Unavailable::VectorizationEvidence),
        _ => GateStatus::Inconclusive(Unavailable::LayoutEvidence),
    };
    GateReport {
        instructions_per_byte: status(g.instructions_per_byte),
        l1d_misses_per_op: status(g.l1d_misses_per_op),
        zero_alloc: status(g.zero_alloc),
        expect_vectorized: status(g.expect_vectorized),
        stabilizer_pages: status(g.stabilizer_pages),
        overall: match g.overall {
            0 => Overall::NotRequested,
            1 => Overall::Pass,
            2 => Overall::Fail,
            _ => Overall::Inconclusive,
        },
    }
}

pub fn survival(s: SurvivalOut, layouts: usize) -> crate::layout::LayoutSurvival {
    use crate::layout::{LayoutSurvival, LayoutVerdict};
    LayoutSurvival {
        verdict: match s.verdict {
            0 => LayoutVerdict::CandidateSlower,
            1 => LayoutVerdict::CandidateFaster,
            2 => LayoutVerdict::DoesNotSurvive,
            _ => LayoutVerdict::InsufficientLayouts,
        },
        analysis: analysis(s.analysis),
        layouts,
    }
}
