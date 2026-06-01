//! The edit overlay: pending cell edits layered on top of a read-only frame.
//!
//! Keeping edits separate from the polars `DataFrame` decouples edit latency
//! from frame size and gives undo/redo for free. The engine materializes the
//! overlay into a new frame only on save.

use std::collections::HashMap;

use crate::value::Value;

/// Address of a cell by zero-based row and column index.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct CellAddr {
    pub row: usize,
    pub col: usize,
}

impl CellAddr {
    pub fn new(row: usize, col: usize) -> Self {
        Self { row, col }
    }
}

#[derive(Debug, Default)]
pub struct EditOverlay {
    cells: HashMap<CellAddr, Value>,
    // TODO: undo/redo stack; column add/drop ops live here too.
}

impl EditOverlay {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn get(&self, addr: CellAddr) -> Option<&Value> {
        self.cells.get(&addr)
    }

    pub fn set(&mut self, addr: CellAddr, value: Value) {
        self.cells.insert(addr, value);
    }

    pub fn clear_cell(&mut self, addr: CellAddr) {
        self.cells.remove(&addr);
    }

    pub fn is_empty(&self) -> bool {
        self.cells.is_empty()
    }

    pub fn iter(&self) -> impl Iterator<Item = (&CellAddr, &Value)> {
        self.cells.iter()
    }
}
