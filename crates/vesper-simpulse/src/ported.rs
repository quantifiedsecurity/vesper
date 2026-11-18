//! The Vesper build of [`Simulator`]: every scheduling decision runs in
//! vesper/src/lib.ves (`Kernel`). This wrapper keeps what Vesper cannot hold:
//! the generic payloads (a token slab indexed by task id), the simulator
//! instance identity that makes another simulator's token foreign, and the
//! serde [`Journal`], which it extends with the events each call commits so
//! the trace bytes are those of the Rust build.
use super::*;
use crate::binding::{Ev, Kernel, Ret};

/// A single-threaded simulator permits at most one checked-out running token.
/// With a fixed finite runnable set, each task is dispatched once per round.
pub struct Simulator<T> {
    instance: u64,
    core: Kernel,
    /// Payload of task `t` at `t - 1`; `None` while its token is out or
    /// after it finished.
    payloads: Vec<Option<T>>,
    journal: Journal,
    recording: bool,
}

impl<T> Simulator<T> {
    pub fn new(seed: u64) -> Self {
        Self {
            instance: next_instance(),
            core: Kernel::new(seed),
            payloads: Vec::new(),
            journal: Journal {
                version: JOURNAL_VERSION,
                seed,
                events: vec![],
            },
            recording: true,
        }
    }
    pub fn replay(journal: Journal) -> Result<Self, Error> {
        journal.validate_version()?;
        let events: Vec<Ev> = journal.events.iter().map(encode_event).collect();
        Ok(Self {
            instance: next_instance(),
            core: Kernel::replay(journal.seed, &events),
            payloads: Vec::new(),
            journal,
            recording: false,
        })
    }
    pub fn now_ns(&self) -> u64 {
        self.core.now_ns()
    }
    pub fn task_state(&self, task: TaskId) -> Option<TaskState> {
        let st = self.core.task_state(task.0);
        let completion = CompletionId(st.completion);
        match st.kind {
            0 => Some(TaskState::Ready),
            1 => Some(TaskState::Running),
            2 => Some(TaskState::Sleeping {
                completion,
                deadline_ns: st.deadline,
            }),
            3 => Some(TaskState::WaitingIo { completion }),
            4 => Some(TaskState::Finished),
            _ => None,
        }
    }
    pub fn cancel_requested(&self, task: TaskId) -> Option<bool> {
        match self.core.cancel_requested(task.0) {
            0 => Some(false),
            1 => Some(true),
            _ => None,
        }
    }
    pub fn pending_count(&self) -> usize {
        self.core.pending_count() as usize
    }
    pub fn is_complete(&self) -> bool {
        self.core.is_complete()
    }
    pub fn journal(&self) -> &Journal {
        &self.journal
    }
    /// Verify that the full recorded prefix was consumed. Separately check
    /// `is_complete()` if the recording represents a complete program run.
    pub fn finish_replay(&self) -> Result<(), Error> {
        match self.core.unconsumed() {
            0 => Ok(()),
            remaining => Err(Error::ReplayUnconsumed {
                remaining: remaining as usize,
            }),
        }
    }
    /// Append the events the last call committed to the recorded journal.
    fn sync(&mut self) {
        if self.recording {
            let view = self.core.as_ref();
            self.journal
                .events
                .extend(view.out().iter().map(|e| decode_event(&e)));
        }
    }
    fn error(&self, code: u64, value: u64) -> Error {
        match code {
            1 => Error::UnknownTask(TaskId(value)),
            2 => Error::UnknownCompletion(CompletionId(value)),
            3 => Error::TaskFinished(TaskId(value)),
            4 => Error::TaskStillRunning(TaskId(value)),
            5 => Error::InvalidToken(TaskId(value)),
            6 => Error::ForeignToken,
            7 => Error::IdExhausted,
            8 => Error::ClockOverflow,
            9 => Error::RoundOverflow,
            _ => {
                let index = value as usize;
                Error::ReplayMismatch {
                    index,
                    expected: self.journal.events.get(index).cloned().map(Box::new),
                    actual: Box::new(decode_event(&self.core.mismatch())),
                }
            }
        }
    }
    fn result(&mut self, ret: Ret) -> Result<u64, Error> {
        self.sync();
        if ret.code == 0 {
            Ok(ret.value)
        } else {
            Err(self.error(ret.code, ret.value))
        }
    }
    fn own(&self, token: &RunToken<T>) -> bool {
        token.instance == self.instance
    }
    /// A transition of `token`'s task: on success its payload goes back to
    /// the slab, on failure the token goes back to the caller.
    fn park(&mut self, token: RunToken<T>, ret: Ret) -> Result<u64, TransitionError<T>> {
        match self.result(ret) {
            Ok(value) => {
                self.payloads[(token.task.0 - 1) as usize] = Some(token.payload);
                Ok(value)
            }
            Err(error) => Err(TransitionError { error, token }),
        }
    }
    pub fn spawn(&mut self, payload: T) -> Result<TaskId, SpawnError<T>> {
        let ret = self.core.spawn();
        match self.result(ret) {
            Ok(task) => {
                self.payloads.push(Some(payload));
                Ok(TaskId(task))
            }
            Err(error) => Err(SpawnError { error, payload }),
        }
    }
    pub fn yield_task(&mut self, token: RunToken<T>) -> Result<(), TransitionError<T>> {
        let ret = self.core.yield_task(token.task.0, self.own(&token));
        self.park(token, ret).map(|_| ())
    }
    pub fn finish(&mut self, token: RunToken<T>) -> Result<T, TransitionError<T>> {
        let ret = self.core.finish(token.task.0, self.own(&token));
        match self.result(ret) {
            Ok(_) => Ok(token.payload),
            Err(error) => Err(TransitionError { error, token }),
        }
    }
    pub fn sleep(
        &mut self,
        token: RunToken<T>,
        duration_ns: u64,
    ) -> Result<CompletionId, TransitionError<T>> {
        let ret = self.core.sleep(token.task.0, self.own(&token), duration_ns);
        self.park(token, ret).map(CompletionId)
    }
    pub fn park_io(&mut self, token: RunToken<T>) -> Result<CompletionId, TransitionError<T>> {
        let ret = self.core.park_io(token.task.0, self.own(&token));
        self.park(token, ret).map(CompletionId)
    }
    /// Inject a completion at `now + delay`, including failures and completion
    /// cancellation. Requesting cancellation never modifies this outcome.
    pub fn schedule_io(
        &mut self,
        token: RunToken<T>,
        delay_ns: u64,
        result: IoResult,
    ) -> Result<CompletionId, TransitionError<T>> {
        let (rk, rv) = encode_result(result);
        let ret = self
            .core
            .schedule_io(token.task.0, self.own(&token), delay_ns, rk, rv);
        self.park(token, ret).map(CompletionId)
    }
    pub fn request_cancel(&mut self, id: TaskId) -> Result<bool, Error> {
        let ret = self.core.request_cancel(id.0);
        self.result(ret).map(|recorded| recorded == 1)
    }
    pub fn complete_io(&mut self, completion: CompletionId, result: IoResult) -> Result<(), Error> {
        let (rk, rv) = encode_result(result);
        let ret = self.core.complete_io(completion.0, rk, rv);
        self.result(ret).map(|_| ())
    }
    /// Return one runnable owner. If all tasks are parked, jump to the earliest
    /// scheduled deadline. External pending I/O with no scheduled event returns
    /// None without advancing time; that is quiescence, not program completion.
    pub fn next_task(&mut self) -> Result<Option<RunToken<T>>, Error> {
        let next = self.core.next_task();
        self.result(Ret {
            code: next.code,
            value: next.task,
        })?;
        if next.task == 0 {
            return Ok(None);
        }
        let completion = CompletionId(next.wc);
        let reason = match next.wk {
            0 => WakeReason::Start,
            1 => WakeReason::Yield,
            2 => WakeReason::Timer(completion),
            _ => WakeReason::Io {
                completion,
                result: decode_result(next.wrk, next.wrv),
            },
        };
        let payload = self.payloads[(next.task - 1) as usize]
            .take()
            .expect("a dispatched task's payload is in the slab");
        Ok(Some(RunToken {
            instance: self.instance,
            task: TaskId(next.task),
            payload,
            reason,
            cancel_requested: next.cancel,
        }))
    }
}

fn encode_result(result: IoResult) -> (u64, u64) {
    match result {
        IoResult::Success { bytes } => (0, bytes),
        IoResult::Error { code } => (1, i64::from(code) as u64),
        IoResult::Cancelled => (2, 0),
    }
}

fn decode_result(rk: u64, rv: u64) -> IoResult {
    match rk {
        0 => IoResult::Success { bytes: rv },
        1 => IoResult::Error {
            code: rv as i64 as i32,
        },
        _ => IoResult::Cancelled,
    }
}

fn ev(kind: u64, task: TaskId, completion: u64, a: u64, b: u64) -> Ev {
    Ev {
        kind,
        task: task.0,
        completion,
        a,
        b,
        rk: 0,
        rv: 0,
    }
}

/// The word form of an event (the codes are listed in vesper/src/lib.ves).
fn encode_event(event: &TraceEvent) -> Ev {
    match *event {
        TraceEvent::Spawn { task, at_ns } => ev(0, task, 0, at_ns, 0),
        TraceEvent::Dispatch { task, at_ns, round } => ev(1, task, 0, at_ns, round),
        TraceEvent::Yield { task, at_ns } => ev(2, task, 0, at_ns, 0),
        TraceEvent::Sleep {
            task,
            completion,
            deadline_ns,
        } => ev(3, task, completion.0, deadline_ns, 0),
        TraceEvent::SubmitIo {
            task,
            completion,
            scheduled,
        } => match scheduled {
            None => ev(4, task, completion.0, 0, 0),
            Some((time, result)) => {
                let (rk, rv) = encode_result(result);
                Ev {
                    rk,
                    rv,
                    ..ev(4, task, completion.0, time, 1)
                }
            }
        },
        TraceEvent::ClockAdvance { from_ns, to_ns } => ev(5, TaskId(0), 0, from_ns, to_ns),
        TraceEvent::TimerElapsed {
            task,
            completion,
            at_ns,
        } => ev(6, task, completion.0, at_ns, 0),
        TraceEvent::IoCompleted {
            task,
            completion,
            at_ns,
            result,
        } => {
            let (rk, rv) = encode_result(result);
            Ev {
                rk,
                rv,
                ..ev(7, task, completion.0, at_ns, 0)
            }
        }
        TraceEvent::CancelRequested { task, at_ns } => ev(8, task, 0, at_ns, 0),
        TraceEvent::Finish { task, at_ns } => ev(9, task, 0, at_ns, 0),
    }
}

fn decode_event(e: &Ev) -> TraceEvent {
    let task = TaskId(e.task);
    let completion = CompletionId(e.completion);
    match e.kind {
        0 => TraceEvent::Spawn { task, at_ns: e.a },
        1 => TraceEvent::Dispatch {
            task,
            at_ns: e.a,
            round: e.b,
        },
        2 => TraceEvent::Yield { task, at_ns: e.a },
        3 => TraceEvent::Sleep {
            task,
            completion,
            deadline_ns: e.a,
        },
        4 => TraceEvent::SubmitIo {
            task,
            completion,
            scheduled: (e.b == 1).then(|| (e.a, decode_result(e.rk, e.rv))),
        },
        5 => TraceEvent::ClockAdvance {
            from_ns: e.a,
            to_ns: e.b,
        },
        6 => TraceEvent::TimerElapsed {
            task,
            completion,
            at_ns: e.a,
        },
        7 => TraceEvent::IoCompleted {
            task,
            completion,
            at_ns: e.a,
            result: decode_result(e.rk, e.rv),
        },
        8 => TraceEvent::CancelRequested { task, at_ns: e.a },
        _ => TraceEvent::Finish { task, at_ns: e.a },
    }
}
