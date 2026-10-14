use super::*;
use germinal_ports::rendering::command_palette::{
    COMMAND_PALETTE_TARGET_ID, CommandPaletteSnapshot,
};

#[derive(Default)]
pub(super) struct CommandPaletteViewport {
    first: usize,
}

impl CommandPaletteViewport {
    fn reveal(&mut self, selected: usize, count: usize, visible: usize) -> usize {
        let selected = selected.min(count.saturating_sub(1));
        self.first = self.first.min(count.saturating_sub(visible));
        if selected < self.first {
            self.first = selected;
        } else if selected >= self.first + visible {
            self.first = selected.saturating_sub(visible - 1);
        }
        self.first
    }
}

fn fit(text: &str, width: u32) -> String {
    let mut result = String::new();
    let mut used = 0;
    for ch in text.chars().filter(|ch| !ch.is_control()) {
        let cells = terminal_char_cell_width(ch);
        if used + cells > width {
            break;
        }
        result.push(ch);
        used += cells;
    }
    result.push_str(&" ".repeat(width.saturating_sub(used) as usize));
    result
}

pub(super) fn query_line(palette: &CommandPaletteSnapshot, columns: u32) -> (String, u32) {
    let before_cursor: String = palette
        .preedit
        .char_indices()
        .take_while(|(byte, _)| *byte < palette.preedit_cursor.unwrap_or(palette.preedit.len()))
        .map(|(_, ch)| ch)
        .collect();
    let prefix = format!("{}{}", palette.query, before_cursor);
    let available = columns.saturating_sub(11);
    let mut tail = Vec::new();
    let mut width = 0;
    for ch in prefix.chars().rev() {
        let cells = terminal_char_cell_width(ch);
        if width + cells > available {
            break;
        }
        tail.push(ch);
        width += cells;
    }
    let text: String = tail.into_iter().rev().collect();
    let after: String = palette
        .preedit
        .chars()
        .skip(before_cursor.chars().count())
        .collect();
    (format!("{text}▏{after}"), width + 2)
}

pub(super) fn build(
    palette: &CommandPaletteSnapshot,
    size: TerminalSizeInfo,
    theme: TerminalColorTheme,
    generation: u64,
    viewport: &mut CommandPaletteViewport,
) -> Option<WgpuTabBarSurface> {
    let cell = size.cell_size();
    let window = size.window_size();
    let columns = (window.width_px() / cell.width_px().max(1))
        .saturating_sub(2)
        .min(76);
    let max_rows = (window.height_px() / cell.height_px().max(1))
        .saturating_sub(2)
        .min(28);
    if columns < 24 || max_rows < 11 {
        return None;
    }
    let visible = ((max_rows - 9) / 2) as usize;
    let start = viewport.reveal(palette.selected, palette.entries.len(), visible);
    let count = palette
        .entries
        .len()
        .saturating_sub(start)
        .min(visible)
        .max(1);
    let height = count as u32 * 2 + 9;
    let background = mix_rgb(theme.background, theme.foreground, 8, 100);
    let muted = mix_rgb(background, theme.foreground, 60, 100);
    let mut rows = Vec::new();
    let mut add = |y, x, text: String, foreground, bold, shortcut: Option<(u32, String)>| {
        let mut runs = vec![RenderSurfaceRunSnapshot {
            x,
            text,
            style: TextStyleDto {
                foreground: Some(foreground),
                background: None,
                bold,
                ..TextStyleDto::plain()
            },
            decoration: Default::default(),
        }];
        if let Some((x, text)) = shortcut {
            runs.push(RenderSurfaceRunSnapshot {
                x,
                text,
                style: TextStyleDto {
                    foreground: Some(muted),
                    background: None,
                    ..TextStyleDto::plain()
                },
                decoration: Default::default(),
            });
        }
        rows.push(RenderSurfaceRowSnapshot { y, runs });
    };
    let query = if palette.query.is_empty() && palette.preedit.is_empty() {
        "▏Search commands…".into()
    } else {
        query_line(palette, columns).0
    };
    add(
        1,
        2,
        fit(&query, columns.saturating_sub(10)),
        theme.foreground,
        false,
        Some((columns - 6, "esc".into())),
    );
    add(4, 2, "Commands".into(), muted, false, None);
    if palette.entries.is_empty() {
        add(
            6,
            3,
            fit("No matching commands", columns - 6),
            muted,
            false,
            None,
        );
    } else {
        for (index, entry) in palette.entries.iter().enumerate().skip(start).take(visible) {
            let selected = index == palette.selected;
            let shortcut_width = terminal_text_cell_width(&entry.shortcut).min(columns / 2);
            let title_width = columns.saturating_sub(shortcut_width + 8);
            add(
                6 + (index - start) as u32 * 2,
                3,
                fit(&entry.title, title_width),
                theme.foreground,
                selected,
                Some((
                    columns - shortcut_width - 3,
                    fit(&entry.shortcut, shortcut_width),
                )),
            );
        }
    }
    add(
        height - 2,
        2,
        fit("Ctrl+J/K · Ctrl+N/P  Navigate", columns.saturating_sub(17)),
        muted,
        false,
        Some((columns - 13, "Enter Run".into())),
    );
    let width_px = columns * cell.width_px();
    let height_px = height * cell.height_px();
    Some(WgpuTabBarSurface {
        placement: RenderSurfacePlacement::new(
            COMMAND_PALETTE_TARGET_ID,
            (window.width_px() - width_px) / 2,
            (window.height_px().saturating_sub(height_px) / 5).min(cell.height_px() * 3),
            width_px,
            height_px,
        ),
        snapshot: RenderSurfaceSnapshot {
            target_id: COMMAND_PALETTE_TARGET_ID,
            latest_seq: Seq::new(generation),
            default_background: background,
            rows,
            dirty_rows: Vec::new(),
            image_surfaces: Vec::new(),
            scrollback: None,
            cursor: None,
            ime_preedit: None,
        },
        cell_width_px: cell.width_px(),
        hit_regions: Vec::new(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use germinal_ports::rendering::command_palette::CommandPaletteEntry;

    #[test]
    #[ignore = "requires a GPU adapter and system fonts; renders only an offscreen fixture"]
    fn palette_offscreen_preview() {
        use crate::rendering::pty_surface::{
            frame_renderer::{WgpuTerminalGpuContext, WgpuTerminalRenderView},
            palette_chrome::PaletteChrome,
        };
        let instance = wgpu::Instance::new(wgpu::InstanceDescriptor {
            backends: wgpu::Backends::VULKAN,
            ..wgpu::InstanceDescriptor::new_without_display_handle()
        });
        let adapter = pollster::block_on(instance.request_adapter(&Default::default())).unwrap();
        let (device, queue) =
            pollster::block_on(adapter.request_device(&Default::default())).unwrap();
        let format = wgpu::TextureFormat::Rgba8UnormSrgb;
        let width = 1024;
        let height = 768;
        let texture = device.create_texture(&wgpu::TextureDescriptor {
            label: Some("palette preview"),
            size: wgpu::Extent3d {
                width,
                height,
                depth_or_array_layers: 1,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format,
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::COPY_SRC,
            view_formats: &[],
        });
        let view = texture.create_view(&Default::default());
        let theme = TerminalColorTheme {
            background: RgbColorDto::new(26, 27, 38),
            foreground: RgbColorDto::new(192, 202, 245),
            ..Default::default()
        };
        let size = TerminalSizeInfo::zero_padding(
            TerminalWindowSize::new(width, height),
            TerminalCellSize::new(10, 22),
        );
        let palette = CommandPaletteSnapshot {
            query: String::new(),
            preedit: String::new(),
            preedit_cursor: None,
            selected: 1,
            entries: [
                ("New tab", "Ctrl+Shift+T"),
                ("New window", "Ctrl+Shift+N"),
                ("Split horizontally", "Ctrl+Shift+D"),
                ("Split vertically", "Ctrl+Shift+Alt+D"),
                ("Search terminal", "Ctrl+Shift+F"),
                ("Toggle Vi mode", "Ctrl+Shift+Space"),
                ("Copy selection", "Ctrl+Shift+C"),
                ("Paste clipboard", "Ctrl+Shift+V"),
            ]
            .into_iter()
            .map(|(title, shortcut)| CommandPaletteEntry {
                title: title.into(),
                shortcut: shortcut.into(),
            })
            .collect(),
        };
        let built = build(
            &palette,
            size,
            theme,
            1,
            &mut CommandPaletteViewport::default(),
        )
        .unwrap();
        let config = WgpuRendererConfig::from(TerminalSizeInfo::zero_padding(
            built.placement.window_size(),
            size.cell_size(),
        ))
        .with_background_opacity(0.0);
        let surface = WgpuTerminalWorkspaceSurface {
            render_target_plan: WgpuTerminalRenderTargetPlan::new(
                built.placement.width_px,
                built.placement.height_px,
            )
            .with_origin(built.placement.x_px, built.placement.y_px)
            .with_load_op(WgpuTerminalLoadOp::Load),
            surface_snapshot: &built.snapshot,
            renderer_config: config,
            hyperlink_highlights: &[],
        };
        let pipeline =
            WgpuTerminalPipelineFactory::new(WgpuTerminalPipelineSpec::new(format)).create(&device);
        let renderer = WgpuTerminalFrameRenderer::new(
            WgpuTerminalFrameBuilder::new(config)
                .with_crossfont_glyph_atlas("monospace", 16.0)
                .unwrap(),
        );
        let mut encoder = device.create_command_encoder(&Default::default());
        {
            let attachments = [Some(wgpu::RenderPassColorAttachment {
                view: &view,
                depth_slice: None,
                resolve_target: None,
                ops: wgpu::Operations {
                    load: wgpu::LoadOp::Clear(wgpu::Color {
                        r: 0.010,
                        g: 0.011,
                        b: 0.019,
                        a: 1.0,
                    }),
                    store: wgpu::StoreOp::Store,
                },
            })];
            let _pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                color_attachments: &attachments,
                ..Default::default()
            });
        }
        PaletteChrome::new(&device, format).encode(
            &device,
            &mut encoder,
            &view,
            [width, height],
            &surface,
        );
        renderer.render_to_view(
            WgpuTerminalGpuContext {
                device: &device,
                queue: &queue,
            },
            WgpuTerminalRenderView {
                command_encoder: &mut encoder,
                target_view: &view,
                render_target_plan: surface.render_target_plan,
                pipeline: &pipeline,
                surface_snapshot: &built.snapshot,
                renderer_config: config,
                damage: None,
                hyperlink_highlights: &[],
            },
        );
        let buffer = device.create_buffer(&wgpu::BufferDescriptor {
            label: None,
            size: u64::from(width * height * 4),
            usage: wgpu::BufferUsages::MAP_READ | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        encoder.copy_texture_to_buffer(
            texture.as_image_copy(),
            wgpu::TexelCopyBufferInfo {
                buffer: &buffer,
                layout: wgpu::TexelCopyBufferLayout {
                    offset: 0,
                    bytes_per_row: Some(width * 4),
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
                sender.send(result).unwrap()
            });
        device
            .poll(wgpu::PollType::Wait {
                submission_index: None,
                timeout: Some(Duration::from_secs(10)),
            })
            .unwrap();
        receiver.recv().unwrap().unwrap();
        let pixels = buffer.slice(..).get_mapped_range().unwrap();
        let pixel = |x: u32, y: u32| pixels[((y * width + x) * 4) as usize];
        assert!(pixel(width / 2, built.placement.y_px + 4) > pixel(0, 0));
        assert!(
            pixel(built.placement.x_px, built.placement.y_px)
                < pixel(width / 2, built.placement.y_px + 4)
        );
        assert_eq!(
            pixel(width / 2, built.placement.y_px + 22),
            pixel(width / 2, built.placement.y_px + 4)
        );
        if let Some(path) = std::env::var_os("GERMINAL_PALETTE_PREVIEW") {
            let mut encoder =
                png::Encoder::new(std::fs::File::create(path).unwrap(), width, height);
            encoder.set_color(png::ColorType::Rgba);
            encoder.set_depth(png::BitDepth::Eight);
            encoder
                .write_header()
                .unwrap()
                .write_image_data(&pixels)
                .unwrap();
        }
    }

    #[test]
    fn reversing_navigation_preserves_scroll_until_selection_crosses_an_edge() {
        let mut viewport = CommandPaletteViewport::default();
        for selected in 0..18 {
            viewport.reveal(selected, 30, 9);
        }
        assert_eq!(viewport.first, 9);
        for selected in (9..17).rev() {
            assert_eq!(viewport.reveal(selected, 30, 9), 9);
        }
        assert_eq!(viewport.reveal(8, 30, 9), 8);
        for selected in 9..17 {
            assert_eq!(viewport.reveal(selected, 30, 9), 8);
        }
        assert_eq!(viewport.reveal(17, 30, 9), 9);
        assert_eq!(viewport.reveal(29, 30, 9), 21);
        assert_eq!(viewport.reveal(0, 30, 9), 0);
        assert_eq!(viewport.reveal(17, 30, 4), 14);
        assert_eq!(viewport.reveal(17, 30, 9), 14);
        assert_eq!(viewport.reveal(0, 0, 9), 0);
    }

    #[test]
    fn selection_stays_visible_and_rows_fit_after_resize_with_wide_text() {
        let palette = CommandPaletteSnapshot {
            query: "分屏".repeat(30),
            preedit: "zhong".into(),
            preedit_cursor: Some(3),
            selected: 23,
            entries: (0..30)
                .map(|index| CommandPaletteEntry {
                    title: format!("Command {index} 中文"),
                    shortcut: "Ctrl+Shift+Alt+Right".into(),
                })
                .collect(),
        };
        for (width, height) in [(1200, 900), (480, 360), (360, 312)] {
            let size = TerminalSizeInfo::zero_padding(
                TerminalWindowSize::new(width, height),
                TerminalCellSize::new(12, 24),
            );
            let surface = build(
                &palette,
                size,
                TerminalColorTheme::default(),
                1,
                &mut CommandPaletteViewport::default(),
            )
            .unwrap();
            assert!(surface.placement.x_px + surface.placement.width_px <= width);
            assert!(surface.placement.y_px + surface.placement.height_px <= height);
            assert!(
                surface
                    .snapshot
                    .rows
                    .iter()
                    .flat_map(|row| &row.runs)
                    .any(|run| run.style.bold && run.text.starts_with("Comm"))
            );
            for row in &surface.snapshot.rows {
                assert!((row.y + 1) * 24 <= surface.placement.height_px);
                for run in &row.runs {
                    assert!(
                        (run.x + terminal_text_cell_width(&run.text)) * 12
                            <= surface.placement.width_px
                    );
                }
            }
            let (query, cursor) = query_line(&palette, surface.placement.width_px / 12);
            assert!(query.contains("zho▏ng"));
            assert!(cursor < surface.placement.width_px / 12 - 1);
        }
    }
}
