//! Typed resource and pass declarations.
//!
//! The declaration types are shared by both builds; `RenderGraph` itself is
//! in `reference/graph.rs` (Rust build) or `port/graph.rs` (Vesper build).

use std::fmt;
use std::sync::atomic::{AtomicU32, Ordering};

use crate::d3d12::{Access, Layout, QueueKind, Sync};

static NEXT_GRAPH_ID: AtomicU32 = AtomicU32::new(1);

/// A fresh graph id, shared by both builds so handles never collide.
pub(crate) fn next_graph_id() -> u32 {
    NEXT_GRAPH_ID.fetch_add(1, Ordering::Relaxed)
}

/// A queue registered with a [`RenderGraph`]. Each queue has one timeline
/// fence in the produced plan.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct QueueId {
    pub(crate) graph: u32,
    pub(crate) index: u16,
}

impl QueueId {
    pub fn index(self) -> usize {
        self.index as usize
    }
}

/// A buffer resource. Buffers have no layout and are tracked by byte range.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct BufferHandle {
    pub(crate) graph: u32,
    pub(crate) index: u32,
}

/// A texture resource. Textures are tracked per subresource
/// (mip level, array slice, plane), each with its own layout.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct TextureHandle {
    pub(crate) graph: u32,
    pub(crate) index: u32,
}

/// Either kind of resource, as reported in plans and errors.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum ResourceRef {
    Buffer(BufferHandle),
    Texture(TextureHandle),
}

impl ResourceRef {
    /// Index into the graph's resource table, shared by buffers and textures.
    pub fn index(self) -> usize {
        match self {
            ResourceRef::Buffer(b) => b.index as usize,
            ResourceRef::Texture(t) => t.index as usize,
        }
    }

    pub(crate) fn graph(self) -> u32 {
        match self {
            ResourceRef::Buffer(b) => b.graph,
            ResourceRef::Texture(t) => t.graph,
        }
    }
}

impl From<BufferHandle> for ResourceRef {
    fn from(value: BufferHandle) -> Self {
        ResourceRef::Buffer(value)
    }
}

impl From<TextureHandle> for ResourceRef {
    fn from(value: TextureHandle) -> Self {
        ResourceRef::Texture(value)
    }
}

/// A declared pass. Declaration order is the graph's sequential meaning:
/// every read observes the latest earlier overlapping write.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct PassId {
    pub(crate) graph: u32,
    pub(crate) index: u32,
}

impl PassId {
    pub fn index(self) -> usize {
        self.index as usize
    }
}

/// A byte range of a buffer. `size == u64::MAX` means "to the end".
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct ByteRange {
    pub offset: u64,
    pub size: u64,
}

impl ByteRange {
    pub const WHOLE: ByteRange = ByteRange {
        offset: 0,
        size: u64::MAX,
    };

    pub const fn new(offset: u64, size: u64) -> Self {
        ByteRange { offset, size }
    }
}

/// A box of texture subresources, mirroring
/// `D3D12_BARRIER_SUBRESOURCE_RANGE`. A count of `u32::MAX` means
/// "all remaining".
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct SubresourceRange {
    pub first_mip: u32,
    pub mip_count: u32,
    pub first_slice: u32,
    pub slice_count: u32,
    pub first_plane: u32,
    pub plane_count: u32,
}

impl SubresourceRange {
    pub const ALL: SubresourceRange = SubresourceRange {
        first_mip: 0,
        mip_count: u32::MAX,
        first_slice: 0,
        slice_count: u32::MAX,
        first_plane: 0,
        plane_count: u32::MAX,
    };

    /// `count` mips starting at `first`, all slices and planes.
    pub const fn mips(first: u32, count: u32) -> Self {
        SubresourceRange {
            first_mip: first,
            mip_count: count,
            ..Self::ALL
        }
    }

    /// `count` array slices starting at `first`, all mips and planes.
    pub const fn slices(first: u32, count: u32) -> Self {
        SubresourceRange {
            first_slice: first,
            slice_count: count,
            ..Self::ALL
        }
    }

    /// Exactly one subresource.
    pub const fn single(mip: u32, slice: u32, plane: u32) -> Self {
        SubresourceRange {
            first_mip: mip,
            mip_count: 1,
            first_slice: slice,
            slice_count: 1,
            first_plane: plane,
            plane_count: 1,
        }
    }
}

impl fmt::Display for SubresourceRange {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "mips {}+{} slices {}+{} planes {}+{}",
            self.first_mip,
            self.mip_count,
            self.first_slice,
            self.slice_count,
            self.first_plane,
            self.plane_count
        )
    }
}

/// Buffer declaration.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct BufferDesc {
    pub name: String,
    pub size: u64,
}

/// Texture declaration. `initial_layout` is the layout the subresources are
/// in when the graph starts (`Undefined` for transient textures whose
/// contents are discarded). If `final_layout` is set, the plan ends with
/// every subresource in that layout.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TextureDesc {
    pub name: String,
    pub mip_levels: u32,
    pub array_size: u32,
    pub plane_count: u32,
    pub initial_layout: Layout,
    pub final_layout: Option<Layout>,
}

impl TextureDesc {
    /// One mip, one slice, one plane.
    pub fn simple(name: impl Into<String>, initial_layout: Layout) -> Self {
        TextureDesc {
            name: name.into(),
            mip_levels: 1,
            array_size: 1,
            plane_count: 1,
            initial_layout,
            final_layout: None,
        }
    }

    pub fn with_mips(mut self, mip_levels: u32) -> Self {
        self.mip_levels = mip_levels;
        self
    }

    pub fn with_array_size(mut self, array_size: u32) -> Self {
        self.array_size = array_size;
        self
    }

    pub fn with_final_layout(mut self, layout: Layout) -> Self {
        self.final_layout = Some(layout);
        self
    }
}

#[derive(Clone, Debug)]
pub(crate) enum UseDescRange {
    Bytes(ByteRange),
    Subresources(SubresourceRange),
}

#[derive(Clone, Debug)]
pub(crate) struct UseDesc {
    pub(crate) resource: ResourceRef,
    pub(crate) range: UseDescRange,
    pub(crate) access: Access,
    pub(crate) sync: Sync,
    pub(crate) layout: Option<Layout>,
}

/// A pass to add to a graph: a name, the queue it executes on, and the
/// resource ranges it accesses.
#[derive(Clone, Debug)]
pub struct PassDesc {
    pub(crate) name: String,
    pub(crate) queue: QueueId,
    pub(crate) uses: Vec<UseDesc>,
}

impl PassDesc {
    pub fn new(name: impl Into<String>, queue: QueueId) -> Self {
        PassDesc {
            name: name.into(),
            queue,
            uses: Vec::new(),
        }
    }

    /// Declares an access to a byte range of a buffer.
    pub fn buffer(
        mut self,
        buffer: BufferHandle,
        range: ByteRange,
        access: Access,
        sync: Sync,
    ) -> Self {
        self.uses.push(UseDesc {
            resource: buffer.into(),
            range: UseDescRange::Bytes(range),
            access,
            sync,
            layout: None,
        });
        self
    }

    /// Declares an access to texture subresources. With `layout == None` the
    /// planner keeps the current layout when it admits the access and
    /// otherwise chooses a layout for it.
    pub fn texture(
        mut self,
        texture: TextureHandle,
        range: SubresourceRange,
        access: Access,
        sync: Sync,
        layout: impl Into<Option<Layout>>,
    ) -> Self {
        self.uses.push(UseDesc {
            resource: texture.into(),
            range: UseDescRange::Subresources(range),
            access,
            sync,
            layout: layout.into(),
        });
        self
    }
}

/// Why a queue rejected a use.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum QueueRestriction {
    Access(Access),
    Sync(Sync),
    Layout(Layout),
}

/// Structured errors raised while declaring a graph.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum GraphError {
    /// A handle from a different graph was used.
    ForeignHandle,
    InvalidResource {
        name: String,
        reason: &'static str,
    },
    /// `access` is empty (`COMMON`), `NO_ACCESS`, or has unknown bits.
    InvalidAccess {
        pass: String,
        use_index: usize,
        access: Access,
    },
    /// `sync` is `NONE`, `SPLIT`, or has unknown bits for the queue.
    InvalidSync {
        pass: String,
        use_index: usize,
        sync: Sync,
    },
    /// No supplied sync scope is compatible with these access bits.
    AccessSyncMismatch {
        pass: String,
        use_index: usize,
        access: Access,
        sync: Sync,
    },
    /// The explicit layout does not admit the access.
    AccessLayoutMismatch {
        pass: String,
        use_index: usize,
        access: Access,
        layout: Layout,
    },
    /// Access only meaningful for the other resource kind (e.g. a
    /// render-target access on a buffer).
    AccessResourceKindMismatch {
        pass: String,
        use_index: usize,
        access: Access,
    },
    /// The pass's queue type cannot perform this use.
    QueueUnsupported {
        pass: String,
        use_index: usize,
        queue: QueueKind,
        restriction: QueueRestriction,
    },
    /// Empty or out-of-bounds range.
    InvalidRange {
        pass: String,
        use_index: usize,
    },
    /// Two uses in the same pass overlap and either one writes or their
    /// explicit layouts differ.
    ConflictingUses {
        pass: String,
        first_use: usize,
        second_use: usize,
    },
    /// A pass was ordered after itself.
    SelfDependency {
        pass: PassId,
    },
}

impl fmt::Display for GraphError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            GraphError::ForeignHandle => write!(f, "handle belongs to a different render graph"),
            GraphError::InvalidResource { name, reason } => {
                write!(f, "resource `{name}` is invalid: {reason}")
            }
            GraphError::InvalidAccess {
                pass,
                use_index,
                access,
            } => {
                write!(f, "pass `{pass}` use {use_index}: invalid access {access}")
            }
            GraphError::InvalidSync {
                pass,
                use_index,
                sync,
            } => {
                write!(f, "pass `{pass}` use {use_index}: invalid sync {sync}")
            }
            GraphError::AccessSyncMismatch {
                pass,
                use_index,
                access,
                sync,
            } => write!(
                f,
                "pass `{pass}` use {use_index}: access {access} is not compatible with sync {sync}"
            ),
            GraphError::AccessLayoutMismatch {
                pass,
                use_index,
                access,
                layout,
            } => write!(
                f,
                "pass `{pass}` use {use_index}: access {access} is not compatible with layout {layout}"
            ),
            GraphError::AccessResourceKindMismatch {
                pass,
                use_index,
                access,
            } => write!(
                f,
                "pass `{pass}` use {use_index}: access {access} does not apply to this resource kind"
            ),
            GraphError::QueueUnsupported {
                pass,
                use_index,
                queue,
                restriction,
            } => write!(
                f,
                "pass `{pass}` use {use_index}: {queue} queue does not support {restriction:?}"
            ),
            GraphError::InvalidRange { pass, use_index } => {
                write!(
                    f,
                    "pass `{pass}` use {use_index}: empty or out-of-bounds range"
                )
            }
            GraphError::ConflictingUses {
                pass,
                first_use,
                second_use,
            } => write!(
                f,
                "pass `{pass}`: uses {first_use} and {second_use} overlap with a write or differing layouts"
            ),
            GraphError::SelfDependency { pass } => {
                write!(f, "pass {} cannot depend on itself", pass.index)
            }
        }
    }
}

impl std::error::Error for GraphError {}
