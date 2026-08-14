use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct TerminalGridSize {
    columns: usize,
    rows: usize,
}

impl TerminalGridSize {
    pub const fn new(columns: usize, rows: usize) -> Self {
        assert!(
            columns > 0,
            "terminal grid columns must be greater than zero"
        );
        assert!(rows > 0, "terminal grid rows must be greater than zero");

        Self { columns, rows }
    }

    pub const fn columns(self) -> usize {
        self.columns
    }

    pub const fn rows(self) -> usize {
        self.rows
    }
}
