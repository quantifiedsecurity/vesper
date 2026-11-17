//! Vesper build of the multi-backend lowering: the stage and access tables,
//! the per-barrier lowering and the split-event pairing run in Vesper; the
//! lowered records are matched back to the planner barriers they came from.

use super::binding as v;
use super::ffi;
use super::plan::barrier_rec;
use crate::d3d12::{Access, QueueKind, Sync};
use crate::graph::ResourceRef;
use crate::multi_backend::{
    GpuApiTarget, LoweredCommand, LoweredPlan, LoweredQueueStream, NativeBackendBarrier,
};
use crate::plan::{Barrier, Command, Plan};

/// Lower a single planner [`Barrier`] into a [`NativeBackendBarrier`]
/// (see the Rust build for the per-target rules).
pub fn lower_to_target_barrier(
    target: GpuApiTarget,
    barrier: &Barrier,
    split_event_id: u32,
) -> Option<NativeBackendBarrier> {
    lower_to_target_barrier_for_queue(target, QueueKind::Direct, barrier, split_event_id)
}

/// Lower a single planner [`Barrier`] on a specific [`QueueKind`].
pub fn lower_to_target_barrier_for_queue(
    target: GpuApiTarget,
    queue_kind: QueueKind,
    barrier: &Barrier,
    split_event_id: u32,
) -> Option<NativeBackendBarrier> {
    let n = v::lower_barrier(
        target,
        queue_kind,
        barrier_rec(barrier),
        split_event_id,
        false,
        0,
    );
    native(barrier, n)
}

fn resource_of(b: &Barrier) -> ResourceRef {
    match *b {
        Barrier::Buffer(bb) => ResourceRef::Buffer(bb.resource),
        Barrier::Texture(tb) => ResourceRef::Texture(tb.resource),
    }
}

/// A lowered record with the handles, layouts and ranges of the barrier it
/// lowers.
fn native(b: &Barrier, n: ffi::Native) -> Option<NativeBackendBarrier> {
    Some(match n {
        ffi::Native::Elided => return None,
        ffi::Native::D3D12 => NativeBackendBarrier::D3D12(*b),
        ffi::Native::BufferBarrier2 {
            src_stage_mask,
            dst_stage_mask,
            src_access_mask,
            dst_access_mask,
        } => match *b {
            Barrier::Buffer(bb) => NativeBackendBarrier::Vulkan14BufferBarrier2 {
                resource: bb.resource,
                src_stage_mask,
                dst_stage_mask,
                src_access_mask,
                dst_access_mask,
            },
            Barrier::Texture(_) => unreachable!("buffer lowering of a texture barrier"),
        },
        ffi::Native::ImageBarrier2 {
            src_stage_mask,
            dst_stage_mask,
            src_access_mask,
            dst_access_mask,
        } => match *b {
            Barrier::Texture(tb) => NativeBackendBarrier::Vulkan14ImageBarrier2 {
                resource: tb.resource,
                subresources: tb.subresources,
                src_stage_mask,
                dst_stage_mask,
                src_access_mask,
                dst_access_mask,
                old_layout: tb.layout_before,
                new_layout: tb.layout_after,
            },
            Barrier::Buffer(_) => unreachable!("image lowering of a buffer barrier"),
        },
        ffi::Native::SplitEventSignal {
            event_id,
            src_stage_mask,
        } => NativeBackendBarrier::Vulkan14SplitEventSignal {
            event_id,
            resource: resource_of(b),
            src_stage_mask,
        },
        ffi::Native::SplitEventWait {
            event_id,
            dst_stage_mask,
            src_access_mask,
            dst_access_mask,
        } => {
            let texture = match *b {
                Barrier::Texture(tb) => Some(tb),
                Barrier::Buffer(_) => None,
            };
            NativeBackendBarrier::Vulkan14SplitEventWait {
                event_id,
                resource: resource_of(b),
                dst_stage_mask,
                src_access_mask,
                dst_access_mask,
                old_layout: texture.map(|t| t.layout_before),
                new_layout: texture.map(|t| t.layout_after),
                subresources: texture.map(|t| t.subresources),
            }
        }
        ffi::Native::Metal4StageBarrier {
            after_stages_mask,
            before_stages_mask,
        } => NativeBackendBarrier::Metal4StageBarrier {
            after_stages_mask,
            before_stages_mask,
        },
    })
}

/// Lower every queue stream and barrier batch in a [`Plan`] to `target`.
pub fn lower_plan(target: GpuApiTarget, plan: &Plan) -> LoweredPlan {
    let kinds: Vec<QueueKind> = plan.queues.iter().map(|q| q.kind).collect();
    let mut commands: Vec<&Command> = Vec::new();
    let mut cmds: Vec<ffi::CmdRec> = Vec::new();
    let mut flat: Vec<&Barrier> = Vec::new();
    let mut keyed: Vec<ffi::KeyedBarrier> = Vec::new();
    for (stream, q) in plan.queues.iter().enumerate() {
        for c in &q.commands {
            let (tag, a, b) = match c {
                Command::Wait { queue, value } => (0, queue.index() as u64, *value),
                Command::Barriers(bs) => {
                    let start = flat.len() as u64;
                    for b in bs {
                        flat.push(b);
                        keyed.push(ffi::KeyedBarrier {
                            graph: resource_of(b).graph(),
                            barrier: barrier_rec(b),
                        });
                    }
                    (1, start, bs.len() as u64)
                }
                Command::Execute(pass) => (2, pass.index() as u64, 0),
                Command::Signal { value } => (3, 0, *value),
            };
            commands.push(c);
            cmds.push(ffi::CmdRec {
                queue: stream as u32,
                tag,
                a,
                b,
            });
        }
    }
    let out = v::lower_plan(target, &kinds, &cmds, &keyed);
    let out = out.as_ref();
    let natives = out.natives();
    let mut queues: Vec<LoweredQueueStream> = plan
        .queues
        .iter()
        .map(|q| LoweredQueueStream {
            queue: q.queue,
            kind: q.kind,
            commands: Vec::new(),
        })
        .collect();
    for rec in out.cmds().iter() {
        let lowered = match commands[rec.source as usize] {
            Command::Wait { queue, value } => LoweredCommand::Wait {
                queue: *queue,
                value: *value,
            },
            Command::Execute(pass) => LoweredCommand::Execute(*pass),
            Command::Signal { value } => LoweredCommand::Signal { value: *value },
            Command::Barriers(_) => LoweredCommand::Barriers(
                (rec.start..rec.start + rec.count)
                    .filter_map(|i| {
                        let n = natives.get(i as usize).expect("lowered barrier");
                        native(flat[n.source as usize], n.native)
                    })
                    .collect(),
            ),
        };
        queues[rec.stream as usize].commands.push(lowered);
    }
    LoweredPlan {
        target,
        order: plan.order.clone(),
        queues,
    }
}

/// Convert Ignis/D3D12 [`Sync`] scope bits to `VkPipelineStageFlags2` bits.
pub fn vk_stage_bits(sync: Sync) -> u64 {
    v::vk_stage_bits(sync.0)
}

/// Convert Ignis/D3D12 [`Sync`] scope bits to `VkPipelineStageFlags2` bits,
/// returning `Err(unmapped)` if any unknown sync bit is present.
pub fn try_vk_stage_bits(sync: Sync) -> Result<u64, Sync> {
    match v::try_vk_stage_bits(sync.0) {
        ffi::StageBits::Mapped(bits) => Ok(bits),
        ffi::StageBits::Unmapped(rest) => Err(Sync(rest)),
    }
}

/// Convert Ignis/D3D12 [`Access`] bits to `VkAccessFlags2` bits.
pub fn vk_access_bits(acc: Access) -> u64 {
    v::vk_access_bits(acc.0)
}

/// Convert Ignis/D3D12 [`Access`] bits to `VkAccessFlags2` bits for a
/// specific [`Sync`] scope.
pub fn vk_access_bits_for_sync(acc: Access, sync: Sync) -> u64 {
    v::vk_access_bits_for_sync(acc.0, sync.0)
}

/// Convert Ignis/D3D12 [`Access`] bits to `VkAccessFlags2` bits, returning
/// `Err(unmapped)` if any unsupported or unknown access bit is present.
pub fn try_vk_access_bits(acc: Access) -> Result<u64, Access> {
    match v::try_vk_access_bits(acc.0) {
        ffi::AccessBits::Mapped(bits) => Ok(bits),
        ffi::AccessBits::Unmapped(rest) => Err(Access(rest)),
    }
}

/// Convert Ignis/D3D12 [`Sync`] scope bits to native Apple Metal 4
/// `MTLStages` bits.
pub fn mtl4_stage_bits(sync: Sync) -> u32 {
    v::mtl4_stage_bits(sync.0)
}
