use germinal_domain::pty_host::terminal_size::TerminalGridSize;

use crate::pty_host::{pty_input::PtyInputSender, terminal_input_mode::TerminalInputModeState};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TerminalDisplayScroll {
    Delta(i32),
    PageUp,
    PageDown,
    Top,
    Bottom,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TerminalSelectionKind {
    Character,
    Word,
    Line,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TerminalSelectionSide {
    Left,
    Right,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TerminalViMotion {
    Up,
    Down,
    Left,
    Right,
    First,
    FirstOccupied,
    Last,
    WordLeft,
    WordRight,
    WordRightEnd,
    High,
    Middle,
    Low,
    HalfPageUp,
    HalfPageDown,
    PageUp,
    PageDown,
    Top,
    Bottom,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TerminalViSelectionKind {
    Character,
    Line,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TerminalViTextObject {
    InnerWord,
    AroundWord,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TerminalSelectionPoint {
    pub column: u16,
    pub row: u16,
    pub side: TerminalSelectionSide,
}

impl TerminalSelectionPoint {
    pub const fn new(column: u16, row: u16, side: TerminalSelectionSide) -> Self {
        Self { column, row, side }
    }
}

pub enum TerminalWorkerInput {
    Bytes(Vec<u8>),
    Resize(TerminalGridSize),
    ScrollDisplay(TerminalDisplayScroll),
    StartSelection {
        kind: TerminalSelectionKind,
        point: TerminalSelectionPoint,
    },
    UpdateSelection(TerminalSelectionPoint),
    RequestSelectionText,
    SetViMode(bool),
    ViMotion(TerminalViMotion),
    SetViSelection(Option<TerminalViSelectionKind>),
    SelectViTextObject(TerminalViTextObject),
    SetPtyInput {
        sender: PtyInputSender,
        input_modes: TerminalInputModeState,
    },
}
