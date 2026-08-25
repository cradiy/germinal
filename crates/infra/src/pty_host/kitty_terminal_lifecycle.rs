use alacritty_terminal::vte::ansi::{ClearMode, Handler, NamedPrivateMode, PrivateMode};

#[derive(Debug)]
pub(crate) enum KittyTerminalLifecycleEvent {
    ClearScreen(ClearMode),
    Reset,
    EnterAlternateScreen,
    LeaveAlternateScreen,
    ScrollUp { start: u32, end: u32, lines: u32 },
    ScrollDown { start: u32, end: u32, lines: u32 },
    DeleteLines { end: u32, lines: u32 },
    InsertBlankLines { end: u32, lines: u32 },
    Linefeed { start: u32, end: u32 },
    ReverseIndex { start: u32, end: u32 },
    Input { start: u32, end: u32 },
}

#[derive(Debug)]
pub(crate) struct KittyTerminalLifecycleObserver {
    screen_lines: u32,
    scroll_start: u32,
    scroll_end: u32,
    record_position_events: bool,
    events: Vec<KittyTerminalLifecycleEvent>,
}

impl KittyTerminalLifecycleObserver {
    pub(crate) fn new(screen_lines: usize) -> Self {
        let screen_lines = u32::try_from(screen_lines).unwrap_or(u32::MAX);
        Self {
            screen_lines,
            scroll_start: 0,
            scroll_end: screen_lines,
            record_position_events: true,
            events: Vec::new(),
        }
    }

    pub(crate) fn set_record_position_events(&mut self, record: bool) {
        self.record_position_events = record;
    }

    pub(crate) fn resize(&mut self, screen_lines: usize) {
        self.screen_lines = u32::try_from(screen_lines).unwrap_or(u32::MAX);
        self.scroll_start = 0;
        self.scroll_end = self.screen_lines;
    }

    pub(crate) fn take_events(&mut self) -> Vec<KittyTerminalLifecycleEvent> {
        std::mem::take(&mut self.events)
    }

    fn push_input_event(&mut self) {
        if self.record_position_events {
            self.events.push(KittyTerminalLifecycleEvent::Input {
                start: self.scroll_start,
                end: self.scroll_end,
            });
        }
    }
}

impl Handler for KittyTerminalLifecycleObserver {
    fn input(&mut self, _c: char) {
        self.push_input_event();
    }

    fn put_tab(&mut self, _count: u16) {
        self.push_input_event();
    }

    fn linefeed(&mut self) {
        if self.record_position_events {
            self.events.push(KittyTerminalLifecycleEvent::Linefeed {
                start: self.scroll_start,
                end: self.scroll_end,
            });
        }
    }

    fn scroll_up(&mut self, lines: usize) {
        if self.record_position_events {
            self.events.push(KittyTerminalLifecycleEvent::ScrollUp {
                start: self.scroll_start,
                end: self.scroll_end,
                lines: u32::try_from(lines).unwrap_or(u32::MAX),
            });
        }
    }

    fn scroll_down(&mut self, lines: usize) {
        if self.record_position_events {
            self.events.push(KittyTerminalLifecycleEvent::ScrollDown {
                start: self.scroll_start,
                end: self.scroll_end,
                lines: u32::try_from(lines).unwrap_or(u32::MAX),
            });
        }
    }

    fn insert_blank_lines(&mut self, lines: usize) {
        if self.record_position_events {
            self.events
                .push(KittyTerminalLifecycleEvent::InsertBlankLines {
                    end: self.scroll_end,
                    lines: u32::try_from(lines).unwrap_or(u32::MAX),
                });
        }
    }

    fn delete_lines(&mut self, lines: usize) {
        if self.record_position_events {
            self.events.push(KittyTerminalLifecycleEvent::DeleteLines {
                end: self.scroll_end,
                lines: u32::try_from(lines).unwrap_or(u32::MAX),
            });
        }
    }

    fn clear_screen(&mut self, mode: ClearMode) {
        self.events
            .push(KittyTerminalLifecycleEvent::ClearScreen(mode));
    }

    fn reset_state(&mut self) {
        self.scroll_start = 0;
        self.scroll_end = self.screen_lines;
        self.events.push(KittyTerminalLifecycleEvent::Reset);
    }

    fn reverse_index(&mut self) {
        if self.record_position_events {
            self.events.push(KittyTerminalLifecycleEvent::ReverseIndex {
                start: self.scroll_start,
                end: self.scroll_end,
            });
        }
    }

    fn set_private_mode(&mut self, mode: PrivateMode) {
        match mode {
            PrivateMode::Named(NamedPrivateMode::SwapScreenAndSetRestoreCursor) => self
                .events
                .push(KittyTerminalLifecycleEvent::EnterAlternateScreen),
            PrivateMode::Named(NamedPrivateMode::ColumnMode) => self
                .events
                .push(KittyTerminalLifecycleEvent::ClearScreen(ClearMode::All)),
            _ => {}
        }
    }

    fn unset_private_mode(&mut self, mode: PrivateMode) {
        match mode {
            PrivateMode::Named(NamedPrivateMode::SwapScreenAndSetRestoreCursor) => self
                .events
                .push(KittyTerminalLifecycleEvent::LeaveAlternateScreen),
            PrivateMode::Named(NamedPrivateMode::ColumnMode) => self
                .events
                .push(KittyTerminalLifecycleEvent::ClearScreen(ClearMode::All)),
            _ => {}
        }
    }

    fn set_scrolling_region(&mut self, top: usize, bottom: Option<usize>) {
        let bottom = bottom.unwrap_or(self.screen_lines as usize);
        if top >= bottom {
            return;
        }

        self.scroll_start = u32::try_from(top.saturating_sub(1))
            .unwrap_or(u32::MAX)
            .min(self.screen_lines);
        self.scroll_end = u32::try_from(bottom)
            .unwrap_or(u32::MAX)
            .min(self.screen_lines);
    }
}
