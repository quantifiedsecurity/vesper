//! Multi-backend barrier target emitter for Vesper Ignis (§38.11).
//!
//! Lowers [`vesper_ignis`](crate) D3D12-vocabulary barriers (`Barrier`,
//! `TextureBarrier`, `BufferBarrier`, and full `Plan` command streams) into
//! target-native descriptors for:
//! - [`GpuApiTarget::D3D12Agility1620`] (direct enhanced barrier pass-through)
//! - [`GpuApiTarget::Vulkan14`] (`VkImageMemoryBarrier2` /
//!   `VkBufferMemoryBarrier2` and split `VkEvent` signal/wait handles)
//! - [`GpuApiTarget::Metal4`] (`MTL4` stage-to-stage barriers, automatically
//!   eliding layout-only initial transitions in unified TBDR memory)

use crate::d3d12::{Layout, QueueKind};
use crate::graph::{BufferHandle, PassId, QueueId, ResourceRef, SubresourceRange, TextureHandle};
use crate::plan::Barrier;

pub use crate::engine::multi_backend::{
    lower_plan, lower_to_target_barrier, lower_to_target_barrier_for_queue, mtl4_stage_bits,
    try_vk_access_bits, try_vk_stage_bits, vk_access_bits, vk_access_bits_for_sync, vk_stage_bits,
};

/// Supported GPU backend targets for barrier lowering.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum GpuApiTarget {
    D3D12Agility1620,
    Vulkan14,
    Metal4,
}

/// Target-specific barrier representation emitted from the Ignis planner IR.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum NativeBackendBarrier {
    /// Native D3D12 Agility SDK 1.620 enhanced barrier.
    D3D12(Barrier),
    /// Vulkan 1.4 synchronization2 buffer memory barrier.
    Vulkan14BufferBarrier2 {
        resource: BufferHandle,
        src_stage_mask: u64,
        dst_stage_mask: u64,
        src_access_mask: u64,
        dst_access_mask: u64,
    },
    /// Vulkan 1.4 synchronization2 image memory barrier.
    Vulkan14ImageBarrier2 {
        resource: TextureHandle,
        subresources: SubresourceRange,
        src_stage_mask: u64,
        dst_stage_mask: u64,
        src_access_mask: u64,
        dst_access_mask: u64,
        old_layout: Layout,
        new_layout: Layout,
    },
    /// Begin half of a Vulkan 1.4 split barrier (`vkCmdSetEvent2`).
    Vulkan14SplitEventSignal {
        event_id: u32,
        resource: ResourceRef,
        src_stage_mask: u64,
    },
    /// End half of a Vulkan 1.4 split barrier (`vkCmdWaitEvents2`).
    Vulkan14SplitEventWait {
        event_id: u32,
        resource: ResourceRef,
        dst_stage_mask: u64,
        src_access_mask: u64,
        dst_access_mask: u64,
        old_layout: Option<Layout>,
        new_layout: Option<Layout>,
        subresources: Option<SubresourceRange>,
    },
    /// Apple Silicon Metal 4 command-encoder stage barrier (`after` -> `before`).
    Metal4StageBarrier {
        after_stages_mask: u32,
        before_stages_mask: u32,
    },
}

/// One command step lowered for a specific [`GpuApiTarget`].
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum LoweredCommand {
    Wait { queue: QueueId, value: u64 },
    Barriers(Vec<NativeBackendBarrier>),
    Execute(PassId),
    Signal { value: u64 },
}

/// One per-queue command stream lowered for a specific [`GpuApiTarget`].
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LoweredQueueStream {
    pub queue: QueueId,
    pub kind: QueueKind,
    pub commands: Vec<LoweredCommand>,
}

/// A complete [`Plan`] lowered to target-specific barriers.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LoweredPlan {
    pub target: GpuApiTarget,
    pub order: Vec<PassId>,
    pub queues: Vec<LoweredQueueStream>,
}

impl LoweredPlan {
    /// Total count of emitted native barriers across all queues.
    pub fn barrier_count(&self) -> usize {
        self.queues
            .iter()
            .flat_map(|q| &q.commands)
            .map(|c| match c {
                LoweredCommand::Barriers(bs) => bs.len(),
                _ => 0,
            })
            .sum()
    }
}
