//! Vesper build of the D3D12 compatibility tables: every method forwards
//! to `vesper/src/lib.ves` through the generated binding.

use super::binding as v;
use crate::d3d12::{Access, Layout, QueueKind, Sync};

impl Sync {
    /// Expands aggregate scopes (`ALL`, `DRAW`, `ALL_SHADING`,
    /// `NON_PIXEL_SHADING`) to the individual scopes the specification says
    /// they supersede. `ALL` is not treated as covering the raytracing
    /// acceleration-structure build/copy/postbuild scopes, which the
    /// specification's `ALL` table does not list; this is conservative.
    pub fn expanded(self) -> Sync {
        Sync(v::sync_expanded(self.0))
    }

    /// True when the scopes in `self` include every scope in `other` after
    /// aggregate expansion. Aggregate bits in `other` are only covered by
    /// the same aggregate (or `ALL`), never by the union of their parts.
    pub fn covers(self, other: Sync) -> bool {
        v::sync_covers(self.0, other.0)
    }
}

impl Access {
    /// True when any bit names a write access. `UNORDERED_ACCESS` is treated
    /// as a write because the API cannot tell a UAV read from a UAV write.
    pub fn is_write(self) -> bool {
        v::access_is_write(self.0)
    }

    /// Bits of `self` for which none of the supplied sync scopes is listed as
    /// compatible ("at least one of the listed sync bits must also be
    /// provided").
    pub fn incompatible_with_sync(self, sync: Sync) -> Access {
        Access(v::access_incompatible_with_sync(self.0, sync.0))
    }
}

impl Layout {
    /// Raw `D3D12_BARRIER_LAYOUT` value.
    pub fn raw(self) -> u32 {
        v::layout_raw(self)
    }

    /// Accesses the specification lists as compatible with this layout.
    /// `UNDEFINED` admits only `NO_ACCESS` (the both-undefined case is not
    /// used by this planner).
    pub fn compatible_access(self) -> Access {
        Access(v::layout_compatible_access(self))
    }

    /// True when every bit of `access` may be used in this layout.
    pub fn allows(self, access: Access) -> bool {
        v::layout_allows(self, access.0)
    }
}

impl QueueKind {
    /// Layouts a subresource may be in while used on this queue type.
    pub fn supports_layout(self, layout: Layout) -> bool {
        v::queue_supports_layout(self, layout)
    }

    /// Whether a barrier on this queue type may transition `before -> after`.
    /// "Both LayoutBefore and LayoutAfter must be compatible with the type of
    /// queue performing the Barrier"; `UNDEFINED` is accepted as a before
    /// layout. Copy queues do not support layout transitions at all.
    pub fn can_transition(self, before: Layout, after: Layout) -> bool {
        v::queue_can_transition(self, before, after)
    }

    /// Access bits this planner accepts on the queue type. This is a
    /// conservative model of which operations each command-list type records.
    pub fn supported_access(self) -> Access {
        Access(v::queue_supported_access(self))
    }

    /// Sync scopes this planner accepts on the queue type.
    pub fn supported_sync(self) -> Sync {
        Sync(v::queue_supported_sync(self))
    }
}
