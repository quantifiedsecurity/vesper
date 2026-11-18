//! The port-backed build's glue (port wave P1 lane 8, #905): the generated
//! binding of the Vesper package in `vesper/`, the `Libm` shim it imports,
//! and the conversions the forwarding functions share.
//!
//! The export ABI has no `f32`, so an `f32` crosses as its bit pattern: a
//! `u32` scalar, and `&[u32]` for `&[f32]`, lent in place
//! ([`bits`], [`bits_mut`]). Every forwarding function maps the port's
//! `Fault` to the [`Error`] the Rust build returns at that call.
use crate::Error;

#[rustfmt::skip]
#[path = "../port/binding.rs"]
#[allow(unsafe_code, clippy::all)]
pub(crate) mod binding;

pub(crate) use binding::Fault;

/// The `.vsys` library `Libm` that `vesper/src/lib.ves` imports: the
/// platform `f32::exp`, so the port's softmax and SwiGLU equal the Rust
/// build's bit for bit (the Rust build documents no cross-ISA claim for
/// `exp`).
pub(crate) mod shim {
    #[allow(unsafe_code)]
    #[unsafe(no_mangle)]
    pub extern "C" fn vesper_tensor_expf(bits: u32) -> u32 {
        f32::from_bits(bits).exp().to_bits()
    }
}

/// The `.vsys` library `WeightFile` that `vesper/src/lib.ves` imports: the
/// file `Mapping::open` opened, stat'ed and mapped for the port's
/// `open_weights`, which decides what to do with it (src/mmap.rs). The file
/// and its mapping live in a per-thread slot for the one synchronous call.
pub(crate) mod files {
    use std::cell::RefCell;
    use std::fs::File;

    thread_local! {
        static SLOT: RefCell<(Option<File>, Option<memmap2::Mmap>)> =
            const { RefCell::new((None, None)) };
    }

    /// Runs `open_weights` on `file`; returns its code and the mapping.
    pub(crate) fn with_file(file: File) -> (u64, Option<memmap2::Mmap>) {
        SLOT.with(|s| *s.borrow_mut() = (Some(file), None));
        let code = super::binding::open_weights();
        let map = SLOT.with(|s| {
            let mut s = s.borrow_mut();
            s.0 = None;
            s.1.take()
        });
        (code, map)
    }

    /// The file's length, or `u64::MAX` when it cannot be stat'ed.
    #[allow(unsafe_code)]
    #[unsafe(no_mangle)]
    pub extern "C" fn vesper_tensor_file_len() -> u64 {
        SLOT.with(|s| {
            s.borrow()
                .0
                .as_ref()
                .and_then(|f| f.metadata().ok())
                .map_or(u64::MAX, |m| m.len())
        })
    }

    /// 0 when the file is mapped, 1 when it cannot be.
    #[allow(unsafe_code)]
    #[unsafe(no_mangle)]
    pub extern "C" fn vesper_tensor_file_map() -> u64 {
        SLOT.with(|s| {
            let mut s = s.borrow_mut();
            let Some(file) = s.0.as_ref() else {
                return 1;
            };
            // SAFETY: a read-only private mapping of the file `Mapping::open`
            // just opened; the contract (no concurrent truncation or
            // modification) is the caller's, as in the Rust build, and all
            // access goes through bounds-checked slices.
            match unsafe { memmap2::Mmap::map(file) } {
                Ok(map) => {
                    s.1 = Some(map);
                    0
                }
                Err(_) => 1,
            }
        })
    }
}

pub(crate) fn bits(x: &[f32]) -> &[u32] {
    bytemuck::cast_slice(x)
}

pub(crate) fn bits_mut(x: &mut [f32]) -> &mut [u32] {
    bytemuck::cast_slice_mut(x)
}

/// A failure of a port call: checked arithmetic, an index, or a `Fault`.
#[derive(Debug)]
pub(crate) enum Failure {
    Overflow,
    Index,
    Fault(Fault),
}

pub(crate) trait IntoFailure: std::fmt::Debug {
    fn failure(self) -> Failure;
}
impl IntoFailure for vesper_port_abi::ArithOverflow {
    fn failure(self) -> Failure {
        Failure::Overflow
    }
}
impl IntoFailure for vesper_port_abi::RuntimeError {
    fn failure(self) -> Failure {
        Failure::Index
    }
}
impl IntoFailure for Fault {
    fn failure(self) -> Failure {
        Failure::Fault(self)
    }
}
macro_rules! failures {
    ($($t:ident [$($v:ident),*]),* $(,)?) => {$(
        impl IntoFailure for binding::$t {
            fn failure(self) -> Failure {
                match self {
                    $(binding::$t::$v(e) => e.failure(),)*
                }
            }
        }
    )*};
}
failures! {
    AddAssignError [RuntimeError, Fault],
    AttendError [ArithOverflow, RuntimeError, Fault],
    ArgmaxError [RuntimeError, Fault],
    CheckQ4KBlockError [ArithOverflow, RuntimeError],
    DecodePieceError [ArithOverflow, RuntimeError],
    DequantizeMxfp4BlockError [ArithOverflow, RuntimeError],
    DequantizeQ40BlockError [ArithOverflow, RuntimeError],
    DequantizeQ4KBlockError [ArithOverflow, RuntimeError],
    DequantizeQ80BlockError [ArithOverflow, RuntimeError],
    EncodePromptError [ArithOverflow, RuntimeError, Fault],
    FinitePayloadError [ArithOverflow, RuntimeError],
    Fp8CodesError [ArithOverflow, RuntimeError],
    Fp8ScaleError [RuntimeError, Fault],
    KvAppendError [ArithOverflow, RuntimeError],
    KvBeginError [ArithOverflow, RuntimeError],
    KvCheckpointError [ArithOverflow, RuntimeError],
    KvLayoutError [ArithOverflow, RuntimeError],
    KvLenError [ArithOverflow, RuntimeError],
    KvReadError [ArithOverflow, RuntimeError],
    KvReadPageError [ArithOverflow, RuntimeError],
    KvReleaseError [ArithOverflow, RuntimeError],
    KvRollbackError [ArithOverflow, RuntimeError],
    KvStatsError [ArithOverflow, RuntimeError],
    LlamaCheckpointError [ArithOverflow, RuntimeError],
    LlamaDecodeError [ArithOverflow, RuntimeError],
    LlamaTokenizerError [ArithOverflow, RuntimeError],
    LlamaValidateError [ArithOverflow, RuntimeError],
    MakeQkx2QuantsError [ArithOverflow, RuntimeError],
    ParseGgufError [ArithOverflow, RuntimeError, Fault],
    ParseJsonError [ArithOverflow, RuntimeError],
    Q4KScaleMinError [ArithOverflow, RuntimeError],
    QuantizeMxfp4Error [ArithOverflow, RuntimeError, Fault],
    QuantizeQ40Error [ArithOverflow, RuntimeError, Fault],
    QuantizeQ4KError [ArithOverflow, RuntimeError, Fault],
    QuantizeQ4KBlockError [ArithOverflow, RuntimeError],
    QuantizeQ80Error [ArithOverflow, RuntimeError, Fault],
    RmsNormError [RuntimeError, Fault],
    SampleError [ArithOverflow, RuntimeError, Fault],
    SampleMultError [ArithOverflow, RuntimeError],
    SampleTopPError [ArithOverflow, RuntimeError],
    ShapeNormalizeError [ArithOverflow, RuntimeError],
    SoftmaxError [RuntimeError, Fault],
    SwigluError [RuntimeError, Fault],
    VocabLookupError [ArithOverflow, RuntimeError],
    VocabTableError [ArithOverflow, RuntimeError],
}

/// The value of a port call that cannot fail on its inputs: every index is
/// in bounds and no arithmetic can overflow. A failure here is a port bug,
/// and panics like the Rust build's out-of-range slice index would.
pub(crate) fn must<T, E: IntoFailure>(r: Result<T, E>) -> T {
    match r {
        Ok(v) => v,
        Err(e) => panic!("vesper-tensor port: unexpected failure {e:?}"),
    }
}

/// A port result as the crate's [`Error`]: `fault` gives the Rust build's
/// error for each `Fault`.
pub(crate) fn check<T, E: IntoFailure>(
    r: Result<T, E>,
    fault: impl FnOnce(Fault) -> Error,
) -> Result<T, Error> {
    r.map_err(|e| match e.failure() {
        Failure::Fault(f) => fault(f),
        Failure::Overflow => Error::Overflow,
        Failure::Index => Error::Malformed("vesper-tensor port index out of range"),
    })
}
