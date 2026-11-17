//! Vesper build of the planner: `graph_plan` returns the plan as records,
//! which are turned back into the public [`Plan`] here.

use super::binding as v;
use super::ffi;
use super::graph::RenderGraph;
use crate::d3d12::{Access, Sync};
use crate::graph::{BufferHandle, PassId, QueueId, ResourceRef, TextureHandle};
use crate::plan::{
    Barrier, BufferBarrier, Command, Dependency, Plan, PlanError, PlanOptions, QueueStream,
    TextureBarrier,
};

impl RenderGraph {
    /// Plans with the default options (split barriers enabled).
    pub fn plan(&self) -> Result<Plan, PlanError> {
        self.plan_with(PlanOptions::default())
    }

    /// Orders the passes and computes per-queue barrier batches and fence
    /// signals/waits. The result depends only on the graph contents.
    pub fn plan_with(&self, options: PlanOptions) -> Result<Plan, PlanError> {
        let out = v::graph_plan(&self.inner, options.split_barriers);
        let out = out.as_ref();
        let graph = self.id;
        let pass = |index: u32| PassId { graph, index };
        let texture = |index: u32| TextureHandle { graph, index };
        match out.fault() {
            ffi::PlanFault::None => {}
            ffi::PlanFault::Cycle => {
                return Err(PlanError::Cycle {
                    passes: out.cycle().iter().map(pass).collect(),
                });
            }
            ffi::PlanFault::NoLayoutForAccess {
                pass: p,
                resource,
                subresource,
                access,
            } => {
                return Err(PlanError::NoLayoutForAccess {
                    pass: pass(p),
                    resource: texture(resource),
                    subresource,
                    access: Access(access),
                });
            }
            ffi::PlanFault::UnsupportedQueueHandoff {
                pass: p,
                resource,
                subresource,
                from,
                to,
            } => {
                return Err(PlanError::UnsupportedQueueHandoff {
                    pass: pass(p),
                    resource: texture(resource),
                    subresource,
                    from,
                    to,
                });
            }
            ffi::PlanFault::FinalLayoutUnreachable {
                resource,
                subresource,
                from,
                to,
            } => {
                return Err(PlanError::FinalLayoutUnreachable {
                    resource: texture(resource),
                    subresource,
                    from,
                    to,
                });
            }
        }
        let barriers = out.barriers();
        let queue_count = v::graph_queue_count(&self.inner) as usize;
        let mut queues: Vec<QueueStream> = (0..queue_count)
            .map(|q| QueueStream {
                queue: QueueId {
                    graph,
                    index: q as u16,
                },
                kind: v::graph_queue_kind(&self.inner, q as u64).expect("planned queue"),
                commands: Vec::new(),
            })
            .collect();
        for c in out.cmds().iter() {
            let command = match c.tag {
                0 => Command::Wait {
                    queue: QueueId {
                        graph,
                        index: c.a as u16,
                    },
                    value: c.b,
                },
                1 => Command::Barriers(
                    (c.a..c.a + c.b)
                        .map(|i| barrier(graph, barriers.get(i as usize).expect("barrier")))
                        .collect(),
                ),
                2 => Command::Execute(pass(c.a as u32)),
                _ => Command::Signal { value: c.b },
            };
            queues[c.queue as usize].commands.push(command);
        }
        Ok(Plan {
            order: out.order().iter().map(pass).collect(),
            dependencies: out
                .deps()
                .iter()
                .map(|d| Dependency {
                    before: pass(d.before),
                    after: pass(d.after),
                    kind: d.kind,
                    resource: d.has_resource.then(|| {
                        if d.texture {
                            ResourceRef::Texture(texture(d.resource))
                        } else {
                            ResourceRef::Buffer(BufferHandle {
                                graph,
                                index: d.resource,
                            })
                        }
                    }),
                })
                .collect(),
            queues,
        })
    }
}

/// A planner barrier record as the public [`Barrier`] of graph `graph`.
pub(crate) fn barrier(graph: u32, b: ffi::BarrierRec) -> Barrier {
    if b.texture {
        Barrier::Texture(TextureBarrier {
            resource: TextureHandle {
                graph,
                index: b.resource,
            },
            sync_before: Sync(b.sync_before),
            sync_after: Sync(b.sync_after),
            access_before: Access(b.access_before),
            access_after: Access(b.access_after),
            layout_before: b.layout_before,
            layout_after: b.layout_after,
            subresources: b.subresources,
        })
    } else {
        Barrier::Buffer(BufferBarrier {
            resource: BufferHandle {
                graph,
                index: b.resource,
            },
            sync_before: Sync(b.sync_before),
            sync_after: Sync(b.sync_after),
            access_before: Access(b.access_before),
            access_after: Access(b.access_after),
        })
    }
}

/// The public [`Barrier`] as a planner barrier record.
pub(crate) fn barrier_rec(b: &Barrier) -> ffi::BarrierRec {
    match *b {
        Barrier::Texture(t) => ffi::BarrierRec {
            texture: true,
            resource: t.resource.index,
            sync_before: t.sync_before.0,
            sync_after: t.sync_after.0,
            access_before: t.access_before.0,
            access_after: t.access_after.0,
            layout_before: t.layout_before,
            layout_after: t.layout_after,
            subresources: t.subresources,
        },
        Barrier::Buffer(bb) => ffi::BarrierRec {
            texture: false,
            resource: bb.resource.index,
            sync_before: bb.sync_before.0,
            sync_after: bb.sync_after.0,
            access_before: bb.access_before.0,
            access_after: bb.access_after.0,
            layout_before: crate::d3d12::Layout::Undefined,
            layout_after: crate::d3d12::Layout::Undefined,
            subresources: crate::graph::SubresourceRange {
                first_mip: 0,
                mip_count: 0,
                first_slice: 0,
                slice_count: 0,
                first_plane: 0,
                plane_count: 0,
            },
        },
    }
}
