//! Vesper build of `RenderGraph`: the graph lives in a Vesper `Graph`
//! handle; Rust keeps the graph id and the resource and pass names.

use super::binding as v;
use super::ffi;
use crate::d3d12::{Access, Layout, QueueKind, Sync};
use crate::graph::{
    BufferDesc, BufferHandle, GraphError, PassDesc, PassId, QueueId, QueueRestriction, ResourceRef,
    SubresourceRange, TextureDesc, TextureHandle, UseDescRange, next_graph_id,
};

/// A render graph: queues, typed resources, passes in declaration order and
/// optional explicit ordering edges.
#[derive(Debug)]
pub struct RenderGraph {
    pub(crate) id: u32,
    pub(crate) inner: v::Graph,
    resource_names: Vec<String>,
    pass_names: Vec<String>,
}

impl Default for RenderGraph {
    fn default() -> Self {
        Self::new()
    }
}

const NO_RANGE: SubresourceRange = SubresourceRange {
    first_mip: 0,
    mip_count: 0,
    first_slice: 0,
    slice_count: 0,
    first_plane: 0,
    plane_count: 0,
};

impl RenderGraph {
    pub fn new() -> Self {
        RenderGraph {
            id: next_graph_id(),
            inner: v::graph_new(),
            resource_names: Vec::new(),
            pass_names: Vec::new(),
        }
    }

    pub fn add_queue(&mut self, kind: QueueKind) -> QueueId {
        match v::graph_add_queue(&mut self.inner, kind) {
            ffi::QueueOutcome::Added(index) => QueueId {
                graph: self.id,
                index: index as u16,
            },
            ffi::QueueOutcome::TooManyQueues => panic!("more than 65535 queues"),
        }
    }

    pub fn queue_kind(&self, queue: QueueId) -> Option<QueueKind> {
        (queue.graph == self.id)
            .then(|| v::graph_queue_kind(&self.inner, queue.index() as u64))
            .flatten()
    }

    fn resource(&mut self, name: String, outcome: ffi::ResourceOutcome) -> Result<u32, GraphError> {
        let reason = match outcome {
            ffi::ResourceOutcome::Added(index) => {
                self.resource_names.push(name);
                return Ok(index);
            }
            ffi::ResourceOutcome::ZeroSize => "buffer size is zero",
            ffi::ResourceOutcome::ZeroDimension => "texture has a zero dimension",
            ffi::ResourceOutcome::CountOverflow => "subresource count overflows u32",
            ffi::ResourceOutcome::TooManySubresources => "more than 2^20 subresources",
            ffi::ResourceOutcome::FinalUndefined => "final layout cannot be UNDEFINED",
            ffi::ResourceOutcome::TooManyResources => panic!("more than u32::MAX resources"),
        };
        Err(GraphError::InvalidResource { name, reason })
    }

    pub fn add_buffer(&mut self, desc: BufferDesc) -> Result<BufferHandle, GraphError> {
        let outcome = v::graph_add_buffer(&mut self.inner, desc.size);
        let index = self.resource(desc.name, outcome)?;
        Ok(BufferHandle {
            graph: self.id,
            index,
        })
    }

    pub fn add_texture(&mut self, desc: TextureDesc) -> Result<TextureHandle, GraphError> {
        let outcome = v::graph_add_texture(
            &mut self.inner,
            desc.mip_levels,
            desc.array_size,
            desc.plane_count,
            desc.initial_layout,
            desc.final_layout.is_some(),
            desc.final_layout.unwrap_or(Layout::Undefined),
        );
        let index = self.resource(desc.name, outcome)?;
        Ok(TextureHandle {
            graph: self.id,
            index,
        })
    }

    pub fn resource_name(&self, resource: impl Into<ResourceRef>) -> Option<&str> {
        let r = resource.into();
        (r.graph() == self.id)
            .then(|| self.resource_names.get(r.index()).map(String::as_str))
            .flatten()
    }

    pub fn pass_name(&self, pass: PassId) -> Option<&str> {
        (pass.graph == self.id)
            .then(|| self.pass_names.get(pass.index()).map(String::as_str))
            .flatten()
    }

    pub fn pass_count(&self) -> usize {
        self.pass_names.len()
    }

    /// Validates and appends a pass. Nothing is added on error.
    pub fn add_pass(&mut self, desc: PassDesc) -> Result<PassId, GraphError> {
        let queue = if desc.queue.graph == self.id {
            desc.queue.index() as u64
        } else {
            u64::MAX
        };
        let descs: Vec<v::repr::UseDesc> = desc
            .uses
            .iter()
            .map(|u| {
                let (bytes, offset, size, range) = match u.range {
                    UseDescRange::Bytes(r) => (true, r.offset, r.size, NO_RANGE),
                    UseDescRange::Subresources(r) => (false, 0, 0, r),
                };
                v::repr::UseDesc::from_value(&ffi::UseDesc {
                    foreign: u.resource.graph() != self.id,
                    resource: u.resource.index() as u32,
                    bytes,
                    offset,
                    size,
                    range,
                    access: u.access.0,
                    sync: u.sync.0,
                    has_layout: u.layout.is_some(),
                    layout: u.layout.unwrap_or(Layout::Undefined),
                })
            })
            .collect();
        let outcome = v::graph_add_pass(&mut self.inner, queue, &descs);
        let kind = || self.queue_kind(desc.queue).unwrap_or(QueueKind::Direct);
        let pass = || desc.name.clone();
        let restriction = |use_index: u64, restriction| GraphError::QueueUnsupported {
            pass: pass(),
            use_index: use_index as usize,
            queue: kind(),
            restriction,
        };
        Err(match outcome {
            ffi::PassOutcome::Added(index) => {
                self.pass_names.push(desc.name);
                return Ok(PassId {
                    graph: self.id,
                    index,
                });
            }
            ffi::PassOutcome::ForeignHandle => GraphError::ForeignHandle,
            ffi::PassOutcome::InvalidAccess { use_index, access } => GraphError::InvalidAccess {
                pass: pass(),
                use_index: use_index as usize,
                access: Access(access),
            },
            ffi::PassOutcome::InvalidSync { use_index, sync } => GraphError::InvalidSync {
                pass: pass(),
                use_index: use_index as usize,
                sync: Sync(sync),
            },
            ffi::PassOutcome::AccessSyncMismatch {
                use_index,
                access,
                sync,
            } => GraphError::AccessSyncMismatch {
                pass: pass(),
                use_index: use_index as usize,
                access: Access(access),
                sync: Sync(sync),
            },
            ffi::PassOutcome::AccessLayoutMismatch {
                use_index,
                access,
                layout,
            } => GraphError::AccessLayoutMismatch {
                pass: pass(),
                use_index: use_index as usize,
                access: Access(access),
                layout,
            },
            ffi::PassOutcome::AccessResourceKindMismatch { use_index, access } => {
                GraphError::AccessResourceKindMismatch {
                    pass: pass(),
                    use_index: use_index as usize,
                    access: Access(access),
                }
            }
            ffi::PassOutcome::QueueAccess { use_index, access } => {
                restriction(use_index, QueueRestriction::Access(Access(access)))
            }
            ffi::PassOutcome::QueueSync { use_index, sync } => {
                restriction(use_index, QueueRestriction::Sync(Sync(sync)))
            }
            ffi::PassOutcome::QueueLayout { use_index, layout } => {
                restriction(use_index, QueueRestriction::Layout(layout))
            }
            ffi::PassOutcome::InvalidRange { use_index } => GraphError::InvalidRange {
                pass: pass(),
                use_index: use_index as usize,
            },
            ffi::PassOutcome::ConflictingUses {
                first_use,
                second_use,
            } => GraphError::ConflictingUses {
                pass: pass(),
                first_use: first_use as usize,
                second_use: second_use as usize,
            },
            ffi::PassOutcome::TooManyPasses => panic!("more than u32::MAX passes"),
        })
    }

    /// Requires `after` to execute after `before` even without a resource
    /// hazard. On different queues this produces a fence wait; on the same
    /// queue it only constrains order (no memory barrier is implied).
    pub fn add_dependency(&mut self, before: PassId, after: PassId) -> Result<(), GraphError> {
        let index = |p: PassId| {
            if p.graph == self.id {
                p.index as u64
            } else {
                u64::MAX
            }
        };
        let (b, a) = (index(before), index(after));
        match v::graph_add_dependency(&mut self.inner, b, a) {
            ffi::DependencyOutcome::Added | ffi::DependencyOutcome::AlreadyPresent => Ok(()),
            ffi::DependencyOutcome::ForeignHandle => Err(GraphError::ForeignHandle),
            ffi::DependencyOutcome::SelfDependency => {
                Err(GraphError::SelfDependency { pass: before })
            }
        }
    }
}
