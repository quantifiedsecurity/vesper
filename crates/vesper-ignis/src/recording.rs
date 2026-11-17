//! Deterministic CPU recording backend for lifecycle tests/embedding examples.
//!
//! Executes no shaders and calls no GPU APIs. Waits/signals and queue ordering
//! are simulated; its completion reports establish only this host model.

pub use crate::engine::recording::RecordingBackend;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SubmitMode {
    Accept,
    Reject,
    /// Work is queued, but the caller cannot determine acceptance yet.
    AcceptUncertain,
    /// Only the first queue stream is published; remaining streams and their
    /// terminal signals never execute. Owners must remain in incomplete shutdown.
    AcceptFirstQueueUncertain,
    /// Publication may have happened, and no completion can be established by
    /// this model. Shutdown must return the pending owners indefinitely.
    StallUncertain,
    /// Simulate backend unwinding after it retained submitted work.
    PanicAfterAcceptance,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RecordingError {
    Rejected,
    Uncertain,
    HistoryLimit,
    PendingLimit,
    PollFailure,
}
