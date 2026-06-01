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
    widgets::{Cell, Row, Table},
};
use tabl_core::Snapshot;

use crate::viewport::Viewport;

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

/// Color for an absolute column index — stable across horizontal scrolling.
fn column_color(col: usize) -> Color {
    PALETTE[col % PALETTE.len()]
}

/// Returns the number of columns rendered (the horizontal "page" size).
///
/// `editing`, when `Some`, is the in-progress text for the selected cell — shown
/// in place of its stored value with a caret.
pub fn render(
    frame: &mut Frame,
    area: Rect,
    snap: &Snapshot,
    vp: &Viewport,
    editing: Option<&str>,
) -> usize {
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
        header_cells.push(header_cell(c));
    }
    let header = Row::new(header_cells).height(2);

    let rows = snap.rows.iter().enumerate().map(|(r, row)| {
        let abs_row = snap.row_offset + r;
        let mut cells = Vec::with_capacity(visible + 1);
        cells.push(Cell::from(abs_row.to_string()).style(dim));
        for c in vp.col_offset..end {
            let is_selected = abs_row == vp.sel_row && c == vp.sel_col;

            // The phantom column is blank; selection still highlights it so the
            // cursor is visible when parked there.
            if c == phantom {
                let style = if is_selected {
                    Style::default().add_modifier(Modifier::REVERSED)
                } else {
                    Style::default()
                };
                cells.push(Cell::from("").style(style));
                continue;
            }

            let editing_here = is_selected && editing.is_some();
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
            // column color. The selected cell reverses whichever applies.
            let mut style = if is_null && !editing_here {
                dim
            } else {
                Style::default().fg(column_color(c))
            };
            if is_selected {
                style = style.add_modifier(Modifier::REVERSED);
            }
            cells.push(Cell::from(text).style(style));
        }
        Row::new(cells)
    });

    let mut constraints = Vec::with_capacity(visible + 1);
    constraints.push(Constraint::Length(gutter));
    for c in vp.col_offset..end {
        constraints.push(Constraint::Length(col_width(c)));
    }

    let table = Table::new(rows, constraints)
        .header(header)
        .column_spacing(spacing);
    frame.render_widget(table, area);

    visible
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
