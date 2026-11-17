//! The record types of the Vesper build's export ABI, named and shaped as
//! `vesper/src/lib.ves` declares them; the generated `port/binding.rs`
//! implements `Abi` for them. The types the public API already has with the
//! same shape are re-exported instead.

#![allow(dead_code)]

pub use crate::d3d12::{Layout, QueueKind};
pub use crate::graph::SubresourceRange;
pub use crate::multi_backend::GpuApiTarget;
pub use crate::plan::DependencyKind;
pub use crate::recording::{RecordingError, SubmitMode};

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum AccessBits {
    Mapped(u64),
    Unmapped(u32),
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ActiveStream {
    pub runtime: u64,
    pub index: u64,
    pub first: u64,
    pub count: u64,
    pub cursor: u64,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ActiveSub {
    pub first_stream: u64,
    pub stream_count: u64,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct BarrierRec {
    pub texture: bool,
    pub resource: u32,
    pub sync_before: u32,
    pub sync_after: u32,
    pub access_before: u32,
    pub access_after: u32,
    pub layout_before: Layout,
    pub layout_after: Layout,
    pub subresources: SubresourceRange,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct BindingRec {
    pub queue_graph: u32,
    pub queue_index: u32,
    pub runtime: u64,
    pub index: u64,
    pub alloc_runtime: u64,
    pub alloc_index: u64,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct CmdRec {
    pub queue: u32,
    pub tag: u32,
    pub a: u64,
    pub b: u64,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct DepRec {
    pub before: u32,
    pub after: u32,
    pub kind: DependencyKind,
    pub has_resource: bool,
    pub resource: u32,
    pub texture: bool,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum DependencyOutcome {
    Added,
    AlreadyPresent,
    ForeignHandle,
    SelfDependency,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Edge2 {
    pub before: u32,
    pub after: u32,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Flight {
    pub ticket: u64,
    pub uncertain: bool,
    pub first_cmd: u64,
    pub cmd_count: u64,
    pub first_term: u64,
    pub term_count: u64,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct FlightCmd {
    pub runtime: u64,
    pub index: u64,
    pub signal: bool,
    pub point: Point,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct KeyedBarrier {
    pub graph: u32,
    pub barrier: BarrierRec,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum LifeError {
    None,
    LimitInvalid,
    LimitQueues,
    LimitPlanQueues,
    LimitCommands,
    LimitBarriers,
    LimitScratch,
    LimitInflight,
    LimitPollEvents,
    LimitPollBound,
    LimitDrainPolls,
    InstanceIdsExhausted,
    ForeignTimeline,
    QueueKindMismatch,
    QueueBinding,
    AllocatorBinding,
    ForeignPass,
    ScratchOnCopyQueue,
    FenceOverflow,
    FrameIdsExhausted,
    SubmissionUncertain,
    StaleCompletion,
    UnknownCompletion,
    WaitNotObserved {
        runtime: u64,
        index: u64,
        value: u64,
    },
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Limits {
    pub max_queues: u64,
    pub max_inflight: u64,
    pub max_commands: u64,
    pub max_scratch: u64,
    pub max_poll_events: u64,
    pub max_drain_polls: u64,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct LoweredRec {
    pub stream: u32,
    pub tag: u32,
    pub source: u64,
    pub start: u64,
    pub count: u64,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Native {
    Elided,
    D3D12,
    BufferBarrier2 {
        src_stage_mask: u64,
        dst_stage_mask: u64,
        src_access_mask: u64,
        dst_access_mask: u64,
    },
    ImageBarrier2 {
        src_stage_mask: u64,
        dst_stage_mask: u64,
        src_access_mask: u64,
        dst_access_mask: u64,
    },
    SplitEventSignal {
        event_id: u32,
        src_stage_mask: u64,
    },
    SplitEventWait {
        event_id: u32,
        dst_stage_mask: u64,
        src_access_mask: u64,
        dst_access_mask: u64,
    },
    Metal4StageBarrier {
        after_stages_mask: u32,
        before_stages_mask: u32,
    },
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct NativeRec {
    pub source: u64,
    pub native: Native,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct PassInfo {
    pub queue: u32,
    pub first_use: u64,
    pub use_count: u64,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum PassOutcome {
    Added(u32),
    ForeignHandle,
    InvalidAccess {
        use_index: u64,
        access: u32,
    },
    InvalidSync {
        use_index: u64,
        sync: u32,
    },
    AccessSyncMismatch {
        use_index: u64,
        access: u32,
        sync: u32,
    },
    AccessLayoutMismatch {
        use_index: u64,
        access: u32,
        layout: Layout,
    },
    AccessResourceKindMismatch {
        use_index: u64,
        access: u32,
    },
    QueueAccess {
        use_index: u64,
        access: u32,
    },
    QueueSync {
        use_index: u64,
        sync: u32,
    },
    QueueLayout {
        use_index: u64,
        layout: Layout,
    },
    InvalidRange {
        use_index: u64,
    },
    ConflictingUses {
        first_use: u64,
        second_use: u64,
    },
    TooManyPasses,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct PlanCmd {
    pub stream: u32,
    pub tag: u32,
    pub graph: u32,
    pub index: u32,
    pub value: u64,
    pub barriers: u64,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum PlanFault {
    None,
    Cycle,
    NoLayoutForAccess {
        pass: u32,
        resource: u32,
        subresource: u32,
        access: u32,
    },
    UnsupportedQueueHandoff {
        pass: u32,
        resource: u32,
        subresource: u32,
        from: Layout,
        to: Layout,
    },
    FinalLayoutUnreachable {
        resource: u32,
        subresource: u32,
        from: Layout,
        to: Layout,
    },
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Point {
    pub runtime: u64,
    pub index: u64,
    pub value: u64,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum QueueOutcome {
    Added(u32),
    TooManyQueues,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum RecorderVerdict {
    Refused(RecordingError),
    Stalled,
    Recorded { uncertain: bool, panic: bool },
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Resource {
    pub texture: bool,
    pub size: u64,
    pub mips: u32,
    pub slices: u32,
    pub planes: u32,
    pub initial_layout: Layout,
    pub has_final: bool,
    pub final_layout: Layout,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum ResourceOutcome {
    Added(u32),
    ZeroSize,
    ZeroDimension,
    CountOverflow,
    TooManySubresources,
    FinalUndefined,
    TooManyResources,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ScratchRec {
    pub pass_graph: u32,
    pub pass_index: u32,
    pub initialize: bool,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct SimCmd {
    pub wait: bool,
    pub signal: bool,
    pub point: Point,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct SimStream {
    pub runtime: u64,
    pub index: u64,
    pub first: u64,
    pub count: u64,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum StageBits {
    Mapped(u64),
    Unmapped(u32),
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct StreamInfo {
    pub graph: u32,
    pub queue: u32,
    pub kind: QueueKind,
    pub commands: u64,
    pub barriers: u64,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct SubCmd {
    pub stream: u32,
    pub tag: u32,
    pub point: Point,
    pub source: u64,
    pub initialize: bool,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Use {
    pub resource: u32,
    pub bytes: bool,
    pub start: u64,
    pub end: u64,
    pub range: SubresourceRange,
    pub access: u32,
    pub sync: u32,
    pub has_layout: bool,
    pub layout: Layout,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct UseDesc {
    pub foreign: bool,
    pub resource: u32,
    pub bytes: bool,
    pub offset: u64,
    pub size: u64,
    pub range: SubresourceRange,
    pub access: u32,
    pub sync: u32,
    pub has_layout: bool,
    pub layout: Layout,
}
