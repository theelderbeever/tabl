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
