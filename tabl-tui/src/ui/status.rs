//! Status line at the bottom: mode indicator and cursor position.

use ratatui::{
    Frame,
    layout::Rect,
    style::{Modifier, Style},
    widgets::Paragraph,
};

use crate::app::{App, Mode};

pub fn render(frame: &mut Frame, area: Rect, app: &App) {
    let reversed = Style::default().add_modifier(Modifier::REVERSED);

    // In Command mode the bar becomes the `:` input line, like vim.
    if app.mode == Mode::Command {
        let bar = Paragraph::new(format!(":{}", app.command));
        frame.render_widget(bar, area);
        return;
    }

    // In Insert mode show the buffer and any parse error.
    if app.mode == Mode::Insert {
        let mut text = format!(" INSERT   {}   (Enter commit · Esc cancel)", app.edit);
        if let Some(msg) = &app.message {
            text.push_str(&format!("   ⚠ {msg}"));
        }
        frame.render_widget(Paragraph::new(text).style(reversed), area);
        return;
    }

    let (rows, cols) = app.sheet.shape();
    let vp = &app.viewport;

    let mode = match app.mode {
        Mode::Normal => "NORMAL",
        Mode::Insert => "INSERT",
        Mode::Command => "COMMAND",
    };

    // 1-based for display; show 0 for an empty frame rather than 1/0.
    let row = if rows == 0 { 0 } else { vp.sel_row + 1 };
    // On the phantom "append" slot, show `+` instead of an out-of-range index.
    let col = if cols == 0 {
        "0".to_string()
    } else if vp.sel_col >= cols {
        "+".to_string()
    } else {
        (vp.sel_col + 1).to_string()
    };

    let mut text = format!(" {mode}   row {row}/{rows}   col {col}/{cols}  ");
    if app.sheet.is_filtered() {
        text.push_str(" [filtered] ");
    }
    if let Some(pending) = app.pending_key {
        text.push_str(&format!(" {pending}… "));
    } else if let Some(msg) = &app.message {
        text.push_str(&format!(" {msg} "));
    } else {
        text.push_str(" (i edit · aa/dd row · :eval · :w save · :q quit) ");
    }
    frame.render_widget(Paragraph::new(text).style(reversed), area);
}
