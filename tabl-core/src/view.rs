use crate::{column::ColumnMeta, value::Value};

/// A render-ready window of a sheet: the column metadata plus the rows in
/// `[row_offset, row_offset + rows.len())`, with the edit overlay already
/// applied. Produced by `tabl-engine`, consumed by `tabl-tui`.
#[derive(Debug, Default)]
pub struct Snapshot {
    pub columns: Vec<ColumnMeta>,
    pub row_offset: usize,
    pub total_rows: usize,
    /// Row-major: `rows[r][c]` is the cell at display row `r`, column `c`.
    pub rows: Vec<Vec<Value>>,
}
