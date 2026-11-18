//! An outside event loop (a GLib main context, another reactor) can wait on
//! a driver's pollable descriptor instead of blocking in `poll(true)`: it
//! turns readable when a completion is due and quiet again once harvested.
//! Runs on every Unix driver this host can start.
#![cfg(unix)]
use std::{
    fs::File,
    io::{self, Write},
    os::fd::{AsRawFd, BorrowedFd},
};
use vesper_pulse::driver::{CompletionDriver, DriverKind, Operation, Request};
use vesper_pulse::select::{AnyDriver, SelectError, host_order};

fn drivers() -> Vec<AnyDriver<u32>> {
    let mut out = vec![];
    for &kind in host_order() {
        match AnyDriver::start(kind, 16) {
            Ok(d) => out.push(d),
            Err(SelectError::Unavailable(s)) if kind == DriverKind::IoUring => {
                eprintln!("skipping io_uring on this host: {}", s.error);
            }
            Err(e) => panic!("{kind} failed to start: {e}"),
        }
    }
    assert!(!out.is_empty());
    out
}

/// poll(2) the descriptor for readability with a timeout in milliseconds.
fn readable(fd: BorrowedFd<'_>, timeout_ms: i32) -> bool {
    let mut p = libc::pollfd {
        fd: fd.as_raw_fd(),
        events: libc::POLLIN,
        revents: 0,
    };
    // SAFETY: one live pollfd.
    let n = unsafe { libc::poll(&mut p, 1, timeout_ms) };
    assert!(n >= 0, "poll: {}", io::Error::last_os_error());
    n == 1 && p.revents & libc::POLLIN != 0
}

fn pipe() -> (File, File) {
    let (r, w) = io::pipe().unwrap();
    (
        File::from(std::os::fd::OwnedFd::from(r)),
        File::from(std::os::fd::OwnedFd::from(w)),
    )
}

/// Harvest what is due without blocking; an outside loop may wake
/// spuriously, so allow a few empty rounds.
fn harvest(d: &mut AnyDriver<u32>) -> Vec<(u32, io::Result<usize>, Vec<u8>)> {
    let mut out = vec![];
    for _ in 0..4 {
        for c in d.poll(false).unwrap() {
            out.push((c.request.continuation, c.result, c.request.buffer));
        }
        if !out.is_empty() {
            break;
        }
        let fd = d.pollable_fd().unwrap();
        readable(fd, 1000);
    }
    out
}

#[test]
fn the_pollable_descriptor_wakes_an_outside_loop_for_each_completion() {
    for mut d in drivers() {
        let kind = d.kind();
        eprintln!("pollable descriptor on {kind}");
        let fd = d.pollable_fd().unwrap().as_raw_fd();
        assert!(fd >= 0);
        // Settle any wake-up from registration.
        let _ = d.poll(false).unwrap();
        let (r, mut w) = pipe();
        d.submit(Request {
            file: r,
            buffer: vec![0; 4],
            continuation: 7,
            operation: Operation::Read,
            offset: 0,
        })
        .map_err(|(e, _)| e)
        .unwrap();
        assert!(!d.has_ready(), "{kind}: an empty pipe is not ready");
        // No poll between submit and the wait: the driver must already have
        // handed the request to the kernel.
        assert!(
            !readable(d.pollable_fd().unwrap(), 50),
            "{kind}: readable before any data"
        );
        w.write_all(b"ping").unwrap();
        assert!(
            readable(d.pollable_fd().unwrap(), 5000),
            "{kind}: not readable after the write"
        );
        let got = harvest(&mut d);
        assert_eq!(got.len(), 1, "{kind}");
        let (k, result, buffer) = &got[0];
        assert_eq!(*k, 7);
        assert_eq!(*result.as_ref().unwrap(), 4);
        assert_eq!(buffer, b"ping");
        assert!(
            !readable(d.pollable_fd().unwrap(), 50),
            "{kind}: still readable after harvesting"
        );
        assert_eq!(d.pending(), 0);
        // A cancellation is a completion too: it must wake the loop
        // without a poll in between.
        let (r, _w) = pipe();
        let id = d
            .submit(Request {
                file: r,
                buffer: vec![0; 4],
                continuation: 8,
                operation: Operation::Read,
                offset: 0,
            })
            .map_err(|(e, _)| e)
            .unwrap();
        assert!(d.cancel(id).unwrap());
        assert!(
            d.has_ready() || readable(d.pollable_fd().unwrap(), 5000),
            "{kind}: a cancellation did not wake the loop"
        );
        let got = harvest(&mut d);
        assert_eq!(got.len(), 1, "{kind}");
        assert_eq!(got[0].0, 8);
        assert_eq!(d.pending(), 0);
    }
}

#[test]
fn completions_finished_at_submission_are_reported_by_has_ready() {
    for mut d in drivers() {
        let kind = d.kind();
        let (r, _w) = pipe();
        // A zero-length read completes without waiting for readiness.
        d.submit(Request {
            file: r,
            buffer: vec![],
            continuation: 1,
            operation: Operation::Read,
            offset: 0,
        })
        .map_err(|(e, _)| e)
        .unwrap();
        // The readiness drivers finish it at submission: the descriptor
        // never fires for it, so `has_ready` must say so. io_uring posts it
        // to the ring like any other completion.
        if kind != DriverKind::IoUring {
            assert!(d.has_ready(), "{kind}");
        }
        let got = harvest(&mut d);
        assert_eq!(got.len(), 1, "{kind}");
        assert_eq!(*got[0].1.as_ref().unwrap(), 0);
        assert_eq!(d.pending(), 0, "{kind}");
        assert!(!d.has_ready(), "{kind}");
    }
}
