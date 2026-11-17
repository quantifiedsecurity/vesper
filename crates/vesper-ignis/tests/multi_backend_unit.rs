//! Unit tests moved out of `src/multi_backend.rs` so that both builds run
//! them through the public API.

use vesper_ignis::multi_backend::*;
use vesper_ignis::*;

#[test]
fn metal4_elides_layout_only_transition_but_keeps_hazard() {
    let mut g = RenderGraph::new();
    let direct = g.add_queue(QueueKind::Direct);
    let tex = g
        .add_texture(TextureDesc::simple("color", Layout::Undefined))
        .unwrap();
    g.add_pass(PassDesc::new("draw", direct).texture(
        tex,
        SubresourceRange::ALL,
        Access::RENDER_TARGET,
        Sync::RENDER_TARGET,
        Layout::RenderTarget,
    ))
    .unwrap();
    g.add_pass(PassDesc::new("post", direct).texture(
        tex,
        SubresourceRange::ALL,
        Access::SHADER_RESOURCE,
        Sync::PIXEL_SHADING,
        Layout::ShaderResource,
    ))
    .unwrap();
    let plan = g.plan().unwrap();
    assert_eq!(plan.barrier_count(), 2);

    let d3d = lower_plan(GpuApiTarget::D3D12Agility1620, &plan);
    assert_eq!(d3d.barrier_count(), 2);

    let mtl = lower_plan(GpuApiTarget::Metal4, &plan);
    // Undefined -> RenderTarget layout-only transition is elided; only the RAW barrier remains.
    assert_eq!(mtl.barrier_count(), 1);
    let emitted = mtl.queues[0]
        .commands
        .iter()
        .find_map(|c| match c {
            LoweredCommand::Barriers(bs) => bs.first().copied(),
            _ => None,
        })
        .expect("RAW barrier emitted for Metal4");
    // Native MTLStageFragment = 1 << 1 for both RENDER_TARGET and PIXEL_SHADING.
    assert_eq!(
        emitted,
        NativeBackendBarrier::Metal4StageBarrier {
            after_stages_mask: 1 << 1,
            before_stages_mask: 1 << 1,
        }
    );
}

#[test]
fn vulkan14_maps_texture_barrier_bits() {
    let mut g = RenderGraph::new();
    let direct = g.add_queue(QueueKind::Direct);
    let tex = g
        .add_texture(TextureDesc::simple("rt", Layout::RenderTarget))
        .unwrap();
    let barrier = Barrier::Texture(TextureBarrier {
        resource: tex,
        sync_before: Sync::COMPUTE_SHADING,
        sync_after: Sync::PIXEL_SHADING,
        access_before: Access::UNORDERED_ACCESS,
        access_after: Access::SHADER_RESOURCE,
        layout_before: Layout::UnorderedAccess,
        layout_after: Layout::ShaderResource,
        subresources: SubresourceRange::ALL,
    });
    let vk = lower_to_target_barrier(GpuApiTarget::Vulkan14, &barrier, 0).unwrap();
    assert_eq!(
        vk,
        NativeBackendBarrier::Vulkan14ImageBarrier2 {
            resource: tex,
            subresources: SubresourceRange::ALL,
            src_stage_mask: 1 << 11,
            dst_stage_mask: 1 << 7,
            src_access_mask: (1 << 5) | (1 << 6),
            dst_access_mask: 1 << 5,
            old_layout: Layout::UnorderedAccess,
            new_layout: Layout::ShaderResource,
        }
    );
    // Keep `g` live without unused warning.
    let _ = direct;
}
