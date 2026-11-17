//! Owned host command/fence lifecycle above the render-graph planner.
//!
//! This is a backend-independent protocol, not a D3D12 implementation. The
//! backend is trusted to report **original queue fence signals**, and to call a
//! submission rejected only when no work was accepted. A cancellation request,
//! queued initialization, or an arbitrary application counter is not completion.
//! Payloads are never cloned. Dropping a runtime with pending work intentionally
//! leaks its backend and pending owners; use bounded [`Runtime::shutdown`] to
//! recover them. Rust `Drop` is not Vesper source-language linear exhaustion.

use crate::engine::lifecycle::{self as engine, Ledger, Settle};
use crate::{Barrier, PassId, Plan, PlanError, QueueId, QueueKind, RenderGraph};
use std::collections::VecDeque;
use std::marker::PhantomData;
use std::sync::atomic::{AtomicU64, Ordering};

static NEXT_RUNTIME: AtomicU64 = AtomicU64::new(1);

/// `u64::MAX` is reserved for device-removal/error sentinels in a future D3D12
/// adapter and can never be issued or accepted as an original fence value.
pub const MAX_FENCE_VALUE: u64 = u64::MAX - 1;

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct TimelineId {
    pub(crate) runtime: u64,
    pub(crate) index: usize,
}
impl TimelineId {
    pub fn index(self) -> usize {
        self.index
    }
}
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct FrameTicket {
    pub(crate) runtime: u64,
    pub(crate) sequence: u64,
}
impl FrameTicket {
    pub fn sequence(self) -> u64 {
        self.sequence
    }
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct FencePoint {
    pub timeline: TimelineId,
    pub value: u64,
}
/// A report from the trusted backend, not an independently authenticated proof.
/// It is returned intact on stale/foreign/out-of-order reports.
#[derive(Debug, PartialEq, Eq)]
pub struct Completion {
    point: FencePoint,
}
impl Completion {
    pub fn reported(point: FencePoint) -> Self {
        Self { point }
    }
    pub fn point(&self) -> FencePoint {
        self.point
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Limits {
    pub max_queues: usize,
    pub max_inflight: usize,
    pub max_commands: usize,
    pub max_scratch: usize,
    pub max_poll_events: usize,
    pub max_drain_polls: usize,
}
impl Default for Limits {
    fn default() -> Self {
        Self {
            max_queues: 16,
            max_inflight: 64,
            max_commands: 16_384,
            max_scratch: 64,
            max_poll_events: 256,
            max_drain_polls: 1024,
        }
    }
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct QueueConfig {
    pub kind: QueueKind,
    /// The backend must already have reached this value, with no unrepresented
    /// outstanding work. New signals are strictly greater than this floor.
    pub initial_fence: u64,
}
impl QueueConfig {
    pub fn new(kind: QueueKind) -> Self {
        Self {
            kind,
            initial_fence: 0,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum LifecycleError {
    Limit(&'static str),
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
    WaitNotObserved(FencePoint),
}
#[derive(Debug)]
pub struct OwnedError<T, E> {
    pub owner: T,
    pub error: E,
}

/// Immutable real planner output. Constructing this runs the existing planner;
/// callers cannot inject a mutated `Plan` into the executor.
#[derive(Debug)]
pub struct PreparedPlan {
    plan: Plan,
}
impl PreparedPlan {
    pub fn from_graph(graph: &RenderGraph) -> Result<Self, PlanError> {
        graph.plan().map(|plan| Self { plan })
    }
    pub fn plan(&self) -> &Plan {
        &self.plan
    }
}

#[derive(Debug)]
pub struct ReadyAllocator<A> {
    timeline: TimelineId,
    value: A,
}
impl<A> ReadyAllocator<A> {
    pub fn timeline(&self) -> TimelineId {
        self.timeline
    }
    pub fn value(&self) -> &A {
        &self.value
    }
    pub fn into_inner(self) -> A {
        self.value
    }
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct QueueBinding {
    pub queue: QueueId,
    pub timeline: TimelineId,
}

#[derive(Debug)]
pub struct Recording;
#[derive(Debug)]
pub struct Closed;
#[derive(Debug)]
pub struct Uninitialized;
#[derive(Debug)]
pub struct Ready;

#[derive(Debug)]
pub struct WorkGraphScratch<S, T> {
    program: [u8; 32],
    backing: T,
    runtime: Option<u64>,
    state: PhantomData<S>,
}
impl<T> WorkGraphScratch<Uninitialized, T> {
    pub fn new(program: [u8; 32], backing: T) -> Self {
        Self {
            program,
            backing,
            runtime: None,
            state: PhantomData,
        }
    }
}
impl<S, T> WorkGraphScratch<S, T> {
    pub fn program(&self) -> &[u8; 32] {
        &self.program
    }
    pub fn backing(&self) -> &T {
        &self.backing
    }
    pub fn into_backing(self) -> T {
        self.backing
    }
}
/// Unsubmitted scratch returned with its original initialization state.
///
/// Merely creating or queuing initialization cannot manufacture `Ready`:
/// ```compile_fail
/// use vesper_ignis::lifecycle::{WorkGraphScratch, Uninitialized, Ready};
/// fn dispatch(_: WorkGraphScratch<Ready, ()>) {}
/// let scratch = WorkGraphScratch::<Uninitialized, _>::new([0; 32], ());
/// dispatch(scratch);
/// ```
#[derive(Debug)]
pub enum ScratchOwner<T> {
    Uninitialized(WorkGraphScratch<Uninitialized, T>),
    Ready(WorkGraphScratch<Ready, T>),
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ScratchAction {
    Initialize,
    Use,
}
#[derive(Debug)]
pub struct ScratchLease<T> {
    pass: PassId,
    owner: ScratchOwner<T>,
}
impl<T> ScratchLease<T> {
    pub fn pass(&self) -> PassId {
        self.pass
    }
    pub fn action(&self) -> ScratchAction {
        match &self.owner {
            ScratchOwner::Uninitialized(_) => ScratchAction::Initialize,
            ScratchOwner::Ready(_) => ScratchAction::Use,
        }
    }
    pub fn program(&self) -> &[u8; 32] {
        match &self.owner {
            ScratchOwner::Uninitialized(s) => &s.program,
            ScratchOwner::Ready(s) => &s.program,
        }
    }
    pub fn backing(&self) -> &T {
        match &self.owner {
            ScratchOwner::Uninitialized(s) => &s.backing,
            ScratchOwner::Ready(s) => &s.backing,
        }
    }
    fn retire(self, runtime: u64) -> WorkGraphScratch<Ready, T> {
        match self.owner {
            ScratchOwner::Uninitialized(s) => WorkGraphScratch {
                program: s.program,
                backing: s.backing,
                runtime: Some(runtime),
                state: PhantomData,
            },
            ScratchOwner::Ready(s) => s,
        }
    }
}

#[derive(Debug)]
pub struct BatchInputs<A, R> {
    pub plan: PreparedPlan,
    pub bindings: Vec<QueueBinding>,
    /// Exactly one allocator per binding, in the same order.
    pub allocators: Vec<ReadyAllocator<A>>,
    pub resources: R,
}
#[derive(Debug)]
pub struct CommandBatch<S, A, R, T> {
    runtime: u64,
    inputs: BatchInputs<A, R>,
    scratch: Vec<ScratchLease<T>>,
    max_scratch: usize,
    state: PhantomData<S>,
}
/// A batch can be closed exactly once through the consuming host API.
/// ```compile_fail
/// use vesper_ignis::lifecycle::RecordingBatch;
/// fn twice<A, R, T>(batch: RecordingBatch<A, R, T>) {
///     let first = batch.close();
///     let second = batch.close();
/// }
/// ```
pub type RecordingBatch<A, R, T> = CommandBatch<Recording, A, R, T>;
pub type ClosedBatch<A, R, T> = CommandBatch<Closed, A, R, T>;
#[derive(Debug)]
pub struct BatchParts<A, R, T> {
    pub inputs: BatchInputs<A, R>,
    pub scratch: Vec<ScratchOwner<T>>,
}
impl<S, A, R, T> CommandBatch<S, A, R, T> {
    pub fn plan(&self) -> &Plan {
        self.inputs.plan.plan()
    }
    pub fn into_parts(self) -> BatchParts<A, R, T> {
        BatchParts {
            inputs: self.inputs,
            scratch: self.scratch.into_iter().map(|s| s.owner).collect(),
        }
    }
}
impl<A, R, T> RecordingBatch<A, R, T> {
    fn scratch_pass(&self, pass: PassId) -> Result<(), LifecycleError> {
        engine::scratch_check(self.plan(), pass, self.scratch.len(), self.max_scratch)
    }
    pub fn initialize_scratch(
        &mut self,
        pass: PassId,
        scratch: WorkGraphScratch<Uninitialized, T>,
    ) -> Result<(), OwnedError<WorkGraphScratch<Uninitialized, T>, LifecycleError>> {
        if let Err(error) = self.scratch_pass(pass) {
            return Err(OwnedError {
                owner: scratch,
                error,
            });
        }
        self.scratch.push(ScratchLease {
            pass,
            owner: ScratchOwner::Uninitialized(scratch),
        });
        Ok(())
    }
    pub fn use_scratch(
        &mut self,
        pass: PassId,
        scratch: WorkGraphScratch<Ready, T>,
    ) -> Result<(), OwnedError<WorkGraphScratch<Ready, T>, LifecycleError>> {
        if scratch.runtime != Some(self.runtime) {
            return Err(OwnedError {
                owner: scratch,
                error: LifecycleError::ForeignTimeline,
            });
        }
        if let Err(error) = self.scratch_pass(pass) {
            return Err(OwnedError {
                owner: scratch,
                error,
            });
        }
        self.scratch.push(ScratchLease {
            pass,
            owner: ScratchOwner::Ready(scratch),
        });
        Ok(())
    }
    pub fn close(self) -> ClosedBatch<A, R, T> {
        CommandBatch {
            runtime: self.runtime,
            inputs: self.inputs,
            scratch: self.scratch,
            max_scratch: self.max_scratch,
            state: PhantomData,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum QueueCommand {
    Wait(FencePoint),
    Barriers(Vec<Barrier>),
    Scratch {
        index: usize,
        action: ScratchAction,
        program: [u8; 32],
    },
    Execute(PassId),
    Signal(FencePoint),
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SubmissionStream {
    pub timeline: TimelineId,
    pub kind: QueueKind,
    pub commands: Vec<QueueCommand>,
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Submission {
    pub ticket: FrameTicket,
    pub streams: Vec<SubmissionStream>,
    /// An additional original signal after each whole queue stream, even if
    /// that stream had no planner-generated signal. All are needed to retire.
    pub terminal: Vec<FencePoint>,
}

/// Borrowed submission views expire when `Backend::submit` returns. In-flight
/// records may move as the queue grows/retires: adapters must copy native handles
/// and separately own/pin actual GPU/DMA-visible host buffers. Addresses of these
/// Rust wrappers are not a persistent native ABI.
pub struct OwnerViews<'a, A, R, T> {
    pub allocators: &'a [ReadyAllocator<A>],
    pub resources: &'a R,
    pub scratch: &'a [ScratchLease<T>],
}
/// `Rejected` guarantees that NO work was accepted. After partial/uncertain
/// publication a backend must return `Uncertain`; owners remain in flight and
/// further submissions are blocked until original terminal completion arrives.
#[derive(Debug)]
pub enum BackendSubmit<E> {
    Accepted,
    Rejected(E),
    Uncertain(E),
}
#[derive(Debug)]
pub struct PollBatch<E> {
    pub completions: Vec<Completion>,
    /// Errors do not discard already harvested original completions.
    pub error: Option<E>,
}
/// Trusted device boundary. An implementation must keep command/resource
/// references usable until the ORIGINAL signal completes. Unwinding during
/// submission leaves the flight conservatively uncertain. `poll` is nonblocking
/// and respects its event bound.
///
/// The caller/backend must provide real ownership of nonaliased allocator and
/// scratch handles. Generic `Allocator`/`Scratch` payloads can contain clones of
/// one native object; the host wrapper cannot detect that aliasing. The backend
/// must also validate graph-resource/native-handle mappings and actual device
/// compatibility. `Completion::reported` trusts this backend's original-fence
/// observation; it is not hardware-authenticated evidence.
pub trait Backend {
    type Allocator;
    type Resources;
    type Scratch;
    type Error;
    fn submit(
        &mut self,
        submission: &Submission,
        owners: OwnerViews<'_, Self::Allocator, Self::Resources, Self::Scratch>,
    ) -> BackendSubmit<Self::Error>;
    fn poll(&mut self, max_events: usize) -> PollBatch<Self::Error>;
}

#[derive(Debug)]
pub enum SubmitError<E> {
    Lifecycle(LifecycleError),
    Backend(E),
}
#[derive(Debug)]
pub enum SubmitResult<A, R, T, E> {
    Submitted(FrameTicket),
    Rejected(OwnedError<ClosedBatch<A, R, T>, SubmitError<E>>),
    Uncertain { ticket: FrameTicket, error: E },
}
#[derive(Debug)]
pub struct RetiredFenceReceipt {
    ticket: FrameTicket,
    points: Vec<FencePoint>,
}
impl RetiredFenceReceipt {
    pub fn ticket(&self) -> FrameTicket {
        self.ticket
    }
    pub fn points(&self) -> &[FencePoint] {
        &self.points
    }
}
#[derive(Debug)]
pub struct RetiredFrame<A, R, T> {
    pub ticket: FrameTicket,
    pub allocators: Vec<ReadyAllocator<A>>,
    pub resources: R,
    /// Initialization only produces Ready here, after original completion.
    pub scratch: Vec<WorkGraphScratch<Ready, T>>,
    pub receipt: RetiredFenceReceipt,
}
struct Flight<A, R, T> {
    batch: ClosedBatch<A, R, T>,
    submission: Submission,
}

type Retired<B> =
    RetiredFrame<<B as Backend>::Allocator, <B as Backend>::Resources, <B as Backend>::Scratch>;
type ClosedFor<B> =
    ClosedBatch<<B as Backend>::Allocator, <B as Backend>::Resources, <B as Backend>::Scratch>;
type RecordingFor<B> =
    RecordingBatch<<B as Backend>::Allocator, <B as Backend>::Resources, <B as Backend>::Scratch>;
type InputsFor<B> = BatchInputs<<B as Backend>::Allocator, <B as Backend>::Resources>;

/// What a submission carries into the fence ledger: the plan, its queue
/// bindings, each allocator's timeline and the scratch leases in order.
pub(crate) struct BatchView<'a> {
    pub(crate) runtime: u64,
    pub(crate) plan: &'a Plan,
    pub(crate) bindings: &'a [QueueBinding],
    pub(crate) allocators: Vec<TimelineId>,
    pub(crate) scratch: Vec<(PassId, ScratchAction, [u8; 32])>,
}
fn view<A, R>(runtime: u64, inputs: &BatchInputs<A, R>) -> BatchView<'_> {
    BatchView {
        runtime,
        plan: inputs.plan.plan(),
        bindings: &inputs.bindings,
        allocators: inputs.allocators.iter().map(|a| a.timeline).collect(),
        scratch: Vec::new(),
    }
}

/// Retains every submitted allocator, resource bundle, and scratch owner.
/// No mutable backend access is exposed while work is pending. The fence
/// bookkeeping (reservations, observations, in-flight waits and signals)
/// lives in the [`Ledger`]; this wrapper owns the payloads.
pub struct Runtime<B: Backend> {
    ledger: Ledger,
    backend: Option<B>,
    inflight: VecDeque<Flight<B::Allocator, B::Resources, B::Scratch>>,
}
impl<B: Backend> Drop for Runtime<B> {
    fn drop(&mut self) {
        if !self.inflight.is_empty() {
            // A destructor cannot return owners or prove completion. Leaking is
            // conservative; releasing GPU-visible payloads here would be unsafe.
            std::mem::forget(std::mem::take(&mut self.inflight));
            if let Some(backend) = self.backend.take() {
                std::mem::forget(backend);
            }
        }
    }
}
impl<B: Backend> Runtime<B> {
    pub fn new(
        backend: B,
        queues: Vec<QueueConfig>,
        limits: Limits,
    ) -> Result<Self, OwnedError<B, LifecycleError>> {
        if let Err(error) = Ledger::check(&queues, limits) {
            return Err(OwnedError {
                owner: backend,
                error,
            });
        }
        let Ok(id) =
            NEXT_RUNTIME.try_update(Ordering::Relaxed, Ordering::Relaxed, |n| n.checked_add(1))
        else {
            return Err(OwnedError {
                owner: backend,
                error: LifecycleError::InstanceIdsExhausted,
            });
        };
        Ok(Self {
            ledger: Ledger::new(id, &queues, limits),
            backend: Some(backend),
            inflight: VecDeque::new(),
        })
    }
    pub fn timeline(&self, index: usize) -> Option<TimelineId> {
        (index < self.ledger.queue_count()).then_some(TimelineId {
            runtime: self.ledger.id(),
            index,
        })
    }
    pub fn observed(&self, timeline: TimelineId) -> Result<u64, LifecycleError> {
        self.ledger.observed(timeline)
    }
    pub fn allocator(
        &self,
        timeline: TimelineId,
        value: B::Allocator,
    ) -> Result<ReadyAllocator<B::Allocator>, OwnedError<B::Allocator, LifecycleError>> {
        if let Err(error) = self.ledger.observed(timeline) {
            return Err(OwnedError {
                owner: value,
                error,
            });
        }
        Ok(ReadyAllocator { timeline, value })
    }
    pub fn pending(&self) -> Vec<FrameTicket> {
        self.inflight.iter().map(|f| f.submission.ticket).collect()
    }
    pub fn submission(&self, ticket: FrameTicket) -> Option<&Submission> {
        self.inflight
            .iter()
            .find(|f| f.submission.ticket == ticket)
            .map(|f| &f.submission)
    }
    // Return all caller owners by value; failure must not allocate a Box.
    #[allow(clippy::result_large_err)]
    pub fn record(
        &self,
        inputs: InputsFor<B>,
    ) -> Result<RecordingFor<B>, OwnedError<InputsFor<B>, LifecycleError>> {
        if let Err(error) = self.ledger.validate(&view(self.ledger.id(), &inputs)) {
            return Err(OwnedError {
                owner: inputs,
                error,
            });
        }
        Ok(CommandBatch {
            runtime: self.ledger.id(),
            inputs,
            scratch: Vec::new(),
            max_scratch: self.ledger.max_scratch(),
            state: PhantomData,
        })
    }
    pub fn submit(
        &mut self,
        batch: ClosedFor<B>,
    ) -> SubmitResult<B::Allocator, B::Resources, B::Scratch, B::Error> {
        if self.inflight.try_reserve(1).is_err() {
            return SubmitResult::Rejected(OwnedError {
                owner: batch,
                error: SubmitError::Lifecycle(LifecycleError::Limit("inflight allocation")),
            });
        }
        let mut batch_view = view(batch.runtime, &batch.inputs);
        batch_view.scratch = batch
            .scratch
            .iter()
            .map(|s| (s.pass, s.action(), *s.program()))
            .collect();
        // Preflight, translation and fence reservation. The ledger records
        // the flight as uncertain until `settle`.
        let submission = match self.ledger.begin(&batch_view) {
            Ok(submission) => submission,
            Err(error) => {
                return SubmitResult::Rejected(OwnedError {
                    owner: batch,
                    error: SubmitError::Lifecycle(error),
                });
            }
        };
        let ticket = submission.ticket;
        // Publish ownership BEFORE calling the backend. Even backend unwinding
        // cannot drop GPU-visible owners: this uncertain flight is quarantined
        // by Runtime::drop. All allocations precede device publication.
        self.inflight.push_back(Flight { batch, submission });
        let flight = self.inflight.back().expect("reserved flight");
        let owners = OwnerViews {
            allocators: &flight.batch.inputs.allocators,
            resources: &flight.batch.inputs.resources,
            scratch: &flight.batch.scratch,
        };
        let result = self
            .backend
            .as_mut()
            .expect("live backend")
            .submit(&flight.submission, owners);
        match result {
            BackendSubmit::Rejected(error) => {
                // Definite rejection consumes a fence reservation (a legal gap)
                // but returns all command/allocator/resource/scratch owners.
                self.ledger.settle(Settle::Rejected);
                let flight = self.inflight.pop_back().expect("reserved flight");
                SubmitResult::Rejected(OwnedError {
                    owner: flight.batch,
                    error: SubmitError::Backend(error),
                })
            }
            BackendSubmit::Uncertain(error) => SubmitResult::Uncertain { ticket, error },
            BackendSubmit::Accepted => {
                self.ledger.settle(Settle::Accepted);
                SubmitResult::Submitted(ticket)
            }
        }
    }
    pub fn complete_original(
        &mut self,
        completion: Completion,
    ) -> Result<Vec<Retired<B>>, OwnedError<Completion, LifecycleError>> {
        let point = completion.point;
        if let Err(error) = self.ledger.check_completion(point) {
            return Err(OwnedError {
                owner: completion,
                error,
            });
        }
        let tickets = self.ledger.complete(point);
        let mut retired = Vec::with_capacity(tickets.len());
        for ticket in tickets {
            let Some(index) = self
                .inflight
                .iter()
                .position(|f| f.submission.ticket == ticket)
            else {
                continue;
            };
            let Flight { batch, submission } = self.inflight.remove(index).expect("index exists");
            let runtime = self.ledger.id();
            retired.push(RetiredFrame {
                ticket: submission.ticket,
                allocators: batch.inputs.allocators,
                resources: batch.inputs.resources,
                scratch: batch
                    .scratch
                    .into_iter()
                    .map(|s| s.retire(runtime))
                    .collect(),
                receipt: RetiredFenceReceipt {
                    ticket: submission.ticket,
                    points: submission.terminal,
                },
            });
        }
        Ok(retired)
    }
    /// Harvest nonblocking completions. Invalid/foreign/out-of-order reports and
    /// all reports after them are returned to the caller for inspection/retry.
    pub fn poll(&mut self, max_events: usize) -> PollReport<B> {
        if !self.ledger.poll_allowed(max_events) {
            return PollReport {
                retired: Vec::new(),
                unprocessed: Vec::new(),
                error: Some(PollError::Lifecycle(LifecycleError::Limit("poll events"))),
            };
        }
        let batch = self
            .backend
            .as_mut()
            .expect("live backend")
            .poll(max_events);
        let mut report = PollReport {
            retired: Vec::new(),
            unprocessed: Vec::new(),
            error: batch.error.map(PollError::Backend),
        };
        if batch.completions.len() > max_events {
            report.unprocessed = batch.completions;
            report.error = Some(PollError::with_lifecycle(
                report.error.take(),
                LifecycleError::Limit("backend exceeded poll bound"),
            ));
            return report;
        }
        let mut events = batch.completions.into_iter();
        while let Some(completion) = events.next() {
            match self.complete_original(completion) {
                Ok(mut retired) => report.retired.append(&mut retired),
                Err(OwnedError { owner, error }) => {
                    report.unprocessed.push(owner);
                    report.unprocessed.extend(events);
                    report.error = Some(PollError::with_lifecycle(report.error.take(), error));
                    break;
                }
            }
        }
        report
    }
    pub fn drain(&mut self, polls: usize, events_per_poll: usize) -> DrainReport<B> {
        let mut report = DrainReport {
            retired: Vec::new(),
            unprocessed: Vec::new(),
            error: None,
            pending: self.pending(),
            polls: 0,
        };
        if !self.ledger.drain_allowed(polls) {
            report.error = Some(PollError::Lifecycle(LifecycleError::Limit("drain polls")));
            return report;
        }
        for _ in 0..polls {
            if self.inflight.is_empty() {
                break;
            }
            let mut batch = self.poll(events_per_poll);
            report.polls += 1;
            report.retired.append(&mut batch.retired);
            report.unprocessed.append(&mut batch.unprocessed);
            if batch.error.is_some() {
                report.error = batch.error;
                break;
            }
        }
        report.pending = self.pending();
        report
    }
    pub fn shutdown(mut self, polls: usize, events_per_poll: usize) -> Shutdown<B> {
        let report = self.drain(polls, events_per_poll);
        if self.inflight.is_empty() {
            Shutdown::Complete {
                backend: self.backend.take().expect("live backend"),
                report,
            }
        } else {
            Shutdown::Incomplete {
                runtime: self,
                report,
            }
        }
    }
}
#[derive(Debug)]
pub enum PollError<E> {
    Lifecycle(LifecycleError),
    Backend(E),
    Both {
        lifecycle: LifecycleError,
        backend: E,
    },
}
impl<E> PollError<E> {
    fn with_lifecycle(previous: Option<Self>, lifecycle: LifecycleError) -> Self {
        match previous {
            Some(Self::Backend(backend) | Self::Both { backend, .. }) => {
                Self::Both { lifecycle, backend }
            }
            _ => Self::Lifecycle(lifecycle),
        }
    }
}
pub struct PollReport<B: Backend> {
    pub retired: Vec<Retired<B>>,
    pub unprocessed: Vec<Completion>,
    pub error: Option<PollError<B::Error>>,
}
pub struct DrainReport<B: Backend> {
    pub retired: Vec<Retired<B>>,
    pub unprocessed: Vec<Completion>,
    pub error: Option<PollError<B::Error>>,
    pub pending: Vec<FrameTicket>,
    pub polls: usize,
}
pub enum Shutdown<B: Backend> {
    Complete {
        backend: B,
        report: DrainReport<B>,
    },
    Incomplete {
        runtime: Runtime<B>,
        report: DrainReport<B>,
    },
}
