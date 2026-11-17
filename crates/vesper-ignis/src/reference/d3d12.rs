//! Rust build of the D3D12 compatibility tables (`crate::d3d12` types).

use crate::d3d12::{Access, Layout, QueueKind, Sync};

const ALL_SYNC_SUPERSEDES: u32 =
    0x2 | 0x4 | 0x8 | 0x10 | 0x20 | 0x40 | 0x80 | 0x100 | 0x200 | 0x400 | 0x800 | 0x8000;

impl Sync {
    /// Expands aggregate scopes (`ALL`, `DRAW`, `ALL_SHADING`,
    /// `NON_PIXEL_SHADING`) to the individual scopes the specification says
    /// they supersede. `ALL` is not treated as covering the raytracing
    /// acceleration-structure build/copy/postbuild scopes, which the
    /// specification's `ALL` table does not list; this is conservative.
    pub fn expanded(self) -> Sync {
        let mut bits = self.0 & !Self::SPLIT.0;
        if bits & Self::ALL.0 != 0 {
            bits |= ALL_SYNC_SUPERSEDES | Self::ALL_SHADING.0 | Self::NON_PIXEL_SHADING.0;
        }
        if bits & Self::DRAW.0 != 0 {
            bits |= Self::INDEX_INPUT.0
                | Self::VERTEX_SHADING.0
                | Self::PIXEL_SHADING.0
                | Self::DEPTH_STENCIL.0
                | Self::RENDER_TARGET.0;
        }
        if bits & Self::ALL_SHADING.0 != 0 {
            bits |= Self::VERTEX_SHADING.0 | Self::PIXEL_SHADING.0 | Self::COMPUTE_SHADING.0;
        }
        if bits & Self::NON_PIXEL_SHADING.0 != 0 {
            bits |= Self::VERTEX_SHADING.0 | Self::COMPUTE_SHADING.0;
        }
        Sync(bits)
    }

    /// True when the scopes in `self` include every scope in `other` after
    /// aggregate expansion. Aggregate bits in `other` are only covered by
    /// the same aggregate (or `ALL`), never by the union of their parts.
    pub fn covers(self, other: Sync) -> bool {
        let have = self.expanded();
        let need = other.expanded();
        have.contains(need)
    }

    fn union_nonempty(self, other: Sync) -> Sync {
        match (self == Sync::NONE, other == Sync::NONE) {
            (true, _) => other,
            (_, true) => self,
            _ => self | other,
        }
    }
}

const WRITE_ACCESS: u32 = 0x8 | 0x10 | 0x20 | 0x100 | 0x400 | 0x1000 | 0x8000;
const BUFFER_ONLY_ACCESS: u32 = 0x1 | 0x2 | 0x4 | 0x100 | 0x200 | 0x4000 | 0x8000;
const TEXTURE_ONLY_ACCESS: u32 = 0x8 | 0x20 | 0x40 | 0x1000 | 0x2000 | 0x1_0000;
const KNOWN_ACCESS: u32 = 0x1_ffff;

impl Access {
    /// True when any bit names a write access. `UNORDERED_ACCESS` is treated
    /// as a write because the API cannot tell a UAV read from a UAV write.
    pub fn is_write(self) -> bool {
        self != Self::NO_ACCESS && self.0 & WRITE_ACCESS != 0
    }

    pub(crate) fn union_nonempty(self, other: Access) -> Access {
        match (self == Access::NO_ACCESS, other == Access::NO_ACCESS) {
            (true, _) => other,
            (_, true) => self,
            _ => self | other,
        }
    }

    pub(crate) fn unknown_bits(self) -> u32 {
        self.0 & !KNOWN_ACCESS
    }

    pub(crate) fn buffer_only(self) -> bool {
        self.0 & BUFFER_ONLY_ACCESS != 0
    }

    pub(crate) fn texture_only(self) -> Access {
        Access(self.0 & TEXTURE_ONLY_ACCESS)
    }

    /// Bits of `self` for which none of the supplied sync scopes is listed as
    /// compatible ("at least one of the listed sync bits must also be
    /// provided").
    pub fn incompatible_with_sync(self, sync: Sync) -> Access {
        let mut bad = 0;
        let mut rest = self.0;
        while rest != 0 {
            let bit = rest.isolate_lowest_one();
            rest &= !bit;
            if sync.0 & compatible_sync(bit) == 0 {
                bad |= bit;
            }
        }
        Access(bad)
    }
}

fn compatible_sync(access_bit: u32) -> u32 {
    let s = |x: Sync| x.0;
    let all = s(Sync::ALL);
    let shading = s(Sync::VERTEX_SHADING)
        | s(Sync::PIXEL_SHADING)
        | s(Sync::COMPUTE_SHADING)
        | s(Sync::DRAW)
        | s(Sync::ALL_SHADING);
    let rtas = s(Sync::BUILD_RAYTRACING_ACCELERATION_STRUCTURE)
        | s(Sync::COPY_RAYTRACING_ACCELERATION_STRUCTURE);
    all | match access_bit {
        0x1 => s(Sync::VERTEX_SHADING) | s(Sync::DRAW) | s(Sync::ALL_SHADING),
        0x2 => shading,
        0x4 => s(Sync::INDEX_INPUT) | s(Sync::DRAW),
        0x8 => s(Sync::DRAW) | s(Sync::RENDER_TARGET),
        0x10 => {
            shading
                | s(Sync::CLEAR_UNORDERED_ACCESS_VIEW)
                | s(Sync::EMIT_RAYTRACING_ACCELERATION_STRUCTURE_POSTBUILD_INFO)
                | s(Sync::RAYTRACING)
        }
        0x20 | 0x40 => s(Sync::DRAW) | s(Sync::DEPTH_STENCIL),
        0x80 => shading | s(Sync::BUILD_RAYTRACING_ACCELERATION_STRUCTURE) | s(Sync::RAYTRACING),
        0x100 => s(Sync::VERTEX_SHADING) | s(Sync::DRAW) | s(Sync::ALL_SHADING),
        0x200 => s(Sync::EXECUTE_INDIRECT),
        0x400 | 0x800 => s(Sync::COPY),
        0x1000 | 0x2000 => s(Sync::RESOLVE),
        0x4000 => {
            s(Sync::COMPUTE_SHADING)
                | s(Sync::RAYTRACING)
                | s(Sync::ALL_SHADING)
                | rtas
                | s(Sync::EMIT_RAYTRACING_ACCELERATION_STRUCTURE_POSTBUILD_INFO)
        }
        0x8000 => s(Sync::COMPUTE_SHADING) | s(Sync::RAYTRACING) | s(Sync::ALL_SHADING) | rtas,
        0x1_0000 => s(Sync::PIXEL_SHADING) | s(Sync::ALL_SHADING),
        _ => 0,
    }
}

impl Layout {
    /// Raw `D3D12_BARRIER_LAYOUT` value.
    pub const fn raw(self) -> u32 {
        use Layout::*;
        match self {
            Undefined => 0xffff_ffff,
            Common => 0,
            GenericRead => 1,
            RenderTarget => 2,
            UnorderedAccess => 3,
            DepthStencilWrite => 4,
            DepthStencilRead => 5,
            ShaderResource => 6,
            CopySource => 7,
            CopyDest => 8,
            ResolveSource => 9,
            ResolveDest => 10,
            ShadingRateSource => 11,
            DirectQueueCommon => 18,
            DirectQueueGenericRead => 19,
            DirectQueueUnorderedAccess => 20,
            DirectQueueShaderResource => 21,
            DirectQueueCopySource => 22,
            DirectQueueCopyDest => 23,
            ComputeQueueCommon => 24,
            ComputeQueueGenericRead => 25,
            ComputeQueueUnorderedAccess => 26,
            ComputeQueueShaderResource => 27,
            ComputeQueueCopySource => 28,
            ComputeQueueCopyDest => 29,
        }
    }

    /// Accesses the specification lists as compatible with this layout.
    /// `UNDEFINED` admits only `NO_ACCESS` (the both-undefined case is not
    /// used by this planner).
    pub fn compatible_access(self) -> Access {
        use Layout::*;
        let a = |x: Access| x.0;
        let srv = a(Access::SHADER_RESOURCE);
        let src = a(Access::COPY_SOURCE);
        let dst = a(Access::COPY_DEST);
        let uav = a(Access::UNORDERED_ACCESS);
        Access(match self {
            Undefined => 0,
            Common => srv | src | dst,
            DirectQueueCommon | ComputeQueueCommon => srv | src | dst | uav,
            GenericRead | ComputeQueueGenericRead => srv | src,
            DirectQueueGenericRead => {
                srv | src
                    | a(Access::DEPTH_STENCIL_READ)
                    | a(Access::SHADING_RATE_SOURCE)
                    | a(Access::RESOLVE_SOURCE)
            }
            RenderTarget => a(Access::RENDER_TARGET),
            UnorderedAccess | DirectQueueUnorderedAccess | ComputeQueueUnorderedAccess => uav,
            DepthStencilWrite => a(Access::DEPTH_STENCIL_READ) | a(Access::DEPTH_STENCIL_WRITE),
            DepthStencilRead => a(Access::DEPTH_STENCIL_READ),
            ShaderResource | DirectQueueShaderResource | ComputeQueueShaderResource => srv,
            CopySource | DirectQueueCopySource | ComputeQueueCopySource => src,
            CopyDest | DirectQueueCopyDest | ComputeQueueCopyDest => dst,
            ResolveSource => a(Access::RESOLVE_SOURCE),
            ResolveDest => a(Access::RESOLVE_DEST),
            ShadingRateSource => a(Access::SHADING_RATE_SOURCE),
        })
    }

    /// True when every bit of `access` may be used in this layout.
    pub fn allows(self, access: Access) -> bool {
        access != Access::NO_ACCESS && self.compatible_access().contains(access)
    }

    /// Layouts tried, in order, when a texture use leaves its layout to the
    /// planner and the current layout does not admit the access.
    pub(crate) const AUTO_CANDIDATES: [Layout; 12] = [
        Layout::ShaderResource,
        Layout::CopySource,
        Layout::CopyDest,
        Layout::UnorderedAccess,
        Layout::RenderTarget,
        Layout::DepthStencilRead,
        Layout::DepthStencilWrite,
        Layout::ResolveSource,
        Layout::ResolveDest,
        Layout::ShadingRateSource,
        Layout::GenericRead,
        Layout::DirectQueueGenericRead,
    ];
}

impl QueueKind {
    /// Layouts a subresource may be in while used on this queue type.
    pub fn supports_layout(self, layout: Layout) -> bool {
        use Layout::*;
        match self {
            QueueKind::Direct => !matches!(
                layout,
                Undefined
                    | ComputeQueueCommon
                    | ComputeQueueGenericRead
                    | ComputeQueueUnorderedAccess
                    | ComputeQueueShaderResource
                    | ComputeQueueCopySource
                    | ComputeQueueCopyDest
            ),
            QueueKind::Compute => matches!(
                layout,
                Common
                    | GenericRead
                    | UnorderedAccess
                    | ShaderResource
                    | CopySource
                    | CopyDest
                    | ComputeQueueCommon
                    | ComputeQueueGenericRead
                    | ComputeQueueUnorderedAccess
                    | ComputeQueueShaderResource
                    | ComputeQueueCopySource
                    | ComputeQueueCopyDest
            ),
            QueueKind::Copy => layout == Common,
        }
    }

    /// Whether a barrier on this queue type may transition `before -> after`.
    /// "Both LayoutBefore and LayoutAfter must be compatible with the type of
    /// queue performing the Barrier"; `UNDEFINED` is accepted as a before
    /// layout. Copy queues do not support layout transitions at all.
    pub fn can_transition(self, before: Layout, after: Layout) -> bool {
        self != QueueKind::Copy
            && after != Layout::Undefined
            && (before == Layout::Undefined || self.supports_layout(before))
            && self.supports_layout(after)
    }

    /// Access bits this planner accepts on the queue type. This is a
    /// conservative model of which operations each command-list type records.
    pub fn supported_access(self) -> Access {
        match self {
            QueueKind::Direct => Access(KNOWN_ACCESS),
            QueueKind::Compute => {
                Access::CONSTANT_BUFFER
                    | Access::UNORDERED_ACCESS
                    | Access::SHADER_RESOURCE
                    | Access::INDIRECT_ARGUMENT
                    | Access::COPY_DEST
                    | Access::COPY_SOURCE
                    | Access::RAYTRACING_ACCELERATION_STRUCTURE_READ
                    | Access::RAYTRACING_ACCELERATION_STRUCTURE_WRITE
            }
            QueueKind::Copy => Access::COPY_DEST | Access::COPY_SOURCE,
        }
    }

    /// Sync scopes this planner accepts on the queue type.
    pub fn supported_sync(self) -> Sync {
        match self {
            QueueKind::Direct => Sync(0xffff | 0x0180_0000),
            QueueKind::Compute => {
                Sync::ALL
                    | Sync::COMPUTE_SHADING
                    | Sync::ALL_SHADING
                    | Sync::NON_PIXEL_SHADING
                    | Sync::RAYTRACING
                    | Sync::COPY
                    | Sync::EXECUTE_INDIRECT
                    | Sync::CLEAR_UNORDERED_ACCESS_VIEW
                    | Sync::EMIT_RAYTRACING_ACCELERATION_STRUCTURE_POSTBUILD_INFO
                    | Sync::BUILD_RAYTRACING_ACCELERATION_STRUCTURE
                    | Sync::COPY_RAYTRACING_ACCELERATION_STRUCTURE
            }
            QueueKind::Copy => Sync::ALL | Sync::COPY,
        }
    }
}

pub(crate) fn sync_union(a: Sync, b: Sync) -> Sync {
    a.union_nonempty(b)
}
