//! Rendering. Each submodule owns one region of the screen.

pub mod editor;
pub mod status;
pub mod table;

use ratatui::{
    Frame,
    layout::{Constraint, Layout},
};

use crate::app::{App, Mode};

/// Top-level draw: the grid fills the screen above a one-line status bar.
///
/// Takes `&mut App` so the renderer can feed the visible row/column counts back
/// into the app — navigation needs them to keep the selection on screen.
pub fn draw(frame: &mut Frame, app: &mut App) {
    let [grid_area, status_area] =
        Layout::vertical([Constraint::Min(0), Constraint::Length(1)]).areas(frame.area());

    // The header is two lines (name + dtype), so data rows = height - 2.
    app.page_rows = grid_area.height.saturating_sub(2) as usize;

    let snapshot = app.sheet.view(app.viewport.row_offset, app.page_rows);
    let editing = (app.mode == Mode::Insert).then_some((app.edit.as_str(), app.edit_cursor));
    let geometry = table::render(frame, grid_area, &snapshot, &app.viewport, editing);
    // Feed the rendered geometry back so navigation (selection-follows-view) and
    // mouse hit-testing can read the column window the renderer just produced.
    app.page_cols = geometry.cols.len();
    app.grid = Some(geometry);

    status::render(frame, status_area, app);
}
