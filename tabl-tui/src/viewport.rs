//! Scroll offsets and selection.
//!
//! Vertical scrolling is straightforward, but ratatui's `Table` does not scroll
//! columns horizontally — so we track a column offset and compute which columns
//! fit the available width ourselves.

#[derive(Debug, Default)]
pub struct Viewport {
    pub row_offset: usize,
    pub col_offset: usize,
    pub sel_row: usize,
    pub sel_col: usize,
}

impl Viewport {
    /// Given the visible width and per-column widths starting at `col_offset`,
    /// return how many columns fit. Used to window columns horizontally.
    pub fn visible_cols(&self, avail_width: u16, widths: &[u16]) -> usize {
        let mut used = 0u16;
        let mut count = 0;
        for &w in widths.iter().skip(self.col_offset) {
            // +1 for the column separator.
            let next = used.saturating_add(w).saturating_add(1);
            if next > avail_width {
                break;
            }
            used = next;
            count += 1;
        }
        count
    }
}

/// Screen geometry of the most recently rendered grid, fed back from the
/// renderer so a mouse click can be mapped to the cell under it. The renderer
/// owns the column-windowing math (`col_offset`, per-column widths, the gutter),
/// so rather than re-derive it in the event handler we capture the result here
/// each frame — the same way `page_rows`/`page_cols` are fed back.
#[derive(Debug, Default)]
pub struct GridGeometry {
    /// Screen y of the first data row (just below the two-line header).
    pub data_top: u16,
    /// Number of data rows currently on screen.
    pub rows: usize,
    /// Absolute index of the first visible row.
    pub row_offset: usize,
    /// One span per visible column (including the phantom append slot), left to
    /// right in screen-x order.
    pub cols: Vec<ColSpan>,
}

/// The horizontal extent of one rendered column on screen.
#[derive(Debug, Clone, Copy)]
pub struct ColSpan {
    /// Absolute column index.
    pub col: usize,
    /// Screen x of the column's left edge.
    pub x: u16,
    /// Rendered width in columns.
    pub width: u16,
}

impl GridGeometry {
    /// Map a screen click to the absolute `(row, col)` it lands on, or `None`
    /// if it falls on the header, the gutter, or empty padding past the data.
    pub fn hit(&self, x: u16, y: u16) -> Option<(usize, usize)> {
        if y < self.data_top {
            return None;
        }
        let r = (y - self.data_top) as usize;
        if r >= self.rows {
            return None;
        }
        let col = self
            .cols
            .iter()
            .find(|c| x >= c.x && x < c.x + c.width)?
            .col;
        Some((self.row_offset + r, col))
    }
}
