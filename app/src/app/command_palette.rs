use super::*;
use germinal_ports::rendering::command_palette::{CommandPaletteEntry, CommandPaletteSnapshot};

const COMMANDS: &[(KeyboardAction, &str)] = &[
    (KeyboardAction::NewTab, "New tab"),
    (KeyboardAction::NewWindow, "New window"),
    (KeyboardAction::SplitHorizontal, "Split horizontally"),
    (KeyboardAction::SplitVertical, "Split vertically"),
    (KeyboardAction::NextTab, "Next tab"),
    (KeyboardAction::PreviousTab, "Previous tab"),
    (KeyboardAction::ToggleSearch, "Search terminal"),
    (KeyboardAction::ToggleViMode, "Toggle Vi mode"),
    (KeyboardAction::Copy, "Copy selection"),
    (KeyboardAction::Paste, "Paste clipboard"),
    (KeyboardAction::ToggleToolbar, "Toggle modern toolbar"),
    (KeyboardAction::ToggleTabSidebar, "Toggle tab sidebar"),
    (KeyboardAction::FocusNextPane, "Focus next pane"),
    (KeyboardAction::FocusPreviousPane, "Focus previous pane"),
    (KeyboardAction::FocusPaneLeft, "Focus pane left"),
    (KeyboardAction::FocusPaneRight, "Focus pane right"),
    (KeyboardAction::FocusPaneUp, "Focus pane above"),
    (KeyboardAction::FocusPaneDown, "Focus pane below"),
    (KeyboardAction::MoveTabLeft, "Move tab left"),
    (KeyboardAction::MoveTabRight, "Move tab right"),
    (KeyboardAction::SwapPaneLeft, "Swap pane left"),
    (KeyboardAction::SwapPaneRight, "Swap pane right"),
    (KeyboardAction::SwapPaneUp, "Swap pane above"),
    (KeyboardAction::SwapPaneDown, "Swap pane below"),
    (KeyboardAction::ResizePaneLeft, "Resize pane left"),
    (KeyboardAction::ResizePaneRight, "Resize pane right"),
    (KeyboardAction::ResizePaneUp, "Resize pane up"),
    (KeyboardAction::ResizePaneDown, "Resize pane down"),
    (KeyboardAction::ClosePane, "Close current pane"),
];

pub(super) struct CommandPalette {
    query: String,
    preedit: String,
    preedit_cursor: Option<usize>,
    selected: usize,
    commands: Vec<(KeyboardAction, CommandPaletteEntry, String)>,
}

impl CommandPalette {
    fn new(bindings: &[KeyboardBinding], panes: bool, tabs: bool) -> Self {
        let commands = COMMANDS
            .iter()
            .filter(|(action, _)| match action {
                KeyboardAction::FocusNextPane
                | KeyboardAction::FocusPreviousPane
                | KeyboardAction::FocusPaneLeft
                | KeyboardAction::FocusPaneRight
                | KeyboardAction::FocusPaneUp
                | KeyboardAction::FocusPaneDown
                | KeyboardAction::SwapPaneLeft
                | KeyboardAction::SwapPaneRight
                | KeyboardAction::SwapPaneUp
                | KeyboardAction::SwapPaneDown
                | KeyboardAction::ResizePaneLeft
                | KeyboardAction::ResizePaneRight
                | KeyboardAction::ResizePaneUp
                | KeyboardAction::ResizePaneDown => panes,
                KeyboardAction::NextTab
                | KeyboardAction::PreviousTab
                | KeyboardAction::MoveTabLeft
                | KeyboardAction::MoveTabRight => tabs,
                _ => true,
            })
            .map(|(action, title)| {
                let shortcut = bindings
                    .iter()
                    .find(|binding| binding.action == *action)
                    .map(|binding| {
                        let mods = binding.mods.replace("Control", "Ctrl").replace('|', "+");
                        if mods.is_empty() {
                            binding.key.clone()
                        } else {
                            format!("{mods}+{}", binding.key)
                        }
                    })
                    .unwrap_or_default();
                (
                    *action,
                    CommandPaletteEntry {
                        title: (*title).into(),
                        shortcut,
                    },
                    title.to_lowercase(),
                )
            })
            .collect();
        Self {
            query: String::new(),
            preedit: String::new(),
            preedit_cursor: None,
            selected: 0,
            commands,
        }
    }

    fn matches(&self) -> Vec<usize> {
        let query = self.query.to_lowercase();
        self.commands
            .iter()
            .enumerate()
            .filter(|(_, (_, _, searchable))| {
                query
                    .split_whitespace()
                    .all(|word| searchable.contains(word))
            })
            .map(|(index, _)| index)
            .collect()
    }

    fn insert(&mut self, text: &str) {
        let remaining = 256usize.saturating_sub(self.query.chars().count());
        self.query
            .extend(text.chars().filter(|ch| !ch.is_control()).take(remaining));
        self.selected = 0;
    }

    fn navigate(&mut self, backwards: bool) {
        let count = self.matches().len();
        if count > 0 {
            self.selected = if backwards {
                (self.selected + count - 1) % count
            } else {
                (self.selected + 1) % count
            };
        }
    }

    fn action(&self) -> Option<KeyboardAction> {
        self.matches()
            .get(self.selected)
            .map(|index| self.commands[*index].0)
    }

    fn snapshot(&self) -> CommandPaletteSnapshot {
        CommandPaletteSnapshot {
            query: self.query.clone(),
            preedit: self.preedit.clone(),
            preedit_cursor: self.preedit_cursor,
            selected: self.selected,
            entries: self
                .matches()
                .iter()
                .map(|index| self.commands[*index].1.clone())
                .collect(),
        }
    }
}

impl App {
    pub(super) fn open_command_palette(&mut self) {
        self.clear_ime_preedit(self.focused_gshell());
        self.pointer_capture = None;
        self.pending_hyperlink_activation = None;
        self.command_palette = Some(CommandPalette::new(
            &self.config.keyboard.bindings,
            self.pane_navigation_enabled,
            self.tab_count() > 1,
        ));
        if let Some(runtime) = self.render_runtime.as_mut() {
            runtime.end_terminal_scrollbar_drag();
            runtime.set_hovered_terminal_hyperlinks(None);
        }
        self.sync_command_palette();
    }

    fn sync_command_palette(&mut self) {
        let snapshot = self.command_palette.as_ref().map(CommandPalette::snapshot);
        if let Some(runtime) = self.render_runtime.as_mut() {
            runtime.set_command_palette(snapshot);
        }
        self.update_ime_cursor_area();
        self.request_redraw();
    }

    fn close_command_palette(&mut self) {
        self.command_palette = None;
        if let Some(runtime) = self.render_runtime.as_mut() {
            runtime.set_ime_allowed(false);
            runtime.set_ime_allowed(self.window_focused);
        }
        self.reset_input_modifiers();
        self.sync_command_palette();
    }

    pub(super) fn handle_command_palette_event(
        &mut self,
        event_loop: &ActiveEventLoop,
        event: &WindowEvent,
    ) -> bool {
        if let WindowEvent::MouseInput {
            state: ElementState::Released,
            button,
            ..
        } = event
            && self.palette_consumed_buttons.remove(button)
        {
            return true;
        }
        if let WindowEvent::KeyboardInput { event, .. } = event
            && event.repeat
            && self.palette_consumed_keys.contains(&event.physical_key)
            && self.command_palette.is_none()
        {
            return true;
        }
        if let WindowEvent::KeyboardInput { event, .. } = event
            && event.state == ElementState::Released
            && self.palette_consumed_keys.remove(&event.physical_key)
        {
            self.paste_controller
                .observe_key_event(WindowInputElementState::Released, event.physical_key);
            return true;
        }
        let Some(palette) = self.command_palette.as_mut() else {
            return false;
        };
        match event {
            WindowEvent::KeyboardInput { event, .. } => {
                self.paste_controller.observe_key_event(
                    winit_element_state_to_port(event.state),
                    event.physical_key,
                );
                if event.state == ElementState::Released {
                    return true;
                }
                self.palette_consumed_keys.insert(event.physical_key);
                if !palette.preedit.is_empty() && event.logical_key != Key::Named(NamedKey::Escape)
                {
                    return true;
                }
                match &event.logical_key {
                    Key::Character(key)
                        if self.window_input_modifiers.control_key()
                            && !self.window_input_modifiers.alt_key()
                            && !self.window_input_modifiers.super_key()
                            && ["j", "k", "n", "p"].contains(&key.to_lowercase().as_str()) =>
                    {
                        palette.navigate(matches!(key.to_lowercase().as_str(), "k" | "p"));
                    }
                    Key::Named(NamedKey::Escape) => self.close_command_palette(),
                    Key::Named(NamedKey::Enter) => {
                        let action = palette.action();
                        self.close_command_palette();
                        if let Some(action) = action {
                            self.execute_keyboard_action(event_loop, action);
                        }
                    }
                    Key::Named(NamedKey::ArrowUp) => palette.navigate(true),
                    Key::Named(NamedKey::ArrowDown) => palette.navigate(false),
                    Key::Named(NamedKey::Backspace) => {
                        palette.query.pop();
                        palette.selected = 0;
                    }
                    _ if !self.window_input_modifiers.control_key()
                        && !self.window_input_modifiers.alt_key()
                        && !self.window_input_modifiers.super_key() =>
                    {
                        if let Some(text) = &event.text {
                            palette.insert(text);
                        }
                    }
                    _ => {}
                }
            }
            WindowEvent::Ime(Ime::Commit(text)) => {
                palette.preedit.clear();
                palette.preedit_cursor = None;
                palette.insert(text);
            }
            WindowEvent::Ime(Ime::Preedit(text, cursor)) => {
                palette.preedit = text.clone();
                palette.preedit_cursor = cursor.map(|(_, end)| end);
            }
            WindowEvent::Ime(Ime::Enabled) => self.ime_enabled = true,
            WindowEvent::Ime(Ime::Disabled) => {
                self.ime_enabled = false;
                palette.preedit.clear();
                palette.preedit_cursor = None;
            }
            WindowEvent::ModifiersChanged(modifiers) => {
                self.window_input_modifiers = WindowInputModifiers::new(
                    modifiers.state().control_key(),
                    modifiers.state().alt_key(),
                    modifiers.state().shift_key(),
                    modifiers.state().super_key(),
                );
                return true;
            }
            WindowEvent::MouseInput { state, button, .. } => {
                if *state == ElementState::Pressed {
                    self.palette_consumed_buttons.insert(*button);
                }
                return true;
            }
            WindowEvent::CursorMoved { position, .. } => {
                self.cursor_position = Some(*position);
                return true;
            }
            WindowEvent::CursorLeft { .. } => {
                self.cursor_position = None;
                return true;
            }
            WindowEvent::MouseWheel { .. } => return true,
            WindowEvent::Focused(false) => {
                self.close_command_palette();
                self.palette_consumed_keys.clear();
                self.palette_consumed_buttons.clear();
                return false;
            }
            _ => return false,
        }
        self.sync_command_palette();
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[cfg(target_os = "linux")]
    #[test]
    #[ignore = "opens an isolated Wayland window for command palette inspection"]
    fn command_palette_window_smoke() {
        run_window_preview(false);
    }

    #[cfg(target_os = "linux")]
    #[test]
    #[ignore = "opens an isolated Wayland window for sidebar inspection"]
    fn sidebar_window_smoke() {
        run_window_preview(true);
    }

    #[cfg(target_os = "linux")]
    fn run_window_preview(sidebar: bool) {
        use winit::platform::wayland::EventLoopBuilderExtWayland;
        struct Preview {
            app: App,
            deadline: Instant,
            sidebar: bool,
        }
        impl ApplicationHandler<RuntimeEvent> for Preview {
            fn resumed(&mut self, event_loop: &ActiveEventLoop) {
                self.app.resumed(event_loop);
                if self.sidebar {
                    let size = self.app.current_terminal_size_info().window_size();
                    self.app.config.tabs.position = TabBarPosition::Top;
                    self.app
                        .execute_keyboard_action(event_loop, KeyboardAction::ToggleTabSidebar);
                    assert!(self.app.current_tab_bar_snapshot().is_some());
                    let layout = self.app.current_workspace_render_layout(size);
                    assert!(layout[0].x_px > 0);
                    assert_eq!(layout[0].x_px + layout[0].width_px, size.width_px());
                    assert!(
                        self.app
                            .pointer_input_at(PhysicalPosition::new(1.0, 100.0))
                            .is_none()
                    );
                    self.app
                        .execute_keyboard_action(event_loop, KeyboardAction::ToggleTabSidebar);
                    assert_eq!(self.app.config.tabs.position, TabBarPosition::Top);
                    assert_eq!(
                        self.app.current_workspace_render_layout(size)[0].width_px,
                        size.width_px()
                    );
                    self.app.create_workspace_tab();
                    self.app
                        .execute_keyboard_action(event_loop, KeyboardAction::ToggleTabSidebar);
                    let cell = self.app.current_terminal_size_info().cell_size();
                    let x = f64::from(cell.width_px() * 5);
                    let y = (0..size.height_px())
                        .find(|y| self.app.tab_index_at_position(x, f64::from(*y)) == Some(0))
                        .unwrap();
                    self.app.cursor_position = Some(PhysicalPosition::new(x, f64::from(y)));
                    assert!(self.app.try_activate_tab_at_cursor());
                    assert_eq!(self.app.active_tab_index(), 0);
                    use germinal_ports::rendering::toolbar::{
                        ToolbarAction, ToolbarConfig, toolbar_buttons, toolbar_height,
                    };
                    let h = toolbar_height(
                        self.app
                            .render_runtime
                            .as_ref()
                            .unwrap()
                            .display_scale_factor(),
                        size.height_px(),
                    );
                    let buttons = toolbar_buttons(
                        ToolbarConfig {
                            sidebar: true,
                            window_controls: true,
                        },
                        size.width_px(),
                        h,
                        cell.height_px(),
                        self.app
                            .render_runtime
                            .as_ref()
                            .unwrap()
                            .display_scale_factor(),
                    );
                    let toggle = buttons
                        .iter()
                        .find(|b| b.action == ToolbarAction::ToggleSidebar)
                        .unwrap();
                    let position = PhysicalPosition::new(f64::from(toggle.x + 1), f64::from(h / 2));
                    self.app.cursor_position = Some(position);
                    assert!(
                        self.app
                            .handle_toolbar_button(event_loop, ElementState::Pressed)
                    );
                    self.app.cursor_position =
                        Some(PhysicalPosition::new(position.x, f64::from(h + 1)));
                    assert!(
                        self.app
                            .handle_toolbar_button(event_loop, ElementState::Released)
                    );
                    assert_eq!(self.app.config.tabs.position, TabBarPosition::Left);
                    self.app.cursor_position = Some(position);
                    assert!(
                        self.app
                            .handle_toolbar_button(event_loop, ElementState::Pressed)
                    );
                    assert!(
                        self.app
                            .handle_toolbar_button(event_loop, ElementState::Released)
                    );
                    assert_eq!(self.app.config.tabs.position, TabBarPosition::Top);
                    assert!(self.app.modern_toolbar_visible());
                    for position in [TabBarPosition::Top, TabBarPosition::Bottom] {
                        self.app.config.tabs.position = position;
                        self.app.resize_workspace_gshells(size);
                        assert!(self.app.current_tab_bar_snapshot().is_none());
                        let layout = self.app.current_workspace_render_layout(size);
                        assert_eq!(layout[0].y_px, h);
                        assert_eq!(layout[0].height_px, size.height_px() - h);
                        assert!(
                            self.app
                                .tab_index_at_position(5.0, f64::from(size.height_px() - 2))
                                .is_none()
                        );
                        self.app
                            .execute_keyboard_action(event_loop, KeyboardAction::ToggleToolbar);
                        assert!(self.app.current_tab_bar_snapshot().is_some());
                        self.app.window_input_modifiers =
                            WindowInputModifiers::new(true, true, false, false);
                        for state in [
                            WindowInputElementState::Pressed,
                            WindowInputElementState::Released,
                        ] {
                            assert!(!self.app.try_handle_keyboard_binding(
                                event_loop,
                                state,
                                &WindowInputKey::Character("b".into()),
                                winit::keyboard::PhysicalKey::Code(winit::keyboard::KeyCode::KeyB),
                            ));
                        }
                        assert!(!self.app.modern_toolbar_visible());
                        assert_eq!(
                            self.app.current_workspace_render_layout(size)[0].height_px,
                            size.height_px() - cell.height_px()
                        );
                        self.app
                            .execute_keyboard_action(event_loop, KeyboardAction::ToggleToolbar);
                        for expected in [TabBarPosition::Left, position] {
                            assert!(self.app.try_handle_keyboard_binding(
                                event_loop,
                                WindowInputElementState::Pressed,
                                &WindowInputKey::Character("b".into()),
                                winit::keyboard::PhysicalKey::Code(winit::keyboard::KeyCode::KeyB),
                            ));
                            assert_eq!(self.app.config.tabs.position, expected);
                        }
                        self.app.reset_input_modifiers();
                    }
                    self.app.config.window.show_window_controls = false;
                    self.app.resize_workspace_gshells(size);
                    assert!(
                        self.app
                            .render_runtime
                            .as_ref()
                            .unwrap()
                            .toolbar_action_at(f64::from(size.width_px() - 1), f64::from(h / 2))
                            .is_none()
                    );
                    self.app.config.window.show_window_controls = true;
                    self.app
                        .execute_keyboard_action(event_loop, KeyboardAction::ToggleTabSidebar);
                    let retained = self.app.focused_gshell();
                    self.app.create_workspace_tab();
                    self.app
                        .execute_keyboard_action(event_loop, KeyboardAction::SplitVertical);
                    let closing = self.app.focused_gshell();
                    let closed_panes = self.app.visible_gshells();
                    assert_eq!(closed_panes.len(), 2);
                    assert!(self.app.focus_gshell(retained));
                    self.app.activate_workspace_tab(closing, retained);
                    let count = self.app.tab_count();
                    let close_position = (0..size.height_px())
                        .find_map(|y| {
                            (0..size.width_px() / 3).find_map(|x| {
                                let index = self
                                    .app
                                    .render_runtime
                                    .as_ref()
                                    .unwrap()
                                    .sidebar_close_at_position(f64::from(x), f64::from(y))?;
                                (self.app.tab_gshells()[index] == closing)
                                    .then_some(PhysicalPosition::new(f64::from(x), f64::from(y)))
                            })
                        })
                        .unwrap();
                    let window_id = self.app.render_window_id.unwrap();
                    let click = |app: &mut App, state| {
                        app.window_event(
                            event_loop,
                            window_id,
                            WindowEvent::MouseInput {
                                device_id: winit::event::DeviceId::dummy(),
                                state,
                                button: MouseButton::Left,
                            },
                        )
                    };
                    self.app.cursor_position = Some(close_position);
                    click(&mut self.app, ElementState::Pressed);
                    self.app.cursor_position = Some(PhysicalPosition::new(600.0, 400.0));
                    click(&mut self.app, ElementState::Released);
                    assert_eq!(self.app.tab_count(), count);
                    self.app.cursor_position = Some(close_position);
                    click(&mut self.app, ElementState::Pressed);
                    click(&mut self.app, ElementState::Released);
                    assert_eq!(self.app.tab_count(), count - 1);
                    assert_eq!(self.app.focused_gshell(), retained);
                    for pane in closed_panes {
                        assert!(!self.app.focus_gshell(pane));
                    }
                    return;
                }
                self.app.open_command_palette();
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
        config.window.title = if sidebar {
            "Germinal sidebar preview"
        } else {
            "Germinal command palette preview"
        }
        .into();
        config.window.width_px = 960;
        config.window.height_px = 640;
        config.terminal.shell = Some(config::ShellConfig {
            program: "/bin/bash".into(),
            args: vec![
                "--noprofile".into(),
                "--norc".into(),
                "-c".into(),
                "printf 'Command palette preview\\r\\n'; sleep 25".into(),
            ],
        });
        let app = App::new(event_loop.create_proxy(), config).unwrap();
        event_loop
            .run_app(&mut Preview {
                app,
                deadline: Instant::now() + Duration::from_secs(if sidebar { 8 } else { 20 }),
                sidebar,
            })
            .unwrap();
    }

    #[test]
    fn filtering_navigation_and_commit_select_the_same_command() {
        let mut palette = CommandPalette::new(&[], true, true);
        palette.insert("split");
        assert_eq!(palette.matches().len(), 2);
        palette.navigate(true);
        assert_eq!(palette.action(), Some(KeyboardAction::SplitVertical));
        palette.query.clear();
        palette.insert("TAB new");
        assert_eq!(palette.action(), Some(KeyboardAction::NewTab));
        palette.insert(" missing");
        palette.navigate(false);
        assert_eq!(palette.action(), None);
    }

    #[test]
    fn unavailable_commands_are_excluded_and_custom_bindings_are_displayed() {
        let palette = CommandPalette::new(
            &[KeyboardBinding {
                key: "t".into(),
                mods: "Alt".into(),
                action: KeyboardAction::NewTab,
            }],
            false,
            false,
        );
        assert!(!palette.commands.iter().any(|(action, _, _)| matches!(
            action,
            KeyboardAction::MoveTabLeft | KeyboardAction::FocusPaneLeft
        )));
        assert_eq!(palette.snapshot().entries[0].shortcut, "Alt+t");
    }
}
