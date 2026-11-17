//! Plan data: barriers, fence commands and errors, shared by both builds.
//! The planner itself is in `reference/plan.rs` (Rust build) or
//! `port/plan.rs` (Vesper build).

use std::fmt::{self, Write as _};

use crate::RenderGraph;
use crate::d3d12::{Access, Layout, QueueKind, Sync};
use crate::graph::{BufferHandle, PassId, QueueId, ResourceRef, SubresourceRange, TextureHandle};

/// Planner options.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct PlanOptions {
    /// Emit split barriers (`SyncAfter = SPLIT` after the producer,
    /// `SyncBefore = SPLIT` before the consumer) when other passes on the same
    /// queue run between the last access and the next one.
    pub split_barriers: bool,
}

impl Default for PlanOptions {
    fn default() -> Self {
        PlanOptions {
            split_barriers: true,
        }
    }
}

/// Why one pass must run after another.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum DependencyKind {
    ReadAfterWrite,
    WriteAfterRead,
    WriteAfterWrite,
    /// Two overlapping reads whose layouts are not both explicit and equal.
    /// They are kept in declaration order because a layout transition may be
    /// needed between them.
    ReadOrder,
    /// Added with [`RenderGraph::add_dependency`].
    Explicit,
}

/// An ordering edge derived from the graph.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct Dependency {
    pub before: PassId,
    pub after: PassId,
    pub kind: DependencyKind,
    pub resource: Option<ResourceRef>,
}

/// `D3D12_BUFFER_BARRIER`. Enhanced barriers require `Offset = 0` and
/// `Size = UINT64_MAX` (or the buffer size), so buffer barriers always cover
/// the whole buffer even though hazards are tracked per byte range.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct BufferBarrier {
    pub resource: BufferHandle,
    pub sync_before: Sync,
    pub sync_after: Sync,
    pub access_before: Access,
    pub access_after: Access,
}

/// `D3D12_TEXTURE_BARRIER` for one box of subresources.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct TextureBarrier {
    pub resource: TextureHandle,
    pub sync_before: Sync,
    pub sync_after: Sync,
    pub access_before: Access,
    pub access_after: Access,
    pub layout_before: Layout,
    pub layout_after: Layout,
    pub subresources: SubresourceRange,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Barrier {
    Buffer(BufferBarrier),
    Texture(TextureBarrier),
}

/// One step of a queue's command stream. Every `Wait` and `Signal` is an
/// `ID3D12CommandQueue::Wait`/`Signal` on the queue timeline fences, and so
/// implies an `ExecuteCommandLists` boundary; how passes between those
/// boundaries are grouped into command lists is left to the executor.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Command {
    /// Wait until `queue`'s fence reaches `value`.
    Wait {
        queue: QueueId,
        value: u64,
    },
    /// One `ID3D12GraphicsCommandList7::Barrier` batch.
    Barriers(Vec<Barrier>),
    Execute(PassId),
    /// Signal this queue's fence with `value` (strictly increasing per queue).
    Signal {
        value: u64,
    },
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct QueueStream {
    pub queue: QueueId,
    pub kind: QueueKind,
    pub commands: Vec<Command>,
}

/// A deterministic barrier/fence plan.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Plan {
    /// A global topological order of the passes, consistent with every
    /// queue's stream.
    pub order: Vec<PassId>,
    pub dependencies: Vec<Dependency>,
    pub queues: Vec<QueueStream>,
}

/// Structured planning errors.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum PlanError {
    /// The ordering constraints form a cycle. `passes[i]` must run before
    /// `passes[i + 1]`, and the last before the first.
    Cycle { passes: Vec<PassId> },
    /// No layout supported on the pass's queue admits the requested access.
    NoLayoutForAccess {
        pass: PassId,
        resource: TextureHandle,
        subresource: u32,
        access: Access,
    },
    /// The pass's queue cannot transition the subresource out of its current
    /// layout, and no single other queue that last used it can do so first.
    UnsupportedQueueHandoff {
        pass: PassId,
        resource: TextureHandle,
        subresource: u32,
        from: Layout,
        to: Layout,
    },
    /// No queue can perform the requested final layout transition.
    FinalLayoutUnreachable {
        resource: TextureHandle,
        subresource: u32,
        from: Layout,
        to: Layout,
    },
}

impl fmt::Display for PlanError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            PlanError::Cycle { passes } => {
                let ids: Vec<String> = passes.iter().map(|p| p.index().to_string()).collect();
                write!(f, "ordering cycle through passes {}", ids.join(" -> "))
            }
            PlanError::NoLayoutForAccess {
                pass,
                subresource,
                access,
                ..
            } => write!(
                f,
                "pass {}: no layout on its queue admits access {access} (subresource {subresource})",
                pass.index()
            ),
            PlanError::UnsupportedQueueHandoff {
                pass,
                subresource,
                from,
                to,
                ..
            } => write!(
                f,
                "pass {}: cannot transition subresource {subresource} from {from} to {to} on any single queue",
                pass.index()
            ),
            PlanError::FinalLayoutUnreachable {
                subresource,
                from,
                to,
                ..
            } => {
                write!(
                    f,
                    "no queue can transition subresource {subresource} from {from} to {to}"
                )
            }
        }
    }
}

impl std::error::Error for PlanError {}

impl Plan {
    /// Number of barriers (split halves counted separately).
    pub fn barrier_count(&self) -> usize {
        self.queues
            .iter()
            .flat_map(|q| &q.commands)
            .map(|c| {
                if let Command::Barriers(b) = c {
                    b.len()
                } else {
                    0
                }
            })
            .sum()
    }

    /// Stable, human-readable rendering of the plan for review and golden
    /// tests.
    pub fn render(&self, graph: &RenderGraph) -> String {
        let name = |p: PassId| graph.pass_name(p).unwrap_or("?").to_owned();
        let mut out = String::new();
        let order: Vec<String> = self.order.iter().map(|&p| name(p)).collect();
        let _ = writeln!(out, "order: {}", order.join(", "));
        for q in &self.queues {
            let _ = writeln!(out, "queue {} ({}):", q.queue.index(), q.kind);
            for c in &q.commands {
                match c {
                    Command::Wait { queue, value } => {
                        let _ = writeln!(out, "  wait q{} >= {value}", queue.index());
                    }
                    Command::Signal { value } => {
                        let _ = writeln!(out, "  signal {value}");
                    }
                    Command::Execute(p) => {
                        let _ = writeln!(out, "  execute {}", name(*p));
                    }
                    Command::Barriers(list) => {
                        let _ = writeln!(out, "  barriers:");
                        for b in list {
                            match b {
                                Barrier::Buffer(b) => {
                                    let _ = writeln!(
                                        out,
                                        "    buffer {} sync {}->{} access {}->{}",
                                        graph.resource_name(b.resource).unwrap_or("?"),
                                        b.sync_before,
                                        b.sync_after,
                                        b.access_before,
                                        b.access_after
                                    );
                                }
                                Barrier::Texture(t) => {
                                    let _ = writeln!(
                                        out,
                                        "    texture {} [{}] sync {}->{} access {}->{} layout {}->{}",
                                        graph.resource_name(t.resource).unwrap_or("?"),
                                        t.subresources,
                                        t.sync_before,
                                        t.sync_after,
                                        t.access_before,
                                        t.access_after,
                                        t.layout_before,
                                        t.layout_after
                                    );
                                }
                            }
                        }
                    }
                }
            }
        }
        out
    }
}
