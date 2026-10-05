use super::*;
use crate::rendering::pty_surface::{
    frame_builder::WgpuTerminalFrameBuilder, pipeline_factory::WgpuTerminalPipelineFactory,
    pipeline_spec::WgpuTerminalPipelineSpec, render_target_plan::WgpuTerminalLoadOp,
};

#[test]
#[ignore = "requires an offscreen Vulkan adapter; no window or compositor is used"]
fn retained_gpu_pixels_match_full_redraw_during_cursor_animation_and_scroll() {
    let instance = wgpu::Instance::new(wgpu::InstanceDescriptor {
        backends: wgpu::Backends::VULKAN,
        ..wgpu::InstanceDescriptor::new_without_display_handle()
    });
    let adapter = pollster::block_on(instance.request_adapter(&wgpu::RequestAdapterOptions {
        compatible_surface: None,
        ..Default::default()
    }))
    .expect("offscreen Vulkan adapter required");
    eprintln!("Offscreen adapter: {:?}", adapter.get_info());
    let (device, queue) = pollster::block_on(adapter.request_device(&wgpu::DeviceDescriptor {
        label: Some("cursor replay"),
        ..Default::default()
    }))
    .expect("offscreen device");
    let format = wgpu::TextureFormat::Rgba8Unorm;
    let pipeline =
        WgpuTerminalPipelineFactory::new(WgpuTerminalPipelineSpec::new(format)).create(&device);
    let mut actions = [0_u32; 4];
    let mut compared_frames = 0;
    for height in [63, 64, 65] {
        for shape in [
            RenderSurfaceCursorShape::Block,
            RenderSurfaceCursorShape::Underline,
            RenderSurfaceCursorShape::Beam,
            RenderSurfaceCursorShape::HollowBlock,
        ] {
            let base = WgpuRendererConfig {
                content_height_px: height,
                ..config()
            };
            let renderer = WgpuTerminalFrameRenderer::new(WgpuTerminalFrameBuilder::new(base));
            let mut cache = WgpuRetainedWorkspaceFrame::new(&device, 80, height, format);
            let reference = WgpuRetainedWorkspaceFrame::new(&device, 80, height, format);
            let plan = WgpuTerminalRenderTargetPlan::new(80, height)
                .with_load_op(WgpuTerminalLoadOp::Load);
            let mut current = snapshot(1, &["a", "b", "c", "d"], vec![0, 1, 2, 3]);
            current.cursor = Some(RenderSurfaceCursorSnapshot {
                x: 0,
                y: 3,
                focused: true,
                shape,
                blinking: true,
            });
            for frame in 0..36 {
                if frame % 3 == 0 {
                    current.latest_seq = Seq::new(current.latest_seq.value() + 1);
                    current.rows.rotate_left(1);
                    for (y, row) in current.rows.iter_mut().enumerate() {
                        row.y = y as u32;
                    }
                    current.rows[3].runs[0].text = format!("crate {frame}");
                }
                let y = match frame % 12 {
                    0 => 0,
                    1 => height / 3,
                    2 => height / 2,
                    _ => base.row_top_px(3),
                };
                let config = WgpuRendererConfig {
                    cursor_position_px: Some((0, y)),
                    blinking_cursor_visible: frame % 11 != 0,
                    ..base
                };
                let surface = WgpuTerminalWorkspaceSurface {
                    render_target_plan: plan,
                    surface_snapshot: &current,
                    renderer_config: config,
                    hyperlink_highlights: &[],
                };
                let action = retained_frame_action(cache.previous.as_ref(), &surface);
                actions[match action {
                    WgpuRetainedFrameAction::Full => 0,
                    WgpuRetainedFrameAction::Reuse => 1,
                    WgpuRetainedFrameAction::Damage { .. } => 2,
                    WgpuRetainedFrameAction::ScrollUp { .. } => 3,
                }] += 1;
                let mut encoder = device.create_command_encoder(&Default::default());
                prepare_retained_target(&mut encoder, &mut cache, action, &surface);
                let view = cache.current_texture().create_view(&Default::default());
                if action == WgpuRetainedFrameAction::Full {
                    clear_target_view(&mut encoder, &view, plan.clear_color);
                }
                if action != WgpuRetainedFrameAction::Reuse {
                    let damage = action.renderer_damage();
                    let render_target_plan = damage.map_or(plan, |damage| {
                        let (top, bottom) = damage.vertical_bounds();
                        plan.with_scissor(0, top, 80, bottom - top)
                    });
                    renderer.render_to_view(
                        WgpuTerminalGpuContext {
                            device: &device,
                            queue: &queue,
                        },
                        WgpuTerminalRenderView {
                            command_encoder: &mut encoder,
                            target_view: &view,
                            render_target_plan,
                            pipeline: &pipeline,
                            surface_snapshot: &current,
                            renderer_config: config,
                            damage,
                            hyperlink_highlights: &[],
                        },
                    );
                }
                // A fresh renderer prevents the reference from sharing stale row/atlas caches.
                let full_renderer =
                    WgpuTerminalFrameRenderer::new(WgpuTerminalFrameBuilder::new(base));
                let full_view = reference.current_texture().create_view(&Default::default());
                clear_target_view(&mut encoder, &full_view, plan.clear_color);
                full_renderer.render_to_view(
                    WgpuTerminalGpuContext {
                        device: &device,
                        queue: &queue,
                    },
                    WgpuTerminalRenderView {
                        command_encoder: &mut encoder,
                        target_view: &full_view,
                        render_target_plan: plan,
                        pipeline: &pipeline,
                        surface_snapshot: &current,
                        renderer_config: config,
                        damage: None,
                        hyperlink_highlights: &[],
                    },
                );
                queue.submit([encoder.finish()]);
                let actual = read_pixels(&device, &queue, cache.current_texture());
                let expected = read_pixels(&device, &queue, reference.current_texture());
                if let Some(pixel) = actual
                    .as_chunks::<4>()
                    .0
                    .iter()
                    .zip(expected.as_chunks::<4>().0)
                    .position(|(a, b)| a != b)
                {
                    panic!(
                        "height={height} shape={shape:?} frame={frame} action={action:?} pixel=({}, {}) actual={:?} expected={:?}",
                        pixel % 80,
                        pixel / 80,
                        &actual[pixel * 4..pixel * 4 + 4],
                        &expected[pixel * 4..pixel * 4 + 4]
                    );
                }
                compared_frames += 1;
                cache.initialized[cache.current] = true;
                cache.previous = Some(WgpuRetainedWorkspaceSnapshot {
                    snapshot: current.clone(),
                    renderer_config: config,
                    hyperlink_highlights: Vec::new(),
                });
            }
        }
    }
    assert!(
        actions.iter().all(|count| *count > 0),
        "missing retained path: {actions:?}"
    );
    eprintln!("Compared {compared_frames} GPU frames; Full/Reuse/Damage/ScrollUp={actions:?}");
}

fn read_pixels(device: &wgpu::Device, queue: &wgpu::Queue, texture: &wgpu::Texture) -> Vec<u8> {
    let row_bytes = texture.width() * 4;
    let stride =
        row_bytes.div_ceil(wgpu::COPY_BYTES_PER_ROW_ALIGNMENT) * wgpu::COPY_BYTES_PER_ROW_ALIGNMENT;
    let buffer = device.create_buffer(&wgpu::BufferDescriptor {
        label: Some("cursor replay readback"),
        size: u64::from(stride) * u64::from(texture.height()),
        usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
        mapped_at_creation: false,
    });
    let mut encoder = device.create_command_encoder(&Default::default());
    encoder.copy_texture_to_buffer(
        texture.as_image_copy(),
        wgpu::TexelCopyBufferInfo {
            buffer: &buffer,
            layout: wgpu::TexelCopyBufferLayout {
                offset: 0,
                bytes_per_row: Some(stride),
                rows_per_image: None,
            },
        },
        texture.size(),
    );
    queue.submit([encoder.finish()]);
    let (sender, receiver) = std::sync::mpsc::channel();
    buffer
        .slice(..)
        .map_async(wgpu::MapMode::Read, move |result| {
            sender.send(result).unwrap();
        });
    device
        .poll(wgpu::PollType::Wait {
            submission_index: None,
            timeout: Some(Duration::from_secs(10)),
        })
        .expect("GPU readback poll");
    receiver
        .recv_timeout(Duration::from_secs(10))
        .expect("map callback")
        .expect("map readback");
    let mapped = buffer.slice(..).get_mapped_range().expect("mapped pixels");
    let pixels = mapped
        .chunks_exact(stride as usize)
        .flat_map(|row| row[..row_bytes as usize].iter().copied())
        .collect();
    drop(mapped);
    buffer.unmap();
    pixels
}
