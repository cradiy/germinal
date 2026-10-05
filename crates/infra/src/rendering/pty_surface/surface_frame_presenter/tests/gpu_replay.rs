use super::*;
use crate::rendering::pty_surface::{
    frame_builder::WgpuTerminalFrameBuilder, pipeline_factory::WgpuTerminalPipelineFactory,
    pipeline_spec::WgpuTerminalPipelineSpec, render_target_plan::WgpuTerminalLoadOp,
};
use germinal_ports::pty_host::worker_input::{
    TerminalSelectionKind, TerminalSelectionPoint, TerminalSelectionSide,
};

const ANSI_REPLAY: &[(&str, &[u8])] = &[
    (
        "watch initial",
        b"\x1b[?25l\x1b[H\x1b[2JGPU stats\r\nload 27%\r\nmem 246M\x1b[2;1H",
    ),
    ("watch shorter value", b"\x1b[2;6H3%\x1b[K"),
    ("colored row", b"\x1b[3;1H\x1b[41mremoved   \x1b[0m"),
    (
        "replace row background",
        b"\x1b[3;1H\x1b[42madded     \x1b[0m",
    ),
    ("erase colored row", b"\x1b[3;1H\x1b[2K"),
    ("visible cursor", b"\x1b[?25h\x1b[1;4H"),
    ("move cursor horizontally", b"\x1b[1;9H"),
    ("move cursor vertically", b"\x1b[4;9H"),
    ("erase display after cursor", b"\x1b[2;4H\x1b[J"),
    ("erase display before cursor", b"\x1b[1J"),
    ("full clear", b"\x1b[H\x1b[2J"),
    ("scrolling output", b"a\r\nb\r\nc\r\nd\r\ne\r\nf"),
    ("enter alternate screen", b"\x1b[?1049h\x1b[Halternate"),
    ("clear alternate screen", b"\x1b[2J\x1b[Halt"),
    ("leave alternate screen", b"\x1b[?1049l"),
    ("hide cursor", b"\x1b[?25l"),
    ("repaint after clear", b"\x1b[H\x1b[2Jrestored"),
    ("fill before buffer swaps", b"\x1b[H\x1b[2Ja\r\nb\r\nc\r\nd"),
    ("first buffer swap", b"\r\ne"),
    ("second buffer swap", b"\r\nf"),
    ("change default background", b"\x1b]11;#112233\x07"),
    ("scroll after background change", b"\r\ng"),
    ("scroll again after background change", b"\r\nh"),
    (
        "hover repeated link",
        b"\x1b[H\x1b[2Jlink\r\nlink\r\nlink\r\nlink",
    ),
    ("scroll under stationary hover", b"\r\nlink"),
    ("scroll again under stationary hover", b"\r\nlink"),
    (
        "set scroll region",
        b"\x1b[H\x1b[2Jheader\r\nabcdef\r\nghijkl\r\nfooter\x1b[2;3r",
    ),
    ("scroll region up", b"\x1b[S"),
    ("scroll region down", b"\x1b[T"),
    ("reverse index at region top", b"\x1b[2;1H\x1bM"),
    ("write region rows", b"\x1b[2;1H123456\x1b[3;1Habcdef"),
    ("insert line inside region", b"\x1b[2;1H\x1b[L"),
    ("delete line inside region", b"\x1b[M"),
    ("insert characters", b"\x1b[2;3H\x1b[2@"),
    ("delete characters", b"\x1b[2P"),
    ("erase characters", b"\x1b[2X"),
    ("reset scroll region", b"\x1b[r\x1b[4;1H\r\nnew footer"),
    (
        "styled glyphs",
        b"\x1b[H\x1b[2J\x1b[1;3;4mAfijgy\x1b[0m\r\nplain",
    ),
    ("erase styled suffix", b"\x1b[1;3H\x1b[K"),
    ("unicode glyphs", "\x1b[2;1H中😈e\u{301}\x1b[K".as_bytes()),
    ("erase wide glyphs", b"\x1b[2;1Hx\x1b[K"),
];

#[test]
#[ignore = "requires an offscreen Vulkan adapter; no window or compositor is used"]
fn retained_gpu_pixels_match_full_redraw_during_cursor_animation_and_scroll() {
    let font = std::env::var("GERMINAL_GPU_REPLAY_FONT").ok();
    eprintln!("Glyph source: {:?}", font.as_deref().unwrap_or("debug 5x7"));
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
    for format in [
        wgpu::TextureFormat::Rgba8Unorm,
        wgpu::TextureFormat::Rgba8UnormSrgb,
        wgpu::TextureFormat::Bgra8Unorm,
        wgpu::TextureFormat::Bgra8UnormSrgb,
    ] {
        let pipeline =
            WgpuTerminalPipelineFactory::new(WgpuTerminalPipelineSpec::new(format)).create(&device);
        let mut actions = [0_u32; 4];
        let mut compared_frames = 0;
        for (height, origin, grid_rows) in
            [(63, 0, 4), (64, 0, 4), (65, 0, 4), (64, 3, 4), (128, 0, 8)]
        {
            for shape in [
                RenderSurfaceCursorShape::Block,
                RenderSurfaceCursorShape::Underline,
                RenderSurfaceCursorShape::Beam,
                RenderSurfaceCursorShape::HollowBlock,
            ] {
                if grid_rows == 8 && shape != RenderSurfaceCursorShape::Block {
                    continue;
                }
                let mut base = WgpuRendererConfig {
                    content_height_px: height,
                    content_origin_y: origin,
                    grid_rows,
                    ..config()
                };
                let renderer =
                    WgpuTerminalFrameRenderer::new(replay_frame_builder(base, font.as_deref()));
                let target_height = height + 2 * origin;
                let mut cache = WgpuRetainedWorkspaceFrame::new(&device, 80, target_height, format);
                let mut reference =
                    WgpuRetainedWorkspaceFrame::new(&device, 80, target_height, format);
                let mut plan = WgpuTerminalRenderTargetPlan::new(80, target_height)
                    .with_load_op(WgpuTerminalLoadOp::Load);
                let mut current = snapshot(1, &["a", "b", "c", "d"], vec![0, 1, 2, 3]);
                current.cursor = Some(RenderSurfaceCursorSnapshot {
                    x: 0,
                    y: 3,
                    focused: true,
                    shape,
                    blinking: true,
                });
                let store = AlacrittyTerminalStore::with_size(AlacrittyTermSize::new(
                    10,
                    grid_rows as usize,
                ));
                let ansi_frames = if shape == RenderSurfaceCursorShape::Block {
                    ANSI_REPLAY.len()
                } else {
                    0
                };
                let interaction_start = 36 + ansi_frames;
                let interaction_frames = if grid_rows == 8 { 14 } else { 0 };
                let resize_start = interaction_start + interaction_frames;
                let resize_frames = if grid_rows == 8 { 9 } else { 0 };
                let continuous_start = resize_start + resize_frames;
                let continuous_frames = if grid_rows == 8 { 104 } else { 0 };
                let mut animated_position = (0, 0);
                let mut motion_start = animated_position;
                let mut motion_target = animated_position;
                for frame in 0..continuous_start + continuous_frames {
                    let continuous = frame >= continuous_start;
                    let resizing = frame >= resize_start && !continuous;
                    let interacting = frame >= interaction_start && frame < resize_start;
                    let scenario = if continuous {
                        "interrupted animation with output and clear"
                    } else if resizing {
                        "resize then scroll"
                    } else if interacting {
                        if frame - interaction_start < 8 {
                            "selection update"
                        } else {
                            "scrollback update"
                        }
                    } else if frame < 36 {
                        "animated scroll"
                    } else {
                        ANSI_REPLAY[frame - 36].0
                    };
                    if continuous {
                        let tick = frame - continuous_start;
                        if tick < 96 && tick % 4 == 0 {
                            let commands: [&[u8]; 8] = [
                                b"\x1b[?25h\x1b[H\x1b[2Ja\r\nb\r\nc\r\nd\r\ne\r\nf\r\ng\r\nh",
                                b"\r\ncompile 1",
                                b"\x1b[2;3H\x1b[41mdiff\x1b[0m",
                                b"\x1b[H\x1b[2Jwatch\r\n27%",
                                b"\x1b[2;1H3%\x1b[K",
                                b"\x1b[?25l\x1b[H\x1b[2Jhidden",
                                b"\x1b[?25h\x1b[8;1Hbottom\r\nnext",
                                b"\x1b[H\x1b[2Jdone",
                            ];
                            let seq = Seq::new(frame as u64 + 1);
                            store.apply_bytes(current.target_id, seq, commands[(tick / 4) % 8]);
                            current = store.render_surface_snapshot_of(current.target_id).unwrap();
                            store.clear_damage_up_to(current.target_id, seq);
                            motion_start = animated_position;
                            if let Some(cursor) = store.cursor_snapshot(current.target_id) {
                                motion_target = (
                                    base.content_origin_x
                                        + cursor.x * base.content_width_px / base.grid_columns,
                                    base.row_top_px(cursor.y),
                                );
                            }
                        }
                        // Six synthetic animation ticks, interrupted by input every four ticks.
                        // The final eight ticks settle without another PTY update.
                        let progress = if tick < 96 {
                            tick % 4 + 1
                        } else {
                            tick - 92 + 1
                        }
                        .min(6);
                        let interpolate = |from: u32, to: u32| {
                            ((from as i64 * (6 - progress) as i64 + to as i64 * progress as i64)
                                / 6) as u32
                        };
                        animated_position = (
                            interpolate(motion_start.0, motion_target.0),
                            interpolate(motion_start.1, motion_target.1),
                        );
                    } else if resizing {
                        let step = frame - resize_start;
                        let seq = Seq::new(frame as u64 + 1);
                        if step % 3 == 0 {
                            let (width, height, columns, rows) =
                                [(64, 96, 8, 6), (96, 144, 12, 9), (80, 128, 10, 8)][step / 3];
                            store.resize(
                                current.target_id,
                                seq,
                                AlacrittyTermSize::new(columns, rows),
                            );
                            base.content_width_px = width;
                            base.content_height_px = height;
                            base.grid_columns = columns as u32;
                            base.grid_rows = rows as u32;
                            plan = WgpuTerminalRenderTargetPlan::new(width, height)
                                .with_load_op(WgpuTerminalLoadOp::Load);
                            // Match the presenter's target recreation, but retain renderer caches.
                            if !cache.matches(width, height, format) {
                                cache =
                                    WgpuRetainedWorkspaceFrame::new(&device, width, height, format);
                            }
                            reference =
                                WgpuRetainedWorkspaceFrame::new(&device, width, height, format);
                        } else {
                            let output = if step % 3 == 1 {
                                (0..=base.grid_rows)
                                    .map(|row| format!("\r\nrow {row}"))
                                    .collect::<String>()
                            } else {
                                "\r\nnext".to_string()
                            };
                            store.apply_bytes(current.target_id, seq, output.as_bytes());
                        }
                        current = store.render_surface_snapshot_of(current.target_id).unwrap();
                        store.clear_damage_up_to(current.target_id, seq);
                    } else if interacting {
                        let seq = Seq::new(frame as u64 + 1);
                        replay_interaction_step(
                            &store,
                            current.target_id,
                            seq,
                            frame - interaction_start,
                        );
                        current = store.render_surface_snapshot_of(current.target_id).unwrap();
                        store.clear_damage_up_to(current.target_id, seq);
                    } else if frame >= 36 {
                        let seq = Seq::new(frame as u64 + 1);
                        store.apply_bytes(current.target_id, seq, ANSI_REPLAY[frame - 36].1);
                        current = store.render_surface_snapshot_of(current.target_id).unwrap();
                        store.clear_damage_up_to(current.target_id, seq);
                    } else if frame % 3 == 0 {
                        current.latest_seq = Seq::new(current.latest_seq.value() + 1);
                        current.rows.rotate_left(1);
                        for (y, row) in current.rows.iter_mut().enumerate() {
                            row.y = y as u32;
                        }
                        current.rows[3].runs[0].text = format!("crate {frame}");
                    }
                    if frame >= 36 {
                        // The PTY worker composes cursor state separately from content rows.
                        current.cursor = store.cursor_snapshot(current.target_id);
                    }
                    let y = match frame % 12 {
                        0 => origin,
                        1 => origin + height / 3,
                        2 => origin + height / 2,
                        _ => base.row_top_px(3),
                    };
                    let config = WgpuRendererConfig {
                        cursor_position_px: if continuous {
                            Some(animated_position)
                        } else {
                            (frame < 36).then_some((0, y))
                        },
                        blinking_cursor_visible: frame % 11 != 0,
                        ..base
                    };
                    let highlights = if scenario.contains("hover") {
                        vec![TerminalHyperlink {
                            uri: "https://example.com".to_string(),
                            x: 0,
                            y: if origin == 0 { 2 } else { 1 },
                            columns: 4,
                        }]
                    } else {
                        Vec::new()
                    };
                    let surface = WgpuTerminalWorkspaceSurface {
                        render_target_plan: plan,
                        surface_snapshot: &current,
                        renderer_config: config,
                        hyperlink_highlights: &highlights,
                    };
                    let action = retained_frame_action(cache.previous.as_ref(), &surface);
                    if resizing && (frame - resize_start) % 3 == 2 {
                        assert!(
                            matches!(action, WgpuRetainedFrameAction::ScrollUp { .. }),
                            "post-resize output must exercise scroll reuse: {action:?}"
                        );
                    }
                    if scenario == "write region rows" && grid_rows == 8 {
                        assert!(
                            matches!(action, WgpuRetainedFrameAction::Damage { .. }),
                            "region update must exercise partial repaint: {action:?}"
                        );
                    }
                    if scenario.contains("stationary hover") && height == 64 {
                        if origin == 0 {
                            assert!(
                                matches!(action, WgpuRetainedFrameAction::ScrollUp { .. }),
                                "hover replay must exercise texture scrolling: {action:?}"
                            );
                        } else {
                            assert_eq!(
                                action,
                                WgpuRetainedFrameAction::Full,
                                "fully repainted hover must skip copying"
                            );
                        }
                    }
                    if scenario == "scroll after background change" && height == 64 {
                        assert_eq!(current.default_background, RgbColorDto::new(17, 34, 51));
                        assert!(cache.initialized.iter().all(|initialized| *initialized));
                        assert!(
                            matches!(action, WgpuRetainedFrameAction::ScrollUp { .. }),
                            "background replay must switch initialized textures: {action:?}"
                        );
                    }
                    if scenario == "watch shorter value" {
                        assert!(
                            matches!(action, WgpuRetainedFrameAction::Damage { .. }),
                            "watch update must exercise partial repaint: {action:?}"
                        );
                    }
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
                            plan.with_scissor(0, top, plan.width_px, bottom - top)
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
                                hyperlink_highlights: &highlights,
                            },
                        );
                    }
                    // A fresh renderer prevents the reference from sharing stale row/atlas caches.
                    let full_renderer =
                        WgpuTerminalFrameRenderer::new(replay_frame_builder(base, font.as_deref()));
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
                            hyperlink_highlights: &highlights,
                        },
                    );
                    queue.submit([encoder.finish()]);
                    let actual = read_pixels(&device, &queue, cache.current_texture());
                    let expected = read_pixels(&device, &queue, reference.current_texture());
                    let expected_bytes = plan.width_px as usize * plan.height_px as usize * 4;
                    assert_eq!(actual.len(), expected_bytes, "retained readback size");
                    assert_eq!(expected.len(), expected_bytes, "reference readback size");
                    if let Some(pixel) = actual
                        .as_chunks::<4>()
                        .0
                        .iter()
                        .zip(expected.as_chunks::<4>().0)
                        .position(|(a, b)| a != b)
                    {
                        panic!(
                            "format={format:?} height={height} origin={origin} shape={shape:?} frame={frame} scenario={scenario} action={action:?} pixel=({}, {}) actual={:?} expected={:?}",
                            pixel % plan.width_px as usize,
                            pixel / plan.width_px as usize,
                            &actual[pixel * 4..pixel * 4 + 4],
                            &expected[pixel * 4..pixel * 4 + 4]
                        );
                    }
                    compared_frames += 1;
                    cache.initialized[cache.current] = true;
                    cache.previous = Some(WgpuRetainedWorkspaceSnapshot {
                        snapshot: current.clone(),
                        renderer_config: config,
                        hyperlink_highlights: highlights,
                    });
                }
            }
        }
        assert!(
            actions.iter().all(|count| *count > 0),
            "missing retained path: {actions:?}"
        );
        eprintln!(
            "format={format:?}: compared {compared_frames} GPU frames; Full/Reuse/Damage/ScrollUp={actions:?}"
        );
        replay_workspace_pixels(&device, &queue, format, font.as_deref());
    }
}

fn replay_workspace_pixels(
    device: &wgpu::Device,
    queue: &wgpu::Queue,
    format: wgpu::TextureFormat,
    font: Option<&str>,
) {
    use crate::rendering::pty_surface::render_target_plan::WgpuTerminalClearColor;

    let pipeline =
        WgpuTerminalPipelineFactory::new(WgpuTerminalPipelineSpec::new(format)).create(device);
    let renderer = WgpuTerminalFrameRenderer::new(replay_frame_builder(config(), font));
    let workspace = WgpuRetainedWorkspaceFrame::new(device, 168, 136, format);
    for frame in 0..16 {
        let alpha = [255, 128, 0, 255][frame / 4];
        let base = WgpuRendererConfig {
            background_alpha: alpha,
            ..config()
        };
        let placements: &[(u32, u32, u64)] = match frame % 4 {
            0 => &[(0, 0, 1), (88, 0, 2)],
            1 => &[(0, 0, 2), (0, 72, 1)],
            2 => &[(88, 72, 1)],
            _ => &[(88, 0, 1), (0, 72, 2)],
        };
        let view = workspace.current_texture().create_view(&Default::default());
        let mut encoder = device.create_command_encoder(&Default::default());
        // Transparent and multi-pane presentation clears once, then loads for each pane.
        let clear_color = if alpha == 255 {
            WgpuTerminalClearColor::black()
        } else {
            WgpuTerminalClearColor::transparent()
        };
        clear_target_view(&mut encoder, &view, clear_color);
        let panes = placements
            .iter()
            .map(|&(x, y, id)| {
                let mut pane = snapshot(
                    frame as u64 + 1,
                    if frame % 3 == 0 {
                        &["long text", "second", "third"]
                    } else {
                        &["x"]
                    },
                    vec![0, 1, 2, 3],
                );
                pane.target_id = RenderTargetId::new(id);
                pane.default_background = if id == 1 {
                    RgbColorDto::new(80, 20, 40)
                } else {
                    RgbColorDto::new(10, 70, 90)
                };
                pane.cursor = (frame % 3 != 2).then_some(RenderSurfaceCursorSnapshot {
                    x: id as u32,
                    y: frame as u32 % 3,
                    focused: id == 1,
                    shape: RenderSurfaceCursorShape::Block,
                    blinking: false,
                });
                (x, y, pane)
            })
            .collect::<Vec<_>>();
        for (x, y, pane) in &panes {
            renderer.render_to_view(
                WgpuTerminalGpuContext { device, queue },
                WgpuTerminalRenderView {
                    command_encoder: &mut encoder,
                    target_view: &view,
                    render_target_plan: WgpuTerminalRenderTargetPlan::new(80, 64)
                        .with_origin(*x, *y)
                        .with_load_op(WgpuTerminalLoadOp::Load),
                    pipeline: &pipeline,
                    surface_snapshot: pane,
                    renderer_config: base,
                    damage: None,
                    hyperlink_highlights: &[],
                },
            );
        }
        queue.submit([encoder.finish()]);
        let actual = read_pixels(device, queue, workspace.current_texture());
        let mut expected = vec![0; 168 * 136 * 4];
        if alpha == 255 {
            for pixel in expected.as_chunks_mut::<4>().0 {
                pixel[3] = 255;
            }
        }
        for (x, y, pane) in &panes {
            let isolated = WgpuRetainedWorkspaceFrame::new(device, 80, 64, format);
            let isolated_view = isolated.current_texture().create_view(&Default::default());
            let fresh_renderer = WgpuTerminalFrameRenderer::new(replay_frame_builder(base, font));
            let mut encoder = device.create_command_encoder(&Default::default());
            fresh_renderer.render_to_view(
                WgpuTerminalGpuContext { device, queue },
                WgpuTerminalRenderView {
                    command_encoder: &mut encoder,
                    target_view: &isolated_view,
                    render_target_plan: WgpuTerminalRenderTargetPlan::new(80, 64)
                        .with_clear_color(WgpuTerminalClearColor::transparent()),
                    pipeline: &pipeline,
                    surface_snapshot: pane,
                    renderer_config: base,
                    damage: None,
                    hyperlink_highlights: &[],
                },
            );
            queue.submit([encoder.finish()]);
            let pixels = read_pixels(device, queue, isolated.current_texture());
            // Bottom-right is a blank cell: background alpha must not become opaque.
            assert_eq!(pixels[(63 * 80 + 79) * 4 + 3], alpha);
            for row in 0..64 {
                let start = (((*y as usize + row) * 168) + *x as usize) * 4;
                expected[start..start + 80 * 4]
                    .copy_from_slice(&pixels[row * 80 * 4..(row + 1) * 80 * 4]);
            }
        }
        assert_eq!(actual.len(), expected.len());
        if let Some(byte) = actual.iter().zip(&expected).position(|(a, b)| a != b) {
            panic!(
                "workspace format={format:?} frame={frame} alpha={alpha} pixel=({}, {}) channel={} actual={} expected={}",
                byte / 4 % 168,
                byte / 4 / 168,
                byte % 4,
                actual[byte],
                expected[byte]
            );
        }
    }
    eprintln!("format={format:?}: compared 16 transparent/multi-pane workspace frames");
}

fn replay_interaction_step(
    store: &AlacrittyTerminalStore,
    target: RenderTargetId,
    seq: Seq,
    step: usize,
) {
    use TerminalSelectionSide::{Left, Right};
    use alacritty_terminal::grid::Scroll;
    let point = TerminalSelectionPoint::new;
    match step {
        0 => {
            store.apply_bytes(target, seq, b"\x1b[H\x1b[2Jalpha\r\nbravo\r\ncharlie");
        }
        1 => {
            assert!(store.start_selection(
                target,
                seq,
                TerminalSelectionKind::Character,
                point(0, 0, Left)
            ));
            assert!(store.update_selection(target, seq, point(4, 0, Right)));
            assert_eq!(store.selection_text(target).as_deref(), Some("alpha"));
        }
        2 => {
            assert!(store.update_selection(target, seq, point(4, 1, Right)));
            assert_eq!(
                store.selection_text(target).as_deref(),
                Some("alpha\nbravo")
            );
        }
        3 => {
            assert!(store.update_selection(target, seq, point(2, 0, Right)));
            assert_eq!(store.selection_text(target).as_deref(), Some("alp"));
        }
        4 | 7 => {
            assert!(store.start_selection(
                target,
                seq,
                TerminalSelectionKind::Character,
                point(0, 0, Left)
            ));
            assert_eq!(store.selection_text(target), None);
        }
        5 => {
            assert!(store.start_selection(
                target,
                seq,
                TerminalSelectionKind::Word,
                point(2, 1, Left)
            ));
            assert_eq!(store.selection_text(target).as_deref(), Some("bravo"));
        }
        6 => {
            assert!(store.start_selection(
                target,
                seq,
                TerminalSelectionKind::Line,
                point(2, 2, Left)
            ));
            assert!(store.selection_text(target).unwrap().contains("charlie"));
        }
        8 => {
            let text = (0..20)
                .map(|line| format!("\r\nline {line:02}"))
                .collect::<String>();
            store.apply_bytes(target, seq, format!("\x1b[?25h{text}").as_bytes());
        }
        9 => {
            assert!(store.scroll_display(target, seq, Scroll::Top));
        }
        10 => {
            assert!(store.scroll_display(target, seq, Scroll::Delta(-2)));
        }
        11 | 13 => {
            store.apply_bytes(target, seq, b"\r\nnew line");
        }
        12 => {
            assert!(store.scroll_display(target, seq, Scroll::Bottom));
        }
        _ => unreachable!("interaction replay step"),
    }
    if step >= 9 {
        let snapshot = store.render_surface_snapshot_of(target).unwrap();
        let offset = snapshot.scrollback.unwrap().display_offset;
        if step <= 11 {
            assert!(offset > 0, "history view must remain active at step {step}");
            assert!(store.cursor_snapshot(target).is_none());
        } else {
            assert_eq!(offset, 0);
            assert!(store.cursor_snapshot(target).is_some());
        }
    }
}

fn replay_frame_builder(
    config: WgpuRendererConfig,
    font: Option<&str>,
) -> WgpuTerminalFrameBuilder {
    let builder = WgpuTerminalFrameBuilder::new(config);
    match font {
        Some(family) => builder
            .with_crossfont_glyph_atlas(family, 12.0)
            .expect("system font atlas"),
        None => builder,
    }
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
