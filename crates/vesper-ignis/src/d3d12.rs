//! Typed models of the D3D12 enhanced-barrier vocabulary.
//!
//! The types are shared by both builds; the compatibility tables are in
//! `reference/d3d12.rs` (Rust build) and `vesper/src/lib.ves` (Vesper build).
//!
//! The bit values mirror `D3D12_BARRIER_SYNC`, `D3D12_BARRIER_ACCESS`, and
//! `D3D12_BARRIER_LAYOUT` from `d3d12.h`, so a later Windows executor can pass
//! [`Sync::bits`], [`Access::bits`], and [`Layout::raw`] straight through. They
//! have not yet been compared against the headers on a Windows host; that is
//! part of the Windows acceptance gate.
//!
//! The compatibility tables follow the "Layout Access Compatibility",
//! "Access Bits Barrier Sync Compatibility", and queue-type sections of the
//! D3D12 Enhanced Barriers specification
//! (<https://microsoft.github.io/DirectX-Specs/d3d/D3D12EnhancedBarriers.html>).
//! Video queues, video layouts/accesses, simultaneous-access textures, and
//! `DIRECT_QUEUE_GENERIC_READ_COMPUTE_QUEUE_ACCESSIBLE` are not modeled.

use std::fmt;
use std::ops::{BitOr, BitOrAssign};

macro_rules! bit_set {
    (
        $(#[$meta:meta])*
        $name:ident { $($(#[$cmeta:meta])* $konst:ident = $value:expr),* $(,)? }
    ) => {
        $(#[$meta])*
        #[derive(Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Default)]
        pub struct $name(pub(crate) u32);

        impl $name {
            $($(#[$cmeta])* pub const $konst: Self = Self($value);)*

            const NAMED: &'static [(&'static str, u32)] = &[$((stringify!($konst), $value)),*];

            /// Raw D3D12 bit pattern.
            pub const fn bits(self) -> u32 {
                self.0
            }

            /// Builds a set from raw bits. Unknown bits are kept, so the caller
            /// must validate values that did not come from these constants.
            pub const fn from_bits(bits: u32) -> Self {
                Self(bits)
            }

            /// True when every bit of `other` is also set in `self`.
            pub const fn contains(self, other: Self) -> bool {
                self.0 & other.0 == other.0
            }

            /// True when the two sets share at least one bit.
            pub const fn intersects(self, other: Self) -> bool {
                self.0 & other.0 != 0
            }
        }

        impl BitOr for $name {
            type Output = Self;
            fn bitor(self, rhs: Self) -> Self {
                Self(self.0 | rhs.0)
            }
        }

        impl BitOrAssign for $name {
            fn bitor_assign(&mut self, rhs: Self) {
                self.0 |= rhs.0;
            }
        }

        impl fmt::Debug for $name {
            fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                fmt::Display::fmt(self, f)
            }
        }

        impl fmt::Display for $name {
            fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                // Exact match first, so zero-valued and aliased names print.
                if let Some((name, _)) = Self::NAMED.iter().find(|(_, v)| *v == self.0) {
                    return f.write_str(name);
                }
                let mut rest = self.0;
                let mut first = true;
                for (name, value) in Self::NAMED {
                    if *value != 0 && value.count_ones() == 1 && rest & value != 0 {
                        if !first {
                            f.write_str("|")?;
                        }
                        f.write_str(name)?;
                        first = false;
                        rest &= !value;
                    }
                }
                if rest != 0 {
                    if !first {
                        f.write_str("|")?;
                    }
                    write!(f, "{rest:#x}")?;
                }
                Ok(())
            }
        }
    };
}

bit_set! {
    /// `D3D12_BARRIER_SYNC`: pipeline scopes a barrier waits for or blocks.
    Sync {
        NONE = 0,
        ALL = 0x1,
        DRAW = 0x2,
        INDEX_INPUT = 0x4,
        VERTEX_SHADING = 0x8,
        PIXEL_SHADING = 0x10,
        DEPTH_STENCIL = 0x20,
        RENDER_TARGET = 0x40,
        COMPUTE_SHADING = 0x80,
        RAYTRACING = 0x100,
        COPY = 0x200,
        RESOLVE = 0x400,
        EXECUTE_INDIRECT = 0x800,
        ALL_SHADING = 0x1000,
        NON_PIXEL_SHADING = 0x2000,
        EMIT_RAYTRACING_ACCELERATION_STRUCTURE_POSTBUILD_INFO = 0x4000,
        CLEAR_UNORDERED_ACCESS_VIEW = 0x8000,
        BUILD_RAYTRACING_ACCELERATION_STRUCTURE = 0x80_0000,
        COPY_RAYTRACING_ACCELERATION_STRUCTURE = 0x100_0000,
        /// Marks the begin (`SyncAfter`) or end (`SyncBefore`) of a split barrier.
        SPLIT = 0x8000_0000,
    }
}

bit_set! {
    /// `D3D12_BARRIER_ACCESS`: cache/visibility scopes of an access.
    Access {
        COMMON = 0,
        VERTEX_BUFFER = 0x1,
        CONSTANT_BUFFER = 0x2,
        INDEX_BUFFER = 0x4,
        RENDER_TARGET = 0x8,
        UNORDERED_ACCESS = 0x10,
        DEPTH_STENCIL_WRITE = 0x20,
        DEPTH_STENCIL_READ = 0x40,
        SHADER_RESOURCE = 0x80,
        STREAM_OUTPUT = 0x100,
        /// Same bit as `D3D12_BARRIER_ACCESS_PREDICATION`.
        INDIRECT_ARGUMENT = 0x200,
        COPY_DEST = 0x400,
        COPY_SOURCE = 0x800,
        RESOLVE_DEST = 0x1000,
        RESOLVE_SOURCE = 0x2000,
        RAYTRACING_ACCELERATION_STRUCTURE_READ = 0x4000,
        RAYTRACING_ACCELERATION_STRUCTURE_WRITE = 0x8000,
        SHADING_RATE_SOURCE = 0x1_0000,
        NO_ACCESS = 0x8000_0000,
    }
}

/// `D3D12_BARRIER_LAYOUT` for textures. Buffers have no layout.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum Layout {
    /// Contents are undefined. Only valid as an initial layout here; the
    /// planner never emits `LayoutAfter = UNDEFINED`.
    Undefined,
    /// `COMMON`, which has the same value as `PRESENT`.
    Common,
    GenericRead,
    RenderTarget,
    UnorderedAccess,
    DepthStencilWrite,
    DepthStencilRead,
    ShaderResource,
    CopySource,
    CopyDest,
    ResolveSource,
    ResolveDest,
    ShadingRateSource,
    DirectQueueCommon,
    DirectQueueGenericRead,
    DirectQueueUnorderedAccess,
    DirectQueueShaderResource,
    DirectQueueCopySource,
    DirectQueueCopyDest,
    ComputeQueueCommon,
    ComputeQueueGenericRead,
    ComputeQueueUnorderedAccess,
    ComputeQueueShaderResource,
    ComputeQueueCopySource,
    ComputeQueueCopyDest,
}

impl Layout {
    /// `D3D12_BARRIER_LAYOUT_PRESENT` is an alias of `COMMON`.
    pub const PRESENT: Layout = Layout::Common;

    /// Name used by the textual plan dump.
    pub fn name(self) -> &'static str {
        use Layout::*;
        match self {
            Undefined => "UNDEFINED",
            Common => "COMMON",
            GenericRead => "GENERIC_READ",
            RenderTarget => "RENDER_TARGET",
            UnorderedAccess => "UNORDERED_ACCESS",
            DepthStencilWrite => "DEPTH_STENCIL_WRITE",
            DepthStencilRead => "DEPTH_STENCIL_READ",
            ShaderResource => "SHADER_RESOURCE",
            CopySource => "COPY_SOURCE",
            CopyDest => "COPY_DEST",
            ResolveSource => "RESOLVE_SOURCE",
            ResolveDest => "RESOLVE_DEST",
            ShadingRateSource => "SHADING_RATE_SOURCE",
            DirectQueueCommon => "DIRECT_QUEUE_COMMON",
            DirectQueueGenericRead => "DIRECT_QUEUE_GENERIC_READ",
            DirectQueueUnorderedAccess => "DIRECT_QUEUE_UNORDERED_ACCESS",
            DirectQueueShaderResource => "DIRECT_QUEUE_SHADER_RESOURCE",
            DirectQueueCopySource => "DIRECT_QUEUE_COPY_SOURCE",
            DirectQueueCopyDest => "DIRECT_QUEUE_COPY_DEST",
            ComputeQueueCommon => "COMPUTE_QUEUE_COMMON",
            ComputeQueueGenericRead => "COMPUTE_QUEUE_GENERIC_READ",
            ComputeQueueUnorderedAccess => "COMPUTE_QUEUE_UNORDERED_ACCESS",
            ComputeQueueShaderResource => "COMPUTE_QUEUE_SHADER_RESOURCE",
            ComputeQueueCopySource => "COMPUTE_QUEUE_COPY_SOURCE",
            ComputeQueueCopyDest => "COMPUTE_QUEUE_COPY_DEST",
        }
    }
}

impl fmt::Display for Layout {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.name())
    }
}

/// Command queue type. Each queue owns one timeline fence in the plan.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum QueueKind {
    Direct,
    Compute,
    Copy,
}

impl QueueKind {
    pub fn name(self) -> &'static str {
        match self {
            QueueKind::Direct => "direct",
            QueueKind::Compute => "compute",
            QueueKind::Copy => "copy",
        }
    }
}

impl fmt::Display for QueueKind {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.name())
    }
}
