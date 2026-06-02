//! The grid widget.
//!
//! Renders a `tabl_core::Snapshot` as a ratatui `Table`. ratatui doesn't scroll
//! columns, so we window them ourselves: starting at `viewport.col_offset`, take
//! as many columns as fit the width, and return that count so navigation can
//! keep the selected column on screen.

use ratatui::{
    Frame,
    layout::{Constraint, Rect},
    style::{Color, Modifier, Style},
    text::{Line, Text},
    widgets::{Cell, HighlightSpacing, Row, Table, TableState},
};
use tabl_core::Snapshot;

use crate::viewport::{ColSpan, GridGeometry, Viewport};

/// Columns wider than this are truncated by ratatui's cell clipping.
const MAX_COL_WIDTH: usize = 40;

/// Shown (dimmed) in place of a null cell.
const NULL_SENTINEL: &str = "<null>";

/// Width of the trailing phantom column — a navigable "append column" slot
/// past the real columns, marked with a dim `+`.
const PHANTOM_WIDTH: u16 = 3;

/// Cycled per column so adjacent columns are easy to tell apart. Named ANSI
/// colors so they respect the user's terminal theme.
const PALETTE: [Color; 6] = [
    Color::Cyan,
    Color::Green,
    Color::Yellow,
    Color::Magenta,
    Color::Blue,
    Color::LightRed,
];

/// Faint background marking the current row and column (the crosshair), so the
/// cursor stays easy to locate in a wide table. The selected cell keeps its
/// stronger reversed highlight at the intersection. A dark indexed gray reads as
/// a subtle tint over the column colors and the null dimming alike.
///
/// The row/column arms are drawn by ratatui's `Table` highlight styles rather
/// than per cell, so the tint spans the inter-column spacing too — painting it
/// cell-by-cell would leave the row arm segmented at every column boundary.
pub(crate) const CROSSHAIR_BG: Color = Color::Indexed(236);

/// Color for an absolute column index — stable across horizontal scrolling.
fn column_color(col: usize) -> Color {
    PALETTE[col % PALETTE.len()]
}

/// Renders the grid and returns its screen [`GridGeometry`] — the column spans
/// and data-row band — so mouse clicks can be resolved to a cell. The number of
/// rendered columns (the horizontal "page" size) is `geometry.cols.len()`.
///
/// `editing`, when `Some`, is the in-progress text for the selected cell — shown
/// in place of its stored value with a caret.
pub fn render(
    frame: &mut Frame,
    area: Rect,
    snap: &Snapshot,
    vp: &Viewport,
    editing: Option<&str>,
) -> GridGeometry {
    let ncols = snap.columns.len();
    // One extra navigable slot past the real columns: the phantom "append" cell.
    let phantom = ncols;
    let navigable = ncols + 1;

    // Display width of column `c`: the widest of its name, its dtype label, and
    // any visible cell. The dtype label matters or the second header line clips.
    let col_width = |c: usize| -> u16 {
        if c >= ncols {
            return PHANTOM_WIDTH;
        }
        let mut w = snap.columns[c]
            .name
            .chars()
            .count()
            .max(snap.columns[c].dtype.to_string().chars().count());
        for row in &snap.rows {
            if let Some(value) = row.get(c) {
                let cell_w = if value.is_null() {
                    NULL_SENTINEL.chars().count()
                } else {
                    value.display().chars().count()
                };
                w = w.max(cell_w);
            }
        }
        w.clamp(1, MAX_COL_WIDTH) as u16
    };

    // Left gutter shows the absolute row index, sized to the largest one.
    let gutter = snap.total_rows.saturating_sub(1).to_string().len().max(1) as u16;
    let spacing = 1u16;

    // Walk columns from the offset, accumulating width until we run out of room
    // (always show at least one, even if it overflows a narrow terminal).
    let mut used = gutter + spacing;
    let mut visible = 0usize;
    for c in vp.col_offset..navigable {
        let needed = col_width(c) + spacing;
        if used + needed > area.width && visible > 0 {
            break;
        }
        used += needed;
        visible += 1;
    }
    let end = (vp.col_offset + visible).min(navigable);

    let dim = Style::default().add_modifier(Modifier::DIM);

    // Header cell for a column index — real columns get a colored name over a
    // dim dtype; the phantom gets a dim `+`.
    let header_cell = |c: usize| -> Cell {
        if c >= ncols {
            return Cell::from(Text::from(vec![Line::from("+").style(dim), Line::from("")]));
        }
        let name_style = Style::default()
            .fg(column_color(c))
            .add_modifier(Modifier::BOLD);
        Cell::from(Text::from(vec![
            Line::from(snap.columns[c].name.clone()).style(name_style),
            Line::from(snap.columns[c].dtype.to_string()).style(dim),
        ]))
    };

    let mut header_cells = Vec::with_capacity(visible + 1);
    header_cells.push(Cell::from(Text::from(vec![
        Line::from("#"),
        Line::from(""),
    ])));
    for c in vp.col_offset..end {
        let mut cell = header_cell(c);
        if c == vp.sel_col {
            cell = cell.style(Style::default().bg(CROSSHAIR_BG));
        }
        header_cells.push(cell);
    }
    let header = Row::new(header_cells).height(2);

    // The crosshair arms and the reversed selected cell are applied by the
    // `Table` highlight styles below; cells here carry only their own color so
    // those tints patch cleanly on top.
    let rows = snap.rows.iter().enumerate().map(|(r, row)| {
        let abs_row = snap.row_offset + r;
        let is_selected_row = abs_row == vp.sel_row;
        let mut cells = Vec::with_capacity(visible + 1);
        cells.push(Cell::from(abs_row.to_string()).style(dim));
        for c in vp.col_offset..end {
            // The phantom column is blank; the highlight styles still mark it so
            // the cursor is visible when parked there.
            if c == phantom {
                cells.push(Cell::from(""));
                continue;
            }

            let editing_here = is_selected_row && c == vp.sel_col && editing.is_some();
            let is_null = row.get(c).map(|v| v.is_null()).unwrap_or(true);

            let text = if editing_here {
                // Show the live buffer with a caret while editing this cell.
                format!("{}▏", editing.unwrap_or_default())
            } else if is_null {
                NULL_SENTINEL.to_string()
            } else {
                row.get(c).map(|v| v.display()).unwrap_or_default()
            };

            // Null cells render faded like the dtype labels; otherwise the
            // column color. The selected cell's reverse comes from the table's
            // cell highlight, which patches over whichever applies here.
            let style = if is_null && !editing_here {
                dim
            } else {
                Style::default().fg(column_color(c))
            };
            cells.push(Cell::from(text).style(style));
        }
        Row::new(cells)
    });

    let mut constraints = Vec::with_capacity(visible + 1);
    constraints.push(Constraint::Length(gutter));
    for c in vp.col_offset..end {
        constraints.push(Constraint::Length(col_width(c)));
    }

    let crosshair = Style::default().bg(CROSSHAIR_BG);
    let table = Table::new(rows, constraints)
        .header(header)
        .column_spacing(spacing)
        // Row and column arms get the faint tint; the intersection resets the bg
        // and reverses so the selected cell reads as the usual solid block. No
        // highlight symbol, so no column is stolen for selection spacing.
        .row_highlight_style(crosshair)
        .column_highlight_style(crosshair)
        .cell_highlight_style(
            Style::default()
                .bg(Color::Reset)
                .add_modifier(Modifier::REVERSED),
        )
        .highlight_spacing(HighlightSpacing::Never);

    // Selection indices are absolute and span all columns including the gutter;
    // translate them to positions within the rendered window. Columns are
    // offset by one for the leading row-index gutter.
    let mut state = TableState::default();
    if !snap.rows.is_empty() {
        state.select(Some(vp.sel_row - snap.row_offset));
    }
    if vp.sel_col >= vp.col_offset && vp.sel_col < end {
        state.select_column(Some(vp.sel_col - vp.col_offset + 1));
    }
    frame.render_stateful_widget(table, area, &mut state);

    // Capture the screen geometry for mouse hit-testing. The gutter sits flush
    // at the left edge; every column is then preceded by one cell of spacing,
    // mirroring the constraint/`column_spacing` layout above. The header is two
    // lines, so data rows start two below the table top.
    let mut cols = Vec::with_capacity(visible);
    let mut x = area.x + gutter + spacing;
    for c in vp.col_offset..end {
        let width = col_width(c);
        cols.push(ColSpan { col: c, x, width });
        x += width + spacing;
    }
    GridGeometry {
        data_top: area.y + 2,
        rows: snap.rows.len(),
        row_offset: snap.row_offset,
        cols,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn column_color_cycles() {
        assert_ne!(column_color(0), column_color(1));
        // Wraps back to the start after a full palette.
        assert_eq!(column_color(0), column_color(PALETTE.len()));
    }
}
