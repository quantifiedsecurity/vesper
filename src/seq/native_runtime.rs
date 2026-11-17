//! JIT runtime for `ChunkSeq[T]` (§13): header allocation and growth.
//!
//! A sequence handle points at a three-word header `{len, cap, data}`
//! ([`crate::seq::HEADER_BYTES`]). Generated code reads and writes elements
//! in place; only allocation, growth and drops call into this runtime.
//! Generated code frees the sequences and strings it drops
//! ([`vesper_seq_free`], [`vesper_str_release`]); the JIT records every live
//! header and grown string buffer on this thread and frees what is still live
//! in [`finish`]. [`live_allocations`] counts them, as the executable's
//! `vesper_seq_live` counter does. Executables define the same symbols in
//! generated code (`src/seq/aot_runtime.rs`).
//!
//! An allocation that cannot be satisfied (including a capacity whose size
//! overflows) ends the process with a message, as an executable exits with
//! status [`crate::seq::FAULT_STATUS`]: generated code has no way to continue
//! without the buffer it asked for.
use std::alloc::{Layout, alloc, dealloc, realloc};
use std::cell::RefCell;
use std::collections::HashMap;

const LEN: usize = 0;
const CAP: usize = 1;
const DATA: usize = 2;

thread_local! {
    /// Live headers and their element stride.
    static HEADERS: RefCell<HashMap<usize, u64>> = RefCell::new(HashMap::new());
    /// Data buffers grown for headers this runtime did not allocate (native
    /// strings, §13): data address to byte size.
    static BUFFERS: RefCell<HashMap<usize, usize>> = RefCell::new(HashMap::new());
    static FAULT: RefCell<Option<String>> = const { RefCell::new(None) };
    /// Allocations the last [`finish`] freed because generated code had not.
    static TEARDOWN: std::cell::Cell<usize> = const { std::cell::Cell::new(0) };
    /// Drops of a sequence or string buffer this runtime does not hold: a
    /// double free in an executable.
    static STRAY: std::cell::Cell<usize> = const { std::cell::Cell::new(0) };
}

/// Records a recoverable fault (code 1: a slice interval out of bounds;
/// code 2: a string interval off a char boundary). Generated code continues
/// with an empty view; [`finish`] reports it.
pub extern "C" fn vesper_seq_fault(code: u64) -> u64 {
    let message = match code {
        1 => "native ChunkSeq fault: slice interval out of bounds",
        2 => "native String fault: interval is not on char boundaries",
        _ => fault("out of memory"),
    };
    FAULT.with(|f| {
        f.borrow_mut().get_or_insert_with(|| message.into());
    });
    0
}

fn fault(message: &str) -> ! {
    eprintln!("vesper: ChunkSeq {message}");
    std::process::abort()
}

fn data_layout(cap: u64, stride: u64) -> Option<Layout> {
    let bytes = cap.checked_mul(stride.max(1))?;
    let bytes = usize::try_from(bytes).ok()?;
    Layout::from_size_align(bytes.max(1), 8).ok()
}

/// Allocates a header with room for `cap` elements of `stride` bytes.
pub extern "C" fn vesper_seq_new(cap: u64, stride: u64) -> u64 {
    let header_layout = Layout::new::<[u64; 3]>();
    // SAFETY: the header layout has a non-zero size.
    let header = unsafe { alloc(header_layout) }.cast::<u64>();
    if header.is_null() {
        fault("out of memory");
    }
    let data = if cap == 0 {
        std::ptr::null_mut()
    } else {
        let layout = data_layout(cap, stride)
            .unwrap_or_else(|| fault("capacity overflows the address space"));
        // SAFETY: `layout` has a non-zero size.
        let data = unsafe { alloc(layout) };
        if data.is_null() {
            fault("out of memory");
        }
        data
    };
    // SAFETY: `header` is valid for three words.
    unsafe {
        header.add(LEN).write(0);
        header.add(CAP).write(cap);
        header.add(DATA).write(data as u64);
    }
    HEADERS.with(|h| h.borrow_mut().insert(header as usize, stride));
    header as u64
}

/// Grows the buffer of `handle` to hold at least `needed` elements,
/// doubling its capacity (at least four elements).
pub extern "C" fn vesper_seq_grow(handle: u64, stride: u64, needed: u64) -> u64 {
    let header = handle as *mut u64;
    // SAFETY: generated code passes a live handle from `vesper_seq_new`.
    let (cap, data) = unsafe { (header.add(CAP).read(), header.add(DATA).read() as *mut u8) };
    if needed <= cap {
        return 0;
    }
    let new_cap = cap.saturating_mul(2).max(4).max(needed);
    let layout = data_layout(new_cap, stride)
        .unwrap_or_else(|| fault("capacity overflows the address space"));
    let tracked = HEADERS.with(|h| h.borrow().contains_key(&(header as usize)));
    let grown = if data.is_null() || cap == 0 {
        // SAFETY: `layout` has a non-zero size.
        let fresh = unsafe { alloc(layout) };
        if !data.is_null() && !fresh.is_null() {
            // Capacity 0 with data: borrowed bytes (a string literal or
            // view). Copy them before the first write.
            // SAFETY: `header` is valid; `len * stride` bytes are readable
            // at `data` and fit in the new buffer (`needed > len`).
            unsafe {
                let len = header.add(LEN).read();
                let bytes = usize::try_from(len.saturating_mul(stride.max(1)))
                    .unwrap_or_else(|_| fault("has a corrupt length"));
                std::ptr::copy_nonoverlapping(data, fresh, bytes);
            }
        }
        fresh
    } else {
        let old = data_layout(cap, stride).unwrap_or_else(|| fault("has a corrupt capacity"));
        // SAFETY: `data` was allocated with `old`, and the new size is non-zero.
        unsafe { realloc(data, old, layout.size()) }
    };
    if grown.is_null() {
        fault("out of memory");
    }
    if !tracked {
        BUFFERS.with(|b| {
            let mut b = b.borrow_mut();
            b.remove(&(data as usize));
            b.insert(grown as usize, layout.size());
        });
    }
    // SAFETY: `header` is valid for three words.
    unsafe {
        header.add(CAP).write(new_cap);
        header.add(DATA).write(grown as u64);
    }
    0
}

/// Frees a header and the buffer it owns.
///
/// # Safety
/// `header` came from [`vesper_seq_new`] with this `stride` and is freed once.
unsafe fn free_header(header: *mut u64, stride: u64) {
    // SAFETY: the caller passes a live header.
    unsafe {
        let cap = header.add(CAP).read();
        let data = header.add(DATA).read() as *mut u8;
        // Capacity 0 with data is borrowed bytes the header does not own.
        if !data.is_null()
            && cap != 0
            && let Some(layout) = data_layout(cap, stride)
        {
            dealloc(data, layout);
        }
        dealloc(header.cast::<u8>(), Layout::new::<[u64; 3]>());
    }
}

/// Frees a sequence that generated code drops: its header and buffer. A
/// handle this runtime does not know (0, or one already freed) is ignored,
/// so a drop can never free twice.
pub extern "C" fn vesper_seq_free(handle: u64) -> u64 {
    if let Some(stride) = HEADERS.with(|h| h.borrow_mut().remove(&(handle as usize))) {
        // SAFETY: the header was live and is now unrecorded, so it is freed once.
        unsafe { free_header(handle as *mut u64, stride) };
    } else if handle != 0 {
        STRAY.with(|s| s.set(s.get() + 1));
    }
    0
}

/// Frees the buffer an inline `String` value at `value` owns (borrowed
/// literal bytes, capacity 0, are not owned). A buffer this runtime did not
/// grow is ignored.
pub extern "C" fn vesper_str_release(value: u64) -> u64 {
    if value == 0 {
        return 0;
    }
    let header = value as *mut u64;
    // SAFETY: generated code passes the address of a live `String` value.
    let (cap, data) = unsafe { (header.add(CAP).read(), header.add(DATA).read()) };
    if cap == 0 || data == 0 {
        return 0;
    }
    if let Some(bytes) = BUFFERS.with(|b| b.borrow_mut().remove(&(data as usize))) {
        // SAFETY: the buffer came from `vesper_seq_grow` with this size and
        // alignment and is now unrecorded, so it is freed once.
        unsafe { dealloc(data as *mut u8, Layout::from_size_align_unchecked(bytes, 8)) };
    } else {
        STRAY.with(|s| s.set(s.get() + 1));
    }
    0
}

pub fn symbols() -> Vec<(&'static str, *const u8)> {
    vec![
        ("vesper_seq_new", vesper_seq_new as *const u8),
        ("vesper_seq_grow", vesper_seq_grow as *const u8),
        ("vesper_seq_fault", vesper_seq_fault as *const u8),
        ("vesper_seq_free", vesper_seq_free as *const u8),
        ("vesper_str_release", vesper_str_release as *const u8),
    ]
}

/// The live sequence headers and grown string buffers of this thread's JIT
/// (a header and the buffer it owns count once each, as in executables).
pub fn live_allocations() -> usize {
    let headers = HEADERS.with(|h| {
        h.borrow()
            .keys()
            .map(|&header| {
                let header = header as *const u64;
                // SAFETY: every recorded header is live.
                let (cap, data) = unsafe { (header.add(CAP).read(), header.add(DATA).read()) };
                1 + usize::from(cap != 0 && data != 0)
            })
            .sum::<usize>()
    });
    headers + BUFFERS.with(|b| b.borrow().len())
}

/// How many sequence allocations (as [`live_allocations`] counts them) the
/// last [`finish`] on this thread freed because generated code had not: 0
/// when every sequence and string was dropped by the program itself.
pub fn teardown_frees() -> usize {
    TEARDOWN.with(std::cell::Cell::get)
}

/// Drops on this thread of a sequence or string buffer that was not live,
/// which would be a double free in an executable. The count only grows:
/// compare it before and after a run.
pub fn stray_frees() -> usize {
    STRAY.with(std::cell::Cell::get)
}

/// Frees every sequence this thread's JIT allocated that generated code did
/// not drop, and returns the first recorded fault.
pub fn finish() -> Result<(), String> {
    TEARDOWN.with(|t| t.set(live_allocations()));
    HEADERS.with(|h| {
        for (header, stride) in h.borrow_mut().drain() {
            // SAFETY: each header came from `vesper_seq_new` and is freed once.
            unsafe { free_header(header as *mut u64, stride) };
        }
    });
    BUFFERS.with(|b| {
        for (data, bytes) in b.borrow_mut().drain() {
            // SAFETY: each buffer came from `vesper_seq_grow` with this size
            // and alignment and is freed once.
            unsafe { dealloc(data as *mut u8, Layout::from_size_align_unchecked(bytes, 8)) };
        }
    });
    FAULT.with(|f| f.borrow_mut().take()).map_or(Ok(()), Err)
}
