use super::*;
use std::{fs, io::Write};

#[derive(Default)]
pub(super) struct SidebarResize {
    state: UiState,
    path: Option<PathBuf>,
    drag: Option<Drag>,
    last_click: Option<(Instant, PhysicalPosition<f64>)>,
}

#[cfg(all(test, target_os = "linux"))]
mod tests {
    use super::*;
    use winit::platform::wayland::EventLoopBuilderExtWayland;

    #[test]
    #[ignore = "opens an isolated Wayland window to check resize capture and persistence"]
    fn sidebar_drag_restores_width_without_capturing_terminal_input() {
        struct Probe {
            app: App,
            path: PathBuf,
            deadline: Instant,
        }
        impl ApplicationHandler<RuntimeEvent> for Probe {
            fn resumed(&mut self, event_loop: &ActiveEventLoop) {
                self.app.resumed(event_loop);
                let size = self.app.current_terminal_size_info().window_size();
                let original = self.app.sidebar_width(size.width_px());
                let id = self.app.current_window_id().unwrap();
                let device_id = winit::event::DeviceId::dummy();
                let move_to = |x| WindowEvent::CursorMoved {
                    device_id,
                    position: PhysicalPosition::new(x, 150.0),
                };
                let button = |state| WindowEvent::MouseInput {
                    device_id,
                    state,
                    button: MouseButton::Left,
                };
                self.app
                    .window_event(event_loop, id, move_to(f64::from(original)));
                self.app
                    .window_event(event_loop, id, button(ElementState::Pressed));
                self.app
                    .window_event(event_loop, id, move_to(f64::from(original + 96)));
                assert!(self.app.sidebar_resize.is_dragging());
                assert!(self.app.pointer_capture.is_none());
                assert!(self.app.pointer_gshell.is_none());
                assert!(!self.path.exists(), "do not save on every pointer move");
                self.app
                    .window_event(event_loop, id, button(ElementState::Released));
                assert!(!self.app.sidebar_resize.is_dragging());
                let width = self.app.sidebar_width(size.width_px());
                assert_eq!(width, original + 96);
                assert_eq!(
                    self.app.current_workspace_render_layout(size)[0].x_px,
                    width
                );
                let runtime = self.app.render_runtime.as_ref().unwrap();
                assert_eq!(
                    runtime.tab_index_at_position(f64::from(width + 1), 150.0),
                    None
                );
                self.app.sidebar_resize = SidebarResize::default();
                self.app.restore_ui_state(self.path.clone());
                assert_eq!(self.app.sidebar_width(size.width_px()), width);
                assert!(self.app.sidebar_width(200) < 200);
                assert_eq!(self.app.sidebar_width(size.width_px()), width);
                // A double click must clear the saved override, not only the current layout.
                for _ in 0..2 {
                    self.app
                        .window_event(event_loop, id, button(ElementState::Pressed));
                    self.app
                        .window_event(event_loop, id, button(ElementState::Released));
                }
                self.app.restore_ui_state(self.path.clone());
                assert_eq!(self.app.sidebar_width(size.width_px()), original);
            }

            fn window_event(
                &mut self,
                event_loop: &ActiveEventLoop,
                id: WindowId,
                event: WindowEvent,
            ) {
                self.app.window_event(event_loop, id, event);
            }
            fn user_event(&mut self, event_loop: &ActiveEventLoop, event: RuntimeEvent) {
                self.app.user_event(event_loop, event);
            }
            fn about_to_wait(&mut self, event_loop: &ActiveEventLoop) {
                self.app.about_to_wait(event_loop);
                if Instant::now() >= self.deadline {
                    event_loop.exit();
                } else {
                    event_loop.set_control_flow(ControlFlow::WaitUntil(self.deadline));
                }
            }
        }
        let event_loop = EventLoop::<RuntimeEvent>::with_user_event()
            .with_wayland()
            .with_any_thread(true)
            .build()
            .unwrap();
        let mut config = GerminalConfig::default();
        config.window.modern = true;
        config.window.title = "Germinal sidebar resize check".into();
        config.window.width_px = 960;
        config.window.height_px = 640;
        config.terminal.shell = Some(config::ShellConfig {
            program: "/bin/sleep".into(),
            args: vec!["10".into()],
        });
        let path = std::env::temp_dir().join(format!(
            "germinal-sidebar-resize-{}.toml",
            std::process::id()
        ));
        let mut app = App::new(event_loop.create_proxy(), config).unwrap();
        app.restore_ui_state(path.clone());
        event_loop
            .run_app(&mut Probe {
                app,
                path: path.clone(),
                deadline: Instant::now() + Duration::from_secs(3),
            })
            .unwrap();
        fs::remove_file(path).unwrap();
    }
}

#[derive(Default, Clone, serde::Serialize, serde::Deserialize)]
#[serde(default)]
struct UiState {
    #[cfg(windows)]
    #[serde(skip_serializing_if = "Option::is_none")]
    window: Option<germinal_infra::rendering::pty_surface::window_runtime::SavedWindowPlacement>,
    #[serde(skip_serializing_if = "Option::is_none")]
    sidebar_width: Option<f64>,
}

struct Drag {
    x: f64,
    width: f64,
    moved: bool,
}

impl SidebarResize {
    pub(super) fn is_dragging(&self) -> bool {
        self.drag.is_some()
    }

    fn save(&self) {
        let Some(path) = &self.path else { return };
        if let Err(error) = self.state.save(path) {
            warn!(%error, path = %path.display(), "failed to save sidebar width");
        }
    }
}

impl UiState {
    fn save(&self, path: &std::path::Path) -> std::io::Result<()> {
        let contents = toml::to_string(self).map_err(std::io::Error::other)?;
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent)?;
        }
        let nonce = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_err(std::io::Error::other)?
            .as_nanos();
        let temporary = path.with_extension(format!("{}.{nonce}.tmp", std::process::id()));
        let mut file = fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&temporary)?;
        let result = (|| {
            file.write_all(contents.as_bytes())?;
            file.sync_all()?;
            fs::rename(&temporary, path)
        })();
        if result.is_err() {
            let _ = fs::remove_file(&temporary);
        }
        result
    }
}

impl App {
    #[cfg(windows)]
    pub(super) fn restore_window_placement(&mut self, window: &winit::window::Window) -> bool {
        let Some(state) = &self.sidebar_resize.state.window else {
            return false;
        };
        if !state.restore_hidden(window) {
            return false;
        }
        self.config.window.maximized |= state.maximized;
        true
    }

    #[cfg(windows)]
    pub(super) fn save_window_placement(&self) {
        let Some(path) = &self.sidebar_resize.path else {
            return;
        };
        let Some(window) = self
            .render_runtime
            .as_ref()
            .and_then(|runtime| runtime.saved_window_placement())
        else {
            return;
        };
        let mut state = self.sidebar_resize.state.clone();
        state.window = Some(window);
        if let Err(error) = state.save(path) {
            warn!(%error, "failed to save window placement");
        }
    }
    pub fn restore_ui_state(&mut self, path: PathBuf) {
        match fs::read_to_string(&path) {
            Ok(contents) => match toml::from_str::<UiState>(&contents) {
                Ok(mut state) => {
                    state.sidebar_width = state
                        .sidebar_width
                        .filter(|width| width.is_finite() && *width > 0.0);
                    self.sidebar_resize.state = state;
                }
                Err(error) => warn!(%error, path = %path.display(), "invalid UI state"),
            },
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) => warn!(%error, path = %path.display(), "failed to read UI state"),
        }
        self.sidebar_resize.path = Some(path);
    }

    fn sidebar_scale(&self) -> f64 {
        self.render_runtime
            .as_ref()
            .map_or(1.0, |runtime| runtime.display_scale_factor().value())
    }

    pub(super) fn sidebar_width(&self, window_width: u32) -> u32 {
        match self.sidebar_resize.state.sidebar_width {
            Some(width) => self.clamp_sidebar_width(width * self.sidebar_scale(), window_width),
            None => germinal_ports::rendering::tab_bar::sidebar_width_px(
                window_width,
                self.current_terminal_size_info().cell_size().height_px(),
            ),
        }
    }

    fn clamp_sidebar_width(&self, width: f64, window_width: u32) -> u32 {
        let maximum = window_width
            .saturating_sub(1)
            .min(window_width.saturating_mul(2) / 3);
        let minimum = ((160.0 * self.sidebar_scale()).round() as u32).min(maximum);
        (width.round().max(0.0) as u32).clamp(minimum, maximum)
    }

    pub(super) fn sidebar_edge_at_cursor(&self) -> bool {
        if self.layout_tab_position() != TabBarPosition::Left || self.pointer_capture.is_some() {
            return false;
        }
        let Some(position) = self.cursor_position else {
            return false;
        };
        let size = self.current_terminal_size_info().window_size();
        let width = self.sidebar_width(size.width_px());
        width > 0
            && position.y >= 0.0
            && position.y < f64::from(size.height_px())
            && (position.x - f64::from(width)).abs() <= 4.0 * self.sidebar_scale()
    }

    pub(super) fn handle_sidebar_resize_button(&mut self, state: ElementState) -> bool {
        if state == ElementState::Released {
            return self.finish_sidebar_resize();
        }
        if !self.sidebar_edge_at_cursor() {
            return false;
        }
        let position = self.cursor_position.unwrap();
        let now = Instant::now();
        let double_click = self
            .sidebar_resize
            .last_click
            .take()
            .is_some_and(|(time, last)| {
                now.duration_since(time) <= Duration::from_millis(400)
                    && (position.x - last.x).abs() <= 4.0 * self.sidebar_scale()
                    && (position.y - last.y).abs() <= 4.0 * self.sidebar_scale()
            });
        if double_click {
            self.sidebar_resize.state.sidebar_width = None;
            self.sidebar_resize.save();
            self.refresh_sidebar_layout();
        } else {
            self.sidebar_resize.last_click = Some((now, position));
        }
        self.sidebar_resize.drag = Some(Drag {
            x: position.x,
            width: f64::from(
                self.sidebar_width(self.current_terminal_size_info().window_size().width_px()),
            ),
            moved: false,
        });
        self.pending_hyperlink_activation = None;
        self.route_pointer_left();
        if let Some(runtime) = self.render_runtime.as_mut() {
            runtime.set_hovered_terminal_hyperlinks(None);
            runtime.set_sidebar_resize_cursor(true);
            runtime.hover_tab_sidebar(None);
            runtime.hover_toolbar(None);
        }
        true
    }

    pub(super) fn move_sidebar_resize(&mut self, position: PhysicalPosition<f64>) -> bool {
        let Some(drag) = &self.sidebar_resize.drag else {
            return false;
        };
        if !position.x.is_finite() || (position.x - drag.x).abs() < 1.0 && !drag.moved {
            return true;
        }
        let window_width = self.current_terminal_size_info().window_size().width_px();
        let width = self.clamp_sidebar_width(drag.width + position.x - drag.x, window_width);
        let previous_width = self.sidebar_width(window_width);
        self.sidebar_resize.state.sidebar_width = Some(f64::from(width) / self.sidebar_scale());
        self.sidebar_resize.drag.as_mut().unwrap().moved = true;
        self.sidebar_resize.last_click = None;
        if width != previous_width {
            self.refresh_sidebar_layout();
        }
        true
    }

    pub(super) fn finish_sidebar_resize(&mut self) -> bool {
        let Some(drag) = self.sidebar_resize.drag.take() else {
            return false;
        };
        if drag.moved {
            self.sidebar_resize.save();
        }
        let edge = self.sidebar_edge_at_cursor();
        if let Some(runtime) = self.render_runtime.as_mut() {
            runtime.set_sidebar_resize_cursor(edge);
        }
        true
    }

    fn refresh_sidebar_layout(&mut self) {
        self.resize_workspace_gshells(self.current_terminal_size_info().window_size());
        self.update_ime_cursor_area();
    }
}
