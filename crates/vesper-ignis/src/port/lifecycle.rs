//! Vesper build of the lifecycle fence ledger. The fence protocol (limit
//! checks, translation, reservations, completion validation, retirement)
//! runs on a Vesper `Lifecycle` handle; the shared `Runtime` keeps the
//! payloads, and this file rebuilds the public `Submission` from the
//! translated records.

use super::binding as v;
use super::ffi;
use crate::lifecycle::{
    BatchView, FencePoint, FrameTicket, LifecycleError, Limits, QueueCommand, QueueConfig,
    ScratchAction, Submission, SubmissionStream, TimelineId,
};
use crate::{Command, PassId, Plan};

/// How the backend answered the submission most recently begun.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Settle {
    Rejected,
    Accepted,
}

fn limits(l: Limits) -> ffi::Limits {
    ffi::Limits {
        max_queues: l.max_queues as u64,
        max_inflight: l.max_inflight as u64,
        max_commands: l.max_commands as u64,
        max_scratch: l.max_scratch as u64,
        max_poll_events: l.max_poll_events as u64,
        max_drain_polls: l.max_drain_polls as u64,
    }
}

/// `Ok` for `LifeError::None`, else the public error.
fn result(e: ffi::LifeError) -> Result<(), LifecycleError> {
    use LifecycleError::Limit;
    Err(match e {
        ffi::LifeError::None => return Ok(()),
        ffi::LifeError::LimitInvalid => Limit("invalid lifecycle limits"),
        ffi::LifeError::LimitQueues => Limit("queues"),
        ffi::LifeError::LimitPlanQueues => Limit("plan queues"),
        ffi::LifeError::LimitCommands => Limit("commands"),
        ffi::LifeError::LimitBarriers => Limit("barriers"),
        ffi::LifeError::LimitScratch => Limit("scratch per frame"),
        ffi::LifeError::LimitInflight => Limit("inflight frames"),
        ffi::LifeError::LimitPollEvents => Limit("poll events"),
        ffi::LifeError::LimitPollBound => Limit("backend exceeded poll bound"),
        ffi::LifeError::LimitDrainPolls => Limit("drain polls"),
        ffi::LifeError::InstanceIdsExhausted => LifecycleError::InstanceIdsExhausted,
        ffi::LifeError::ForeignTimeline => LifecycleError::ForeignTimeline,
        ffi::LifeError::QueueKindMismatch => LifecycleError::QueueKindMismatch,
        ffi::LifeError::QueueBinding => LifecycleError::QueueBinding,
        ffi::LifeError::AllocatorBinding => LifecycleError::AllocatorBinding,
        ffi::LifeError::ForeignPass => LifecycleError::ForeignPass,
        ffi::LifeError::ScratchOnCopyQueue => LifecycleError::ScratchOnCopyQueue,
        ffi::LifeError::FenceOverflow => LifecycleError::FenceOverflow,
        ffi::LifeError::FrameIdsExhausted => LifecycleError::FrameIdsExhausted,
        ffi::LifeError::SubmissionUncertain => LifecycleError::SubmissionUncertain,
        ffi::LifeError::StaleCompletion => LifecycleError::StaleCompletion,
        ffi::LifeError::UnknownCompletion => LifecycleError::UnknownCompletion,
        ffi::LifeError::WaitNotObserved {
            runtime,
            index,
            value,
        } => LifecycleError::WaitNotObserved(point(ffi::Point {
            runtime,
            index,
            value,
        })),
    })
}

fn point(p: ffi::Point) -> FencePoint {
    FencePoint {
        timeline: TimelineId {
            runtime: p.runtime,
            index: p.index as usize,
        },
        value: p.value,
    }
}

/// A plan as stream and command records, plus each command record's
/// command.
fn plan_records(
    plan: &Plan,
) -> (
    Vec<v::repr::StreamInfo>,
    Vec<v::repr::PlanCmd>,
    Vec<&Command>,
) {
    let mut streams = Vec::with_capacity(plan.queues.len());
    let mut cmds = Vec::new();
    let mut commands = Vec::new();
    for (si, q) in plan.queues.iter().enumerate() {
        let mut barriers = 0u64;
        for c in &q.commands {
            let (tag, graph, index, value, count) = match c {
                Command::Wait { queue, value } => (0, queue.graph, queue.index as u32, *value, 0),
                Command::Barriers(bs) => {
                    barriers += bs.len() as u64;
                    (1, 0, 0, 0, bs.len() as u64)
                }
                Command::Execute(pass) => (2, pass.graph, pass.index, 0, 0),
                Command::Signal { value } => (3, 0, 0, *value, 0),
            };
            cmds.push(v::repr::PlanCmd::from_value(&ffi::PlanCmd {
                stream: si as u32,
                tag,
                graph,
                index,
                value,
                barriers: count,
            }));
            commands.push(c);
        }
        streams.push(v::repr::StreamInfo::from_value(&ffi::StreamInfo {
            graph: q.queue.graph,
            queue: q.queue.index as u32,
            kind: q.kind,
            commands: q.commands.len() as u64,
            barriers,
        }));
    }
    (streams, cmds, commands)
}

fn binding_records(batch: &BatchView<'_>) -> Vec<v::repr::BindingRec> {
    batch
        .bindings
        .iter()
        .enumerate()
        .map(|(i, b)| {
            let alloc = batch.allocators.get(i).copied().unwrap_or(TimelineId {
                runtime: 0,
                index: usize::MAX,
            });
            v::repr::BindingRec::from_value(&ffi::BindingRec {
                queue_graph: b.queue.graph,
                queue_index: b.queue.index as u32,
                runtime: b.timeline.runtime,
                index: b.timeline.index as u64,
                alloc_runtime: alloc.runtime,
                alloc_index: alloc.index as u64,
            })
        })
        .collect()
}

/// Checks a scratch lease for `pass` against the plan and the per-frame bound.
pub(crate) fn scratch_check(
    plan: &Plan,
    pass: PassId,
    leases: usize,
    max_scratch: usize,
) -> Result<(), LifecycleError> {
    let (streams, cmds, _) = plan_records(plan);
    result(v::scratch_check(
        &streams,
        &cmds,
        pass.graph,
        pass.index,
        leases as u64,
        max_scratch as u64,
    ))
}

pub(crate) struct Ledger {
    id: u64,
    inner: v::Lifecycle,
}

impl Ledger {
    pub(crate) fn check(queues: &[QueueConfig], l: Limits) -> Result<(), LifecycleError> {
        let initial: Vec<u64> = queues.iter().map(|q| q.initial_fence).collect();
        result(v::lifecycle_check_new(
            &v::repr::Limits::from_value(&limits(l)),
            &initial,
        ))
    }
    pub(crate) fn new(id: u64, queues: &[QueueConfig], l: Limits) -> Self {
        let kinds: Vec<_> = queues.iter().map(|q| q.kind).collect();
        let initial: Vec<u64> = queues.iter().map(|q| q.initial_fence).collect();
        Self {
            id,
            inner: v::lifecycle_new(
                id,
                &v::repr::Limits::from_value(&limits(l)),
                &kinds,
                &initial,
            ),
        }
    }
    pub(crate) fn id(&self) -> u64 {
        self.id
    }
    pub(crate) fn queue_count(&self) -> usize {
        v::lifecycle_queue_count(&self.inner) as usize
    }
    pub(crate) fn max_scratch(&self) -> usize {
        v::lifecycle_max_scratch(&self.inner) as usize
    }
    pub(crate) fn poll_allowed(&self, max_events: usize) -> bool {
        v::lifecycle_poll_allowed(&self.inner, max_events as u64)
    }
    pub(crate) fn drain_allowed(&self, polls: usize) -> bool {
        v::lifecycle_drain_allowed(&self.inner, polls as u64)
    }
    pub(crate) fn observed(&self, timeline: TimelineId) -> Result<u64, LifecycleError> {
        let (runtime, index) = (timeline.runtime, timeline.index as u64);
        if !v::lifecycle_owns(&self.inner, runtime, index) {
            return Err(LifecycleError::ForeignTimeline);
        }
        Ok(v::lifecycle_observed(&self.inner, runtime, index))
    }
    pub(crate) fn validate(&self, batch: &BatchView<'_>) -> Result<(), LifecycleError> {
        let (streams, _, _) = plan_records(batch.plan);
        let bindings = binding_records(batch);
        result(v::lifecycle_validate(
            &self.inner,
            &streams,
            &bindings,
            batch.allocators.len() as u64,
        ))
    }
    /// Preflight, translate and reserve: the flight stays uncertain until
    /// [`Ledger::settle`].
    pub(crate) fn begin(&mut self, batch: &BatchView<'_>) -> Result<Submission, LifecycleError> {
        let (streams, cmds, commands) = plan_records(batch.plan);
        let bindings = binding_records(batch);
        let scratch: Vec<v::repr::ScratchRec> = batch
            .scratch
            .iter()
            .map(|(pass, action, _)| {
                v::repr::ScratchRec::from_value(&ffi::ScratchRec {
                    pass_graph: pass.graph,
                    pass_index: pass.index,
                    initialize: *action == ScratchAction::Initialize,
                })
            })
            .collect();
        let t = v::lifecycle_submit(
            &mut self.inner,
            batch.runtime,
            &streams,
            &bindings,
            &cmds,
            &scratch,
        );
        let t = t.as_ref();
        result(t.error())?;
        let mut out: Vec<SubmissionStream> = batch
            .plan
            .queues
            .iter()
            .map(|q| SubmissionStream {
                timeline: batch
                    .bindings
                    .iter()
                    .find(|b| b.queue == q.queue)
                    .expect("validated binding")
                    .timeline,
                kind: q.kind,
                commands: Vec::new(),
            })
            .collect();
        for c in t.cmds().iter() {
            let command = match c.tag {
                0 => QueueCommand::Wait(point(c.point)),
                1 => match commands[c.source as usize] {
                    Command::Barriers(bs) => QueueCommand::Barriers(bs.clone()),
                    _ => unreachable!("barrier record of another command"),
                },
                2 => {
                    let (_, action, program) = batch.scratch[c.source as usize];
                    QueueCommand::Scratch {
                        index: c.source as usize,
                        action,
                        program,
                    }
                }
                3 => match commands[c.source as usize] {
                    Command::Execute(pass) => QueueCommand::Execute(*pass),
                    _ => unreachable!("execute record of another command"),
                },
                _ => QueueCommand::Signal(point(c.point)),
            };
            out[c.stream as usize].commands.push(command);
        }
        Ok(Submission {
            ticket: FrameTicket {
                runtime: self.id,
                sequence: t.ticket(),
            },
            streams: out,
            terminal: t.terminal().iter().map(point).collect(),
        })
    }
    pub(crate) fn settle(&mut self, outcome: Settle) {
        let code = match outcome {
            Settle::Rejected => 0,
            Settle::Accepted => 2,
        };
        v::lifecycle_settle(&mut self.inner, code);
    }
    pub(crate) fn check_completion(&self, p: FencePoint) -> Result<(), LifecycleError> {
        result(v::lifecycle_check_completion(
            &self.inner,
            p.timeline.runtime,
            p.timeline.index as u64,
            p.value,
        ))
    }
    /// Records a validated completion and returns the tickets it retires,
    /// oldest first.
    pub(crate) fn complete(&mut self, p: FencePoint) -> Vec<FrameTicket> {
        v::lifecycle_complete(&mut self.inner, p.timeline.index as u64, p.value)
            .into_iter()
            .map(|sequence| FrameTicket {
                runtime: self.id,
                sequence,
            })
            .collect()
    }
}
