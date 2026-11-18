//! The port-backed build of [`Region`] (`cfg(vesper_port)`, port wave P1
//! lane 6, #903).
//!
//! Every decision is made by the Vesper `Book` in `vesper/src/lib.ves`:
//! placement, the growth policy, the reservation bound, serials, handle
//! validation, tail reclamation, watermarks, rewind, reset and the element
//! metadata of vectors. This file is the Rust shim around it. It owns what
//! Vesper cannot: chunk memory from the [`ChunkBacking`], the Rust payloads
//! (it writes and reads them at the addresses the book returns) and their
//! destructors, which the book names by address and queues as actions.
use super::{
    NEXT_REGION_ID, RefPairError, Region, RegionBox, RegionBytes, RegionConfig, RegionId,
    RegionRef, RegionStats, Watermark, drop_value,
};
use crate::backing::ChunkBacking;
use crate::error::{
    AllocError, ConfigError, HandleError, RegionError, RewindError, ValueAllocError,
};
use crate::port_binding::{Action, Book, Cfg, Extended, Fault, Placed, View};
use std::alloc::Layout;
use std::collections::VecDeque;
use std::marker::PhantomData;
use std::mem;
use std::ptr::{self, NonNull};
use std::sync::atomic::Ordering;

pub(super) const KIND_BYTES: u64 = 0;
pub(super) const KIND_VALUE: u64 = 1;
pub(super) const KIND_ELEMENTS: u64 = 2;
pub(super) const KIND_FROZEN: u64 = 3;

/// A fresh chunk handed to a repeated book call, as `(base, capacity)`.
pub(super) type Fresh = (u64, u64);

pub(super) fn word(n: usize) -> u64 {
    n as u64
}

pub(super) fn size(n: u64) -> usize {
    n as usize
}

/// The address the book computed, as a pointer with the provenance of the
/// chunk it lies in (every chunk base is exposed when it is obtained).
pub(super) fn ptr_at(addr: u64) -> *mut u8 {
    ptr::with_exposed_provenance_mut(addr as usize)
}

fn cfg_of(c: &RegionConfig) -> Cfg {
    Cfg {
        initial: word(c.initial_chunk_bytes),
        max_chunk: word(c.max_chunk_bytes),
        max_reserved: word(c.max_reserved_bytes),
        max_align: word(c.max_align),
        max_allocations: word(c.max_allocations),
        max_checkpoints: word(c.max_checkpoints),
        retained: word(c.retained_bytes),
    }
}

fn config_error(code: u64) -> ConfigError {
    match code {
        0 => ConfigError::ZeroInitialChunk,
        1 => ConfigError::InitialExceedsMaxChunk,
        2 => ConfigError::InitialExceedsReservation,
        3 => ConfigError::MaxAlignNotPowerOfTwo,
        4 => ConfigError::ChunkLayout,
        _ => ConfigError::AllocationLimitTooLarge,
    }
}

/// The crate error a book fault stands for.
pub(super) fn region_error(f: Fault) -> RegionError {
    match f {
        Fault::InvalidAlignment(align) => {
            AllocError::InvalidAlignment { align: size(align) }.into()
        }
        Fault::AlignmentTooLarge { align, max } => AllocError::AlignmentTooLarge {
            align: size(align),
            max: size(max),
        }
        .into(),
        Fault::SizeOverflow => AllocError::SizeOverflow.into(),
        Fault::CapacityExceeded {
            requested,
            reserved,
            limit,
        } => AllocError::CapacityExceeded {
            requested: size(requested),
            reserved: size(reserved),
            limit: size(limit),
        }
        .into(),
        Fault::TooManyAllocations(limit) => {
            AllocError::TooManyAllocations { limit: size(limit) }.into()
        }
        Fault::ExhaustedSerials => AllocError::ExhaustedSerials.into(),
        Fault::Stale => HandleError::Stale.into(),
        Fault::Moved => HandleError::Moved.into(),
        Fault::Linear(count) => RegionError::LinearObligationsLive { count: size(count) },
        Fault::TooManyCheckpoints(limit) => RegionError::TooManyCheckpoints { limit: size(limit) },
        Fault::Protected => RegionError::ProtectedByWatermark,
        Fault::Config(code) => RegionError::Config(config_error(code)),
    }
}

/// A fault the book reports only from allocating paths.
pub(super) fn alloc_error(f: Fault) -> AllocError {
    match region_error(f) {
        RegionError::Alloc(e) => e,
        // The book raises no other fault on these paths.
        _ => AllocError::SizeOverflow,
    }
}

/// A fault the book reports only from handle validation.
pub(super) fn handle_error(f: Fault) -> HandleError {
    match f {
        Fault::Moved => HandleError::Moved,
        _ => HandleError::Stale,
    }
}

impl<B: ChunkBacking> Region<B> {
    pub fn with_backing(config: RegionConfig, backing: B) -> Result<Self, ConfigError> {
        if let Some(Fault::Config(code)) = crate::port_binding::config_check(cfg_of(&config)) {
            return Err(config_error(code));
        }
        let id = NEXT_REGION_ID.fetch_add(1, Ordering::Relaxed);
        assert!(id != u64::MAX, "region identities exhausted");
        Ok(Self {
            id: RegionId(id),
            config,
            backing,
            book: Book::new(cfg_of(&config)),
            pending: VecDeque::new(),
            _local: PhantomData,
        })
    }

    /// [`Region::scoped`] with an explicit backing.
    pub fn scoped_in<R>(
        config: RegionConfig,
        backing: B,
        body: impl FnOnce(&mut Region<B>) -> R,
    ) -> Result<R, RegionError> {
        let mut region = Self::with_backing(config, backing)?;
        let out = body(&mut region);
        region.close()?;
        Ok(out)
    }

    pub fn id(&self) -> RegionId {
        self.id
    }

    pub fn config(&self) -> &RegionConfig {
        &self.config
    }

    pub fn backing(&self) -> &B {
        &self.backing
    }

    pub fn stats(&self) -> RegionStats {
        let s = self.book.stats();
        RegionStats {
            reserved_bytes: size(s.reserved),
            used_bytes: size(s.used),
            chunks: size(s.chunks),
            tracked_allocations: size(s.tracked),
            live_linear: size(s.live_linear),
            checkpoints: size(s.checkpoints),
            epoch: s.epoch,
        }
    }

    // ----- the shim -----------------------------------------------------------

    /// Obtains the chunk a book call asked for. `None` is the backing's
    /// refusal; the book has already checked the layout.
    pub(super) fn obtain(&mut self, cap: u64, align: u64) -> Option<Fresh> {
        let layout = Layout::from_size_align(size(cap), size(align)).ok()?;
        let base = self.backing.allocate(layout)?;
        Some((word(base.as_ptr().expose_provenance()), cap))
    }

    /// Takes the book's queued actions and does them, oldest first. Each is
    /// removed before it runs, so a panicking destructor cannot run twice;
    /// the rest stay queued for `Drop`.
    pub(super) fn settle(&mut self) {
        self.pending.extend(self.book.take_pending());
        while let Some(action) = self.pending.pop_front() {
            match action {
                Action::Finalize { fin, addr } => {
                    // SAFETY: the book queues the address of a drop function
                    // this shim registered (`drop_value::<T>`) with the
                    // address of the live, initialized `T` it was registered
                    // for, once, after removing its slot.
                    unsafe {
                        let f: unsafe fn(*mut u8) = mem::transmute(size(fin));
                        f(ptr_at(addr));
                    }
                }
                Action::DropElems { fin, addr, len } => {
                    // SAFETY: as above, for `drop_elems::<T>` and the `len`
                    // initialized elements the book recorded.
                    unsafe {
                        let f: unsafe fn(*mut u8, usize) = mem::transmute(size(fin));
                        f(ptr_at(addr), size(len));
                    }
                }
                Action::Release { base, cap, align } => {
                    let layout = Layout::from_size_align(size(cap), size(align))
                        .expect("the chunk was allocated with this layout");
                    let base = NonNull::new(ptr_at(base)).expect("chunk bases are nonnull");
                    // SAFETY: the book releases each chunk it adopted once,
                    // after every slot in it was removed, with the layout
                    // `obtain` allocated it with.
                    unsafe { self.backing.deallocate(base, layout) }
                }
            }
        }
    }

    /// Runs an allocating book call, obtaining the chunk it asks for and
    /// repeating it once with that chunk.
    fn placed(
        &mut self,
        call: impl Fn(&mut Book, Fresh) -> Placed,
    ) -> Result<(u32, u64, *mut u8), AllocError> {
        let mut fresh = (0, 0);
        loop {
            match call(&mut self.book, fresh) {
                Placed::Done {
                    slot,
                    serial,
                    addr,
                    pending,
                } => {
                    if pending > 0 {
                        self.settle();
                    }
                    return Ok((slot as u32, serial, ptr_at(addr)));
                }
                Placed::Need { cap, align } if fresh.1 == 0 => match self.obtain(cap, align) {
                    Some(chunk) => fresh = chunk,
                    None => {
                        return Err(AllocError::OutOfMemory {
                            chunk_bytes: size(cap),
                        });
                    }
                },
                Placed::Need { .. } => return Err(AllocError::SizeOverflow),
                Placed::Fail(f) => {
                    if fresh.1 != 0 {
                        self.settle();
                    }
                    return Err(alloc_error(f));
                }
            }
        }
    }

    pub(super) fn check_region(&self, region: RegionId) -> Result<(), HandleError> {
        if region != self.id {
            return Err(HandleError::CrossRegion {
                expected: self.id,
                found: region,
            });
        }
        Ok(())
    }

    pub(super) fn view(
        &self,
        region: RegionId,
        slot: u32,
        serial: u64,
        kind: u64,
    ) -> Result<View, HandleError> {
        self.check_region(region)?;
        self.book
            .view(u64::from(slot), serial, kind)
            .map_err(handle_error)
    }

    // ----- bytes ------------------------------------------------------------

    /// Allocates `len` zero-initialized bytes aligned to `align`.
    pub fn alloc_bytes(&mut self, len: usize, align: usize) -> Result<RegionBytes, AllocError> {
        let (slot, serial, ptr) = self
            .placed(|b, (fb, fc)| b.alloc(word(len), word(align), KIND_BYTES, 0, false, fb, fc))?;
        // SAFETY: the book reserved `len` bytes at `ptr` inside a chunk that
        // no other live allocation overlaps.
        unsafe { ptr.write_bytes(0, len) };
        Ok(self.bytes_handle(slot, serial))
    }

    /// Allocates a copy of `data` aligned to `align`.
    pub fn alloc_bytes_copy(
        &mut self,
        data: &[u8],
        align: usize,
    ) -> Result<RegionBytes, AllocError> {
        let (slot, serial, ptr) = self.placed(|b, (fb, fc)| {
            b.alloc(word(data.len()), word(align), KIND_BYTES, 0, false, fb, fc)
        })?;
        // SAFETY: fresh, exclusively reserved destination of `data.len()` bytes.
        unsafe { ptr::copy_nonoverlapping(data.as_ptr(), ptr, data.len()) };
        Ok(self.bytes_handle(slot, serial))
    }

    fn bytes_handle(&self, slot: u32, serial: u64) -> RegionBytes {
        RegionBytes {
            region: self.id,
            slot,
            serial,
        }
    }

    fn bytes_view(&self, h: &RegionBytes) -> Result<View, HandleError> {
        self.view(h.region, h.slot, h.serial, KIND_BYTES)
    }

    pub fn len(&self, h: &RegionBytes) -> Result<usize, HandleError> {
        Ok(size(self.bytes_view(h)?.len))
    }

    pub fn bytes(&self, h: &RegionBytes) -> Result<&[u8], HandleError> {
        let v = self.bytes_view(h)?;
        // SAFETY: a live byte slot, initialized at allocation and bounded by
        // its chunk; `&self` excludes mutable views.
        Ok(unsafe { std::slice::from_raw_parts(ptr_at(v.addr), size(v.len)) })
    }

    pub fn bytes_mut(&mut self, h: &RegionBytes) -> Result<&mut [u8], HandleError> {
        let v = self.bytes_view(h)?;
        // SAFETY: as in `bytes`; `&mut self` makes this the only view.
        Ok(unsafe { std::slice::from_raw_parts_mut(ptr_at(v.addr), size(v.len)) })
    }

    /// Two simultaneous mutable views of distinct allocations.
    pub fn bytes_mut_pair(
        &mut self,
        a: &RegionBytes,
        b: &RegionBytes,
    ) -> Result<(&mut [u8], &mut [u8]), HandleError> {
        let va = self.bytes_view(a)?;
        let vb = self.bytes_view(b)?;
        if a.slot == b.slot {
            return Err(HandleError::Aliased);
        }
        // SAFETY: two distinct live slots occupy disjoint byte ranges.
        Ok(unsafe {
            (
                std::slice::from_raw_parts_mut(ptr_at(va.addr), size(va.len)),
                std::slice::from_raw_parts_mut(ptr_at(vb.addr), size(vb.len)),
            )
        })
    }

    /// Appends `data`, in place at the unprotected tip or by relocation (see
    /// the Rust build for the contract).
    pub fn extend_bytes(&mut self, h: &mut RegionBytes, data: &[u8]) -> Result<(), RegionError> {
        self.check_region(h.region)?;
        let (slot, serial) = (u64::from(h.slot), h.serial);
        let mut fresh = (0, 0);
        loop {
            match self
                .book
                .extend(slot, serial, word(data.len()), fresh.0, fresh.1)
            {
                Extended::Unchanged => return Ok(()),
                Extended::InPlace { dst } => {
                    // SAFETY: the bytes after the tip up to capacity belong
                    // to no live allocation.
                    unsafe { ptr::copy_nonoverlapping(data.as_ptr(), ptr_at(dst), data.len()) };
                    return Ok(());
                }
                Extended::Relocated {
                    slot,
                    serial,
                    src,
                    dst,
                    old_len,
                    pending,
                } => {
                    // SAFETY: the destination is a fresh reservation disjoint
                    // from the old slot (released, not yet reused) and `data`.
                    unsafe {
                        ptr::copy_nonoverlapping(ptr_at(src), ptr_at(dst), size(old_len));
                        ptr::copy_nonoverlapping(
                            data.as_ptr(),
                            ptr_at(dst).add(size(old_len)),
                            data.len(),
                        );
                    }
                    if pending > 0 {
                        self.settle();
                    }
                    *h = self.bytes_handle(slot as u32, serial);
                    return Ok(());
                }
                Extended::Need { cap, align } if fresh.1 == 0 => match self.obtain(cap, align) {
                    Some(chunk) => fresh = chunk,
                    None => {
                        return Err(AllocError::OutOfMemory {
                            chunk_bytes: size(cap),
                        }
                        .into());
                    }
                },
                Extended::Need { .. } => return Err(AllocError::SizeOverflow.into()),
                Extended::Fail(f) => {
                    if fresh.1 != 0 {
                        self.settle();
                    }
                    return Err(region_error(f));
                }
            }
        }
    }

    /// Releases a byte allocation. Every copy of the handle becomes stale.
    pub fn release_bytes(&mut self, h: RegionBytes) -> Result<(), HandleError> {
        self.check_region(h.region)?;
        self.book
            .release(u64::from(h.slot), h.serial)
            .map_err(handle_error)?;
        Ok(())
    }

    /// Stable address of a live byte allocation for lowered native code.
    pub fn bytes_ptr(&self, h: &RegionBytes) -> Result<NonNull<u8>, HandleError> {
        let v = self.bytes_view(h)?;
        Ok(NonNull::new(ptr_at(v.addr)).expect("chunk addresses are nonnull"))
    }

    /// Untracked raw bump allocation (the native fast path's slow-path entry).
    pub fn alloc_raw(&mut self, size: usize, align: usize) -> Result<NonNull<u8>, AllocError> {
        let (_, _, ptr) =
            self.placed(|b, (fb, fc)| b.alloc_raw(word(size), word(align), fb, fc))?;
        Ok(NonNull::new(ptr).expect("chunk addresses are nonnull"))
    }

    // ----- typed values -----------------------------------------------------

    fn alloc_typed<T: 'static>(
        &mut self,
        value: T,
        linear: bool,
    ) -> Result<RegionBox<T>, ValueAllocError<T>> {
        let fin = if mem::needs_drop::<T>() {
            word(drop_value::<T> as unsafe fn(*mut u8) as usize)
        } else {
            0
        };
        let placed = self.placed(|b, (fb, fc)| {
            b.alloc(
                word(mem::size_of::<T>()),
                word(mem::align_of::<T>()),
                KIND_VALUE,
                fin,
                linear,
                fb,
                fc,
            )
        });
        let (slot, serial, ptr) = match placed {
            Ok(p) => p,
            Err(error) => return Err(ValueAllocError { value, error }),
        };
        // SAFETY: the book reserved `size_of::<T>()` bytes aligned for `T`.
        unsafe { ptr.cast::<T>().write(value) };
        Ok(RegionBox {
            region: self.id,
            slot,
            serial,
            _payload: PhantomData,
        })
    }

    /// Moves an affine or copy value into the region.
    pub fn alloc_value<T: 'static>(
        &mut self,
        value: T,
    ) -> Result<RegionBox<T>, ValueAllocError<T>> {
        self.alloc_typed(value, false)
    }

    /// Moves a linear value into the region.
    pub fn alloc_linear<T: 'static>(
        &mut self,
        value: T,
    ) -> Result<RegionBox<T>, ValueAllocError<T>> {
        self.alloc_typed(value, true)
    }

    fn value_ptr<T>(
        &self,
        region: RegionId,
        slot: u32,
        serial: u64,
    ) -> Result<*mut T, HandleError> {
        let v = self.view(region, slot, serial, KIND_VALUE)?;
        Ok(ptr_at(v.addr).cast::<T>())
    }

    pub fn get<T>(&self, b: &RegionBox<T>) -> Result<&T, HandleError> {
        let ptr = self.value_ptr::<T>(b.region, b.slot, b.serial)?;
        // SAFETY: a live value slot minted for this handle's `T` (serials are
        // unique) holds an initialized, aligned `T`.
        Ok(unsafe { &*ptr })
    }

    pub fn get_mut<T>(&mut self, b: &RegionBox<T>) -> Result<&mut T, HandleError> {
        let ptr = self.value_ptr::<T>(b.region, b.slot, b.serial)?;
        // SAFETY: as in `get`; `&mut self` makes this the only view.
        Ok(unsafe { &mut *ptr })
    }

    /// Mints a copyable weak reference to a live box payload.
    pub fn box_ref<T>(&self, b: &RegionBox<T>) -> Result<RegionRef<T>, HandleError> {
        self.view(b.region, b.slot, b.serial, KIND_VALUE)?;
        Ok(RegionRef {
            region: b.region,
            slot: b.slot,
            serial: b.serial,
            _payload: PhantomData,
        })
    }

    pub fn ref_get<T>(&self, r: RegionRef<T>) -> Result<&T, HandleError> {
        let ptr = self.value_ptr::<T>(r.region, r.slot, r.serial)?;
        // SAFETY: a reference is only minted from a live `RegionBox<T>` with
        // the same unique serial.
        Ok(unsafe { &*ptr })
    }

    pub fn ref_get_mut<T>(&mut self, r: RegionRef<T>) -> Result<&mut T, HandleError> {
        let ptr = self.value_ptr::<T>(r.region, r.slot, r.serial)?;
        // SAFETY: as in `ref_get`; `&mut self` makes this the only view.
        Ok(unsafe { &mut *ptr })
    }

    /// Two mutable payloads at once (for example both ends of a graph edge).
    pub fn ref_get_disjoint_mut<T>(
        &mut self,
        a: RegionRef<T>,
        b: RegionRef<T>,
    ) -> Result<(&mut T, &mut T), RefPairError> {
        let pa = self
            .value_ptr::<T>(a.region, a.slot, a.serial)
            .map_err(RefPairError::Handle)?;
        let pb = self
            .value_ptr::<T>(b.region, b.slot, b.serial)
            .map_err(RefPairError::Handle)?;
        if a.slot == b.slot {
            return Err(RefPairError::Aliased);
        }
        // SAFETY: two distinct live value slots never overlap, and `&mut self`
        // excludes every other view of region memory.
        Ok(unsafe { (&mut *pa, &mut *pb) })
    }

    /// Moves the value out, unregistering its finalizer and discharging a
    /// linear obligation.
    pub fn take<T>(&mut self, b: RegionBox<T>) -> Result<T, HandleError> {
        self.check_region(b.region)?;
        let addr = self
            .book
            .take(u64::from(b.slot), b.serial)
            .map_err(handle_error)?;
        // SAFETY: the slot was live and is now marked moved, so the value is
        // read exactly once and never finalized; tail reclamation only moved
        // the bump cursor, so the bytes are intact.
        Ok(unsafe { ptr_at(addr).cast::<T>().read() })
    }

    // ----- checkpoints, rewind, reset --------------------------------------

    /// Records the current allocation point.
    pub fn checkpoint(&mut self) -> Result<Watermark, RegionError> {
        let serial = self.book.checkpoint().map_err(region_error)?;
        Ok(Watermark {
            region: self.id,
            serial,
        })
    }

    /// Finalizes and discards every allocation made after `mark`.
    pub fn rewind(&mut self, mark: Watermark) -> Result<(), RewindError> {
        if let Err(e) = self.check_region(mark.region) {
            return Err(RewindError::Handle(e));
        }
        match self.book.rewind(mark.serial) {
            Ok(_) => {
                self.settle();
                Ok(())
            }
            Err(Fault::Linear(count)) => Err(RewindError::LinearObligationsLive {
                mark,
                count: size(count),
            }),
            Err(f) => Err(RewindError::Handle(handle_error(f))),
        }
    }

    /// Discards a watermark without rewinding.
    pub fn commit(&mut self, mark: Watermark) -> Result<(), HandleError> {
        self.check_region(mark.region)?;
        self.book.commit(mark.serial).map_err(handle_error)?;
        Ok(())
    }

    /// Finalizes every allocation, invalidates every handle and watermark, and
    /// returns chunk capacity beyond `retained_bytes` to the backing.
    pub fn reset(&mut self) -> Result<(), RegionError> {
        self.book.reset().map_err(region_error)?;
        self.settle();
        Ok(())
    }

    /// Finalizes and frees the region.
    pub fn close(mut self) -> Result<RegionStats, RegionError> {
        let stats = self.stats();
        let abandoned = self.book.finish();
        self.settle();
        drop(self);
        if abandoned > 0 {
            return Err(RegionError::LinearObligationsLive {
                count: size(abandoned),
            });
        }
        Ok(stats)
    }
}

impl<B: ChunkBacking> Drop for Region<B> {
    fn drop(&mut self) {
        /// Finishes the queued work on normal exit and while unwinding from
        /// a panicking destructor (a second panic aborts, as for `Vec`).
        struct Finish<'a, B: ChunkBacking>(&'a mut Region<B>);
        impl<B: ChunkBacking> Drop for Finish<'_, B> {
            fn drop(&mut self) {
                self.0.settle();
            }
        }
        let finish = Finish(self);
        finish.0.book.finish();
        finish.0.settle();
    }
}
