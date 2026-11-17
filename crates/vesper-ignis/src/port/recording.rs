//! Vesper build of the recording backend: the queue model (submit modes,
//! waits, signals, per-timeline ordering, poll failures) runs on a Vesper
//! `Recorder`; Rust keeps the submission history and implements `Backend`.

use super::binding as v;
use super::ffi;
use crate::lifecycle::{
    Backend, BackendSubmit, Completion, FencePoint, OwnerViews, PollBatch, QueueCommand,
    Submission, TimelineId,
};
use crate::recording::{RecordingError, SubmitMode};
use std::marker::PhantomData;

type OwnersMarker<A, R, T> = PhantomData<fn() -> (A, R, T)>;

pub struct RecordingBackend<A, R, T> {
    history: Vec<Submission>,
    recorder: v::Recorder,
    owners: OwnersMarker<A, R, T>,
}
impl<A, R, T> Default for RecordingBackend<A, R, T> {
    fn default() -> Self {
        Self::new(256, 64, 16_384).expect("valid defaults")
    }
}
impl<A, R, T> RecordingBackend<A, R, T> {
    pub fn new(
        history_limit: usize,
        pending_limit: usize,
        work_per_poll: usize,
    ) -> Result<Self, &'static str> {
        let (h, p, w) = (
            history_limit as u64,
            pending_limit as u64,
            work_per_poll as u64,
        );
        if !v::recorder_bounds_ok(h, p, w) {
            return Err("recording backend bounds");
        }
        Ok(Self {
            history: Vec::new(),
            recorder: v::recorder_new(h, p, w),
            owners: PhantomData,
        })
    }
    pub fn push_submit_mode(&mut self, mode: SubmitMode) -> Result<(), &'static str> {
        if !v::recorder_push_mode(&mut self.recorder, mode) {
            return Err("submit script bound");
        }
        Ok(())
    }
    /// Whether the next poll reports a failure after returning harvested events.
    pub fn push_poll_failure(&mut self, fail: bool) -> Result<(), &'static str> {
        if !v::recorder_push_poll_failure(&mut self.recorder, fail) {
            return Err("poll script bound");
        }
        Ok(())
    }
    pub fn history(&self) -> &[Submission] {
        &self.history
    }
}

fn sim_point(p: FencePoint) -> ffi::Point {
    ffi::Point {
        runtime: p.timeline.runtime,
        index: p.timeline.index as u64,
        value: p.value,
    }
}

impl<A, R, T> Backend for RecordingBackend<A, R, T> {
    type Allocator = A;
    type Resources = R;
    type Scratch = T;
    type Error = RecordingError;
    fn submit(
        &mut self,
        submission: &Submission,
        _owners: OwnerViews<'_, A, R, T>,
    ) -> BackendSubmit<RecordingError> {
        let mut streams = Vec::with_capacity(submission.streams.len());
        let mut cmds = Vec::new();
        for s in &submission.streams {
            let first = cmds.len() as u64;
            for c in &s.commands {
                cmds.push(match c {
                    QueueCommand::Wait(p) => ffi::SimCmd {
                        wait: true,
                        signal: false,
                        point: sim_point(*p),
                    },
                    QueueCommand::Signal(p) => ffi::SimCmd {
                        wait: false,
                        signal: true,
                        point: sim_point(*p),
                    },
                    _ => ffi::SimCmd {
                        wait: false,
                        signal: false,
                        point: ffi::Point {
                            runtime: 0,
                            index: 0,
                            value: 0,
                        },
                    },
                });
            }
            streams.push(ffi::SimStream {
                runtime: s.timeline.runtime,
                index: s.timeline.index as u64,
                first,
                count: s.commands.len() as u64,
            });
        }
        let verdict = v::recorder_submit(
            &mut self.recorder,
            self.history.len() as u64,
            &streams,
            &cmds,
        );
        match verdict {
            ffi::RecorderVerdict::Refused(error) => BackendSubmit::Rejected(error),
            ffi::RecorderVerdict::Stalled => BackendSubmit::Uncertain(RecordingError::Uncertain),
            ffi::RecorderVerdict::Recorded { uncertain, panic } => {
                self.history.push(submission.clone());
                assert!(!panic, "recording backend fault after acceptance");
                if uncertain {
                    BackendSubmit::Uncertain(RecordingError::Uncertain)
                } else {
                    BackendSubmit::Accepted
                }
            }
        }
    }
    fn poll(&mut self, max_events: usize) -> PollBatch<RecordingError> {
        let completions = v::recorder_poll(&mut self.recorder, max_events as u64)
            .into_iter()
            .map(|p| {
                Completion::reported(FencePoint {
                    timeline: TimelineId {
                        runtime: p.runtime,
                        index: p.index as usize,
                    },
                    value: p.value,
                })
            })
            .collect();
        let error =
            v::recorder_pop_poll_failure(&mut self.recorder).then_some(RecordingError::PollFailure);
        PollBatch { completions, error }
    }
}
