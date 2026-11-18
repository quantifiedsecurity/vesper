//! Region vectors in the port-backed build (`cfg(vesper_port)`): the Vesper
//! book keeps each buffer's length, capacity and linear flag and decides
//! growth (amortized, exact, in place at the tip or by relocation); this
//! shim moves the Rust elements at the addresses it returns.
use super::super::Region;
use super::super::port::{
    Fresh, KIND_ELEMENTS, KIND_FROZEN, handle_error, ptr_at, region_error, size, word,
};
use super::{RegionSlice, RegionVec, drop_elems, drop_range};
use crate::backing::ChunkBacking;
use crate::error::{AllocError, HandleError, InsertError, OwnerError, RegionError};
use crate::port_binding::{Fault, Grown, Placed, View};
use std::marker::PhantomData;
use std::mem;
use std::ptr;

/// A buffer ready for `claim` new elements at `base + len`.
struct Room<T> {
    slot: u32,
    serial: u64,
    base: *mut T,
    len: usize,
}

impl<B: ChunkBacking> Region<B> {
    fn alloc_elements<T: 'static>(
        &mut self,
        cap: usize,
        linear: bool,
    ) -> Result<RegionVec<T>, AllocError> {
        let edrop = if mem::needs_drop::<T>() {
            word(drop_elems::<T> as unsafe fn(*mut u8, usize) as usize)
        } else {
            0
        };
        let mut fresh: Fresh = (0, 0);
        loop {
            match self.book.alloc_elems(
                word(cap),
                word(mem::size_of::<T>()),
                word(mem::align_of::<T>()),
                edrop,
                linear,
                fresh.0,
                fresh.1,
            ) {
                Placed::Done {
                    slot,
                    serial,
                    pending,
                    ..
                } => {
                    if pending > 0 || fresh.1 != 0 {
                        self.settle();
                    }
                    return Ok(RegionVec {
                        region: self.id,
                        slot: slot as u32,
                        serial,
                        _payload: PhantomData,
                    });
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
                    return Err(match region_error(f) {
                        RegionError::Alloc(e) => e,
                        _ => AllocError::SizeOverflow,
                    });
                }
            }
        }
    }

    fn vec_view<T>(&self, v: &RegionVec<T>) -> Result<View, HandleError> {
        self.view(v.region, v.slot, v.serial, KIND_ELEMENTS)
    }

    fn slice_view<T>(&self, s: &RegionSlice<T>) -> Result<View, HandleError> {
        self.view(s.region, s.slot, s.serial, KIND_FROZEN)
    }

    /// Grows the buffer for `additional` more elements and claims `claim` of
    /// them; the caller must initialize those at once. On error nothing
    /// changed.
    fn grow<T>(
        &mut self,
        v: &RegionVec<T>,
        additional: usize,
        exact: bool,
        claim: usize,
    ) -> Result<Room<T>, RegionError> {
        self.check_region(v.region)?;
        let mut skip = false;
        let mut fresh: Fresh = (0, 0);
        loop {
            let g = self.book.grow(
                u64::from(v.slot),
                v.serial,
                word(additional),
                exact,
                word(claim),
                skip,
                fresh.0,
                fresh.1,
            );
            match g {
                Grown::Ready {
                    slot,
                    serial,
                    base,
                    len,
                } => {
                    if fresh.1 != 0 {
                        self.settle();
                    }
                    return Ok(Room {
                        slot: slot as u32,
                        serial,
                        base: ptr_at(base).cast::<T>(),
                        len: size(len),
                    });
                }
                Grown::Relocated {
                    slot,
                    serial,
                    src,
                    base,
                    len,
                    bytes,
                    pending,
                } => {
                    // SAFETY: the book moved the buffer's metadata to a fresh
                    // reservation disjoint from every live allocation; the
                    // old bytes stay intact until the next allocation.
                    unsafe { ptr::copy_nonoverlapping(ptr_at(src), ptr_at(base), size(bytes)) };
                    if pending > 0 || fresh.1 != 0 {
                        self.settle();
                    }
                    return Ok(Room {
                        slot: slot as u32,
                        serial,
                        base: ptr_at(base).cast::<T>(),
                        len: size(len),
                    });
                }
                Grown::Need {
                    cap,
                    align,
                    retry,
                    fallback,
                } if fresh.1 == 0 => {
                    if fallback {
                        skip = true;
                    }
                    match self.obtain(cap, align) {
                        Some(chunk) => fresh = chunk,
                        None if retry => skip = true,
                        None => {
                            return Err(AllocError::OutOfMemory {
                                chunk_bytes: size(cap),
                            }
                            .into());
                        }
                    }
                }
                Grown::Need { .. } => return Err(AllocError::SizeOverflow.into()),
                Grown::Fail(f) => {
                    if fresh.1 != 0 {
                        self.settle();
                    }
                    return Err(match f {
                        Fault::Stale | Fault::Moved => handle_error(f).into(),
                        _ => region_error(f),
                    });
                }
            }
        }
    }

    fn rebind<T>(v: &mut RegionVec<T>, room: &Room<T>) {
        v.slot = room.slot;
        v.serial = room.serial;
    }

    /// Releases a live element buffer and drops its elements highest index
    /// first, after the book released the slot.
    fn release_elements<T>(&mut self, slot: u32, serial: u64, kind: u64) -> Result<(), Fault> {
        let span = self.book.release_elems(u64::from(slot), serial, kind)?;
        // SAFETY: the `to` initialized elements are no longer recorded; tail
        // reclamation only moved the bump cursor.
        unsafe { drop_range(ptr_at(span.base).cast::<T>(), 0, size(span.to)) };
        if span.pending > 0 {
            self.settle();
        }
        Ok(())
    }

    // ----- construction -----------------------------------------------------

    /// Creates an empty vector for affine or copy elements.
    pub fn vec_new<T: 'static>(&mut self) -> Result<RegionVec<T>, AllocError> {
        self.vec_with_capacity(0)
    }

    /// Creates an empty vector with room for `cap` elements.
    pub fn vec_with_capacity<T: 'static>(
        &mut self,
        cap: usize,
    ) -> Result<RegionVec<T>, AllocError> {
        self.alloc_elements::<T>(cap, false)
    }

    /// Creates a vector of linear elements.
    pub fn vec_with_capacity_linear<T: 'static>(
        &mut self,
        cap: usize,
    ) -> Result<RegionVec<T>, AllocError> {
        self.alloc_elements::<T>(cap, true)
    }

    // ----- inspection ---------------------------------------------------------

    pub fn vec_len<T>(&self, v: &RegionVec<T>) -> Result<usize, HandleError> {
        Ok(size(self.vec_view(v)?.elen))
    }

    pub fn vec_is_empty<T>(&self, v: &RegionVec<T>) -> Result<bool, HandleError> {
        Ok(self.vec_len(v)? == 0)
    }

    pub fn vec_capacity<T>(&self, v: &RegionVec<T>) -> Result<usize, HandleError> {
        Ok(size(self.vec_view(v)?.ecap))
    }

    /// Shared view of the initialized elements.
    pub fn vec_as_slice<T>(&self, v: &RegionVec<T>) -> Result<&[T], HandleError> {
        let view = self.vec_view(v)?;
        // SAFETY: a live element slot minted for this handle's `T` holds
        // `elen` initialized, aligned elements; `&self` excludes mutable views.
        Ok(unsafe { std::slice::from_raw_parts(ptr_at(view.addr).cast::<T>(), size(view.elen)) })
    }

    /// Exclusive view of the initialized elements.
    pub fn vec_as_mut_slice<T>(&mut self, v: &RegionVec<T>) -> Result<&mut [T], HandleError> {
        let view = self.vec_view(v)?;
        // SAFETY: as in `vec_as_slice`; `&mut self` makes the view exclusive.
        Ok(unsafe {
            std::slice::from_raw_parts_mut(ptr_at(view.addr).cast::<T>(), size(view.elen))
        })
    }

    pub fn vec_get<T>(&self, v: &RegionVec<T>, i: usize) -> Result<Option<&T>, HandleError> {
        Ok(self.vec_as_slice(v)?.get(i))
    }

    pub fn vec_get_mut<T>(
        &mut self,
        v: &RegionVec<T>,
        i: usize,
    ) -> Result<Option<&mut T>, HandleError> {
        Ok(self.vec_as_mut_slice(v)?.get_mut(i))
    }

    // ----- growth and insertion ---------------------------------------------

    /// Ensures capacity for `additional` more elements with amortized
    /// (doubling) growth, falling back to the exact requirement.
    pub fn vec_reserve<T>(
        &mut self,
        v: &mut RegionVec<T>,
        additional: usize,
    ) -> Result<(), RegionError> {
        let room = self.grow(v, additional, false, 0)?;
        Self::rebind(v, &room);
        Ok(())
    }

    /// As [`Region::vec_reserve`], without amortization.
    pub fn vec_reserve_exact<T>(
        &mut self,
        v: &mut RegionVec<T>,
        additional: usize,
    ) -> Result<(), RegionError> {
        let room = self.grow(v, additional, true, 0)?;
        Self::rebind(v, &room);
        Ok(())
    }

    /// Appends `value`. On failure the vector is unchanged and the value is
    /// returned in the error.
    pub fn vec_push<T>(&mut self, v: &mut RegionVec<T>, value: T) -> Result<(), InsertError<T>> {
        let room = match self.grow(v, 1, false, 1) {
            Ok(room) => room,
            Err(error) => return Err(InsertError { value, error }),
        };
        Self::rebind(v, &room);
        // SAFETY: the book made room for and claimed element `len`, which is
        // inside the buffer and uninitialized; nothing reads it before this.
        unsafe { room.base.add(room.len).write(value) };
        Ok(())
    }

    /// Appends a copy of `data`, all or nothing.
    pub fn vec_extend_from_slice<T: Copy>(
        &mut self,
        v: &mut RegionVec<T>,
        data: &[T],
    ) -> Result<(), RegionError> {
        let room = self.grow(v, data.len(), false, data.len())?;
        Self::rebind(v, &room);
        // SAFETY: room for `data.len()` claimed elements; `data` is a shared
        // borrow and cannot alias region memory under `&mut self`.
        unsafe { ptr::copy_nonoverlapping(data.as_ptr(), room.base.add(room.len), data.len()) };
        Ok(())
    }

    // ----- removal and extraction ---------------------------------------------

    /// Moves the last element out. Capacity is kept.
    pub fn vec_pop<T>(&mut self, v: &mut RegionVec<T>) -> Result<Option<T>, HandleError> {
        self.check_region(v.region)?;
        let addr = self
            .book
            .pop(u64::from(v.slot), v.serial)
            .map_err(handle_error)?;
        if addr == u64::MAX {
            return Ok(None);
        }
        // SAFETY: the element at `addr` was initialized and is no longer
        // recorded, so it is read exactly once.
        Ok(Some(unsafe { ptr_at(addr).cast::<T>().read() }))
    }

    /// Drops the elements at `new_len..`, highest index first.
    pub fn vec_truncate<T>(&mut self, v: &RegionVec<T>, new_len: usize) -> Result<(), RegionError> {
        self.check_region(v.region)?;
        let span = self
            .book
            .truncate(u64::from(v.slot), v.serial, word(new_len))
            .map_err(|f| match f {
                Fault::Stale | Fault::Moved => handle_error(f).into(),
                _ => region_error(f),
            })?;
        // SAFETY: `from..to` were initialized and are no longer recorded.
        unsafe {
            drop_range(
                ptr_at(span.base).cast::<T>(),
                size(span.from),
                size(span.to),
            )
        };
        Ok(())
    }

    pub fn vec_clear<T>(&mut self, v: &RegionVec<T>) -> Result<(), RegionError> {
        self.vec_truncate(v, 0)
    }

    /// Moves every element out to `sink`, front to back.
    pub fn vec_drain<T>(
        &mut self,
        v: &mut RegionVec<T>,
        mut sink: impl FnMut(T),
    ) -> Result<(), HandleError> {
        struct Rest<T> {
            base: *mut T,
            next: usize,
            end: usize,
        }
        impl<T> Drop for Rest<T> {
            fn drop(&mut self) {
                // SAFETY: `next..end` are initialized and unrecorded; each
                // earlier element was moved out exactly once.
                unsafe { drop_range(self.base, self.next, self.end) }
            }
        }
        self.check_region(v.region)?;
        let span = self
            .book
            .drain(u64::from(v.slot), v.serial)
            .map_err(handle_error)?;
        let base = ptr_at(span.base).cast::<T>();
        let mut rest = Rest {
            base,
            next: 0,
            end: size(span.to),
        };
        while rest.next < rest.end {
            // SAFETY: element `next` is initialized and read exactly once.
            let value = unsafe { base.add(rest.next).read() };
            rest.next += 1;
            sink(value);
        }
        Ok(())
    }

    /// Drops the elements and releases the buffer.
    pub fn vec_release<T>(&mut self, v: RegionVec<T>) -> Result<(), OwnerError<RegionVec<T>>> {
        if let Err(e) = self.check_region(v.region) {
            return Err(OwnerError {
                owner: v,
                error: e.into(),
            });
        }
        match self.release_elements::<T>(v.slot, v.serial, KIND_ELEMENTS) {
            Ok(()) => Ok(()),
            Err(f) => Err(OwnerError {
                owner: v,
                error: match f {
                    Fault::Stale | Fault::Moved => handle_error(f).into(),
                    _ => region_error(f),
                },
            }),
        }
    }

    /// Freezes the vector into a copyable read-only slice handle.
    pub fn vec_freeze<T>(
        &mut self,
        v: RegionVec<T>,
    ) -> Result<RegionSlice<T>, OwnerError<RegionVec<T>>> {
        if let Err(e) = self.check_region(v.region) {
            return Err(OwnerError {
                owner: v,
                error: e.into(),
            });
        }
        match self.book.freeze(u64::from(v.slot), v.serial) {
            Ok(_) => Ok(RegionSlice {
                region: v.region,
                slot: v.slot,
                serial: v.serial,
                _payload: PhantomData,
            }),
            Err(f) => Err(OwnerError {
                owner: v,
                error: match f {
                    Fault::Stale | Fault::Moved => handle_error(f).into(),
                    _ => region_error(f),
                },
            }),
        }
    }

    // ----- frozen slices --------------------------------------------------------

    pub fn slice_len<T>(&self, s: &RegionSlice<T>) -> Result<usize, HandleError> {
        Ok(size(self.slice_view(s)?.elen))
    }

    /// Shared view of a frozen buffer.
    pub fn slice<T>(&self, s: &RegionSlice<T>) -> Result<&[T], HandleError> {
        let view = self.slice_view(s)?;
        // SAFETY: a live frozen slot holds `elen` initialized elements of the
        // handle's `T`; no API yields a mutable view of it.
        Ok(unsafe { std::slice::from_raw_parts(ptr_at(view.addr).cast::<T>(), size(view.elen)) })
    }

    /// Drops the elements and releases the buffer; every copy becomes stale.
    pub fn slice_release<T>(&mut self, s: RegionSlice<T>) -> Result<(), HandleError> {
        self.check_region(s.region)?;
        self.release_elements::<T>(s.slot, s.serial, KIND_FROZEN)
            .map_err(handle_error)
    }
}
