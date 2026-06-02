//! Map terminal key events to state transitions on [`App`].

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers, MouseButton, MouseEvent, MouseEventKind};

use crate::app::{App, Mode};

/// Whether `key` is Ctrl+`c`.
fn is_ctrl(key: KeyEvent, c: char) -> bool {
    key.modifiers.contains(KeyModifiers::CONTROL) && key.code == KeyCode::Char(c)
}

/// Handle one key press, dispatching on the current mode.
pub fn handle_key(app: &mut App, key: KeyEvent) {
    match app.mode {
        Mode::Normal => normal(app, key),
        Mode::Command => command(app, key),
        Mode::Insert => insert(app, key),
    }
}

/// Handle one mouse event. Only acts in Normal mode: a left click jumps the
/// cursor to the clicked cell and the wheel scrolls the selection, but while
/// editing (Insert) or typing a command (Command) the mouse is ignored so a
/// stray click can't silently discard an in-progress edit.
pub fn handle_mouse(app: &mut App, ev: MouseEvent) {
    if app.mode != Mode::Normal {
        return;
    }
    match ev.kind {
        MouseEventKind::Down(MouseButton::Left) => app.click(ev.column, ev.row),
        // Wheel scroll nudges the selection; the view follows it. Three rows a
        // tick is the usual terminal feel.
        MouseEventKind::ScrollDown => app.move_down(3),
        MouseEventKind::ScrollUp => app.move_up(3),
        _ => {}
    }
}

fn normal(app: &mut App, key: KeyEvent) {
    // Half-page scroll (vim Ctrl-d / Ctrl-u). Checked before the digit/chord
    // logic below because the bare keycodes (`d`/`u`) carry other meaning —
    // `d` is a chord starter, and we don't want the modifier swallowed there.
    // Ctrl-u clears the buffer while *editing*; in Normal mode it scrolls, just
    // like vim. Either keypress also abandons an in-progress count.
    if is_ctrl(key, 'd') {
        app.count = None;
        return app.half_page_down();
    }
    if is_ctrl(key, 'u') {
        app.count = None;
        return app.half_page_up();
    }

    // Complete a pending two-key chord (`aa` add row, `dd` delete row).
    if let Some(pending) = app.pending_key.take()
        && let KeyCode::Char(c) = key.code
    {
        match (pending, c) {
            ('a', 'a') => return app.add_row(),
            ('d', 'd') => return app.delete_row(),
            ('g', 'g') => return app.goto_top(),
            _ => {} // not a chord — fall through and handle `key` afresh
        }
    }

    // Accumulate a leading numeric count (vim `5j`). `0` is ambiguous — it is
    // also the "first column" motion — so a `0` only counts when a count is
    // already in progress; a bare `0` falls through to `goto_first_col` below.
    if let KeyCode::Char(c @ '0'..='9') = key.code {
        let digit = (c as u8 - b'0') as usize;
        if app.count.is_some() || digit != 0 {
            app.count = Some(
                app.count
                    .unwrap_or(0)
                    .saturating_mul(10)
                    .saturating_add(digit),
            );
            return;
        }
    }

    // Any non-digit key consumes the count: the next motion repeats by it, and
    // everything else simply discards it. Taken up front so no arm can forget.
    let count = app.count.take();
    let n = count.unwrap_or(1);

    match key.code {
        // Commands are gated behind `:` — e.g. `:q` to quit.
        KeyCode::Char(':') => app.enter_command(),

        // Edit the selected cell — or start `:add` on the phantom column.
        KeyCode::Char('i') | KeyCode::Enter => app.activate(),

        // Chord starters: `aa` adds a row, `dd` deletes one, `gg` jumps to top.
        KeyCode::Char('a') => app.pending_key = Some('a'),
        KeyCode::Char('d') => app.pending_key = Some('d'),
        KeyCode::Char('g') => app.pending_key = Some('g'),

        KeyCode::Down | KeyCode::Char('j') => app.move_down(n),
        KeyCode::Up | KeyCode::Char('k') => app.move_up(n),
        KeyCode::Left | KeyCode::Char('h') => app.move_left_by(n),
        KeyCode::Right | KeyCode::Char('l') => app.move_right_by(n),

        KeyCode::PageDown | KeyCode::Char(' ') => app.page_down(),
        KeyCode::PageUp => app.page_up(),

        KeyCode::Home => app.goto_top(),
        // Bare `G` jumps to the bottom; `<count>G` jumps to that row, using the
        // same 0-based gutter index as `:N` (so `5G` lands on the row labelled 5).
        KeyCode::Char('G') | KeyCode::End => match count {
            Some(row) => app.goto_row(row),
            None => app.goto_bottom(),
        },
        KeyCode::Char('0') | KeyCode::Char('^') => app.goto_first_col(),
        KeyCode::Char('$') => app.goto_last_col(),

        _ => {}
    }
}

fn command(app: &mut App, key: KeyEvent) {
    if is_ctrl(key, 'u') {
        return app.clear_command();
    }
    match key.code {
        KeyCode::Enter => app.run_command(),
        KeyCode::Esc => app.cancel_command(),
        KeyCode::Backspace => app.backspace_command(),
        KeyCode::Char(c) => app.push_command(c),
        _ => {}
    }
}

fn insert(app: &mut App, key: KeyEvent) {
    if is_ctrl(key, 'u') {
        return app.clear_edit();
    }
    match key.code {
        KeyCode::Enter => app.commit_edit(),
        KeyCode::Esc => app.cancel_edit(),
        KeyCode::Backspace => app.backspace_edit(),
        KeyCode::Left => app.move_edit_left(),
        KeyCode::Right => app.move_edit_right(),
        KeyCode::Char(c) => app.push_edit(c),
        _ => {}
    }
}
