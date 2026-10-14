use super::*;
use germinal_ports::rendering::command_palette::{CommandPaletteEntry, CommandPaletteSnapshot};

const COMMANDS: &[(KeyboardAction, &str, &str)] = &[
    (KeyboardAction::NewTab, "New tab", "新建标签"),
    (KeyboardAction::NewWindow, "New window", "新建窗口"),
    (
        KeyboardAction::SplitHorizontal,
        "Split horizontally",
        "水平分屏 左右",
    ),
    (
        KeyboardAction::SplitVertical,
        "Split vertically",
        "垂直分屏 上下",
    ),
    (KeyboardAction::NextTab, "Next tab", "下一个标签"),
    (KeyboardAction::PreviousTab, "Previous tab", "上一个标签"),
    (KeyboardAction::ToggleSearch, "Search terminal", "搜索查找"),
    (
        KeyboardAction::ToggleViMode,
        "Toggle Vi mode",
        "切换 vim 模式",
    ),
    (KeyboardAction::Copy, "Copy selection", "复制选中"),
    (KeyboardAction::Paste, "Paste clipboard", "粘贴剪贴板"),
    (
        KeyboardAction::FocusNextPane,
        "Focus next pane",
        "下一个面板焦点",
    ),
    (
        KeyboardAction::FocusPreviousPane,
        "Focus previous pane",
        "上一个面板焦点",
    ),
    (
        KeyboardAction::FocusPaneLeft,
        "Focus pane left",
        "左侧面板焦点",
    ),
    (
        KeyboardAction::FocusPaneRight,
        "Focus pane right",
        "右侧面板焦点",
    ),
    (
        KeyboardAction::FocusPaneUp,
        "Focus pane above",
        "上方面板焦点",
    ),
    (
        KeyboardAction::FocusPaneDown,
        "Focus pane below",
        "下方面板焦点",
    ),
    (KeyboardAction::MoveTabLeft, "Move tab left", "左移标签"),
    (KeyboardAction::MoveTabRight, "Move tab right", "右移标签"),
    (
        KeyboardAction::SwapPaneLeft,
        "Swap pane left",
        "交换左侧面板",
    ),
    (
        KeyboardAction::SwapPaneRight,
        "Swap pane right",
        "交换右侧面板",
    ),
    (
        KeyboardAction::SwapPaneUp,
        "Swap pane above",
        "交换上方面板",
    ),
    (
        KeyboardAction::SwapPaneDown,
        "Swap pane below",
        "交换下方面板",
    ),
    (
        KeyboardAction::ResizePaneLeft,
        "Resize pane left",
        "调整面板大小 向左",
    ),
    (
        KeyboardAction::ResizePaneRight,
        "Resize pane right",
        "调整面板大小 向右",
    ),
    (
        KeyboardAction::ResizePaneUp,
        "Resize pane up",
        "调整面板大小 向上",
    ),
    (
        KeyboardAction::ResizePaneDown,
        "Resize pane down",
        "调整面板大小 向下",
    ),
    (
        KeyboardAction::ClosePane,
        "Close current pane",
        "关闭当前面板标签",
    ),
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
            .filter(|(action, _, _)| match action {
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
            .map(|(action, title, aliases)| {
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
                    format!("{title} {aliases}").to_lowercase(),
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
        use winit::platform::wayland::EventLoopBuilderExtWayland;
        struct Preview {
            app: App,
            deadline: Instant,
        }
        impl ApplicationHandler<RuntimeEvent> for Preview {
            fn resumed(&mut self, event_loop: &ActiveEventLoop) {
                self.app.resumed(event_loop);
                self.app.open_command_palette();
                assert!(self.app.handle_command_palette_event(
                    event_loop,
                    &WindowEvent::Ime(Ime::Preedit("fen".into(), Some((3, 3))))
                ));
                assert!(self.app.handle_command_palette_event(
                    event_loop,
                    &WindowEvent::Ime(Ime::Commit("分屏".into()))
                ));
                assert_eq!(
                    self.app.command_palette.as_ref().unwrap().matches().len(),
                    2
                );
                self.app.command_palette.as_mut().unwrap().query.clear();
                self.app.sync_command_palette();
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
        config.window.title = "Germinal command palette preview".into();
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
                deadline: Instant::now() + Duration::from_secs(20),
            })
            .unwrap();
    }

    #[test]
    fn filtering_navigation_and_commit_select_the_same_command() {
        let mut palette = CommandPalette::new(&[], true, true);
        palette.insert("分屏");
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
