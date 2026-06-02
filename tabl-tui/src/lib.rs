//! Ratatui + crossterm front end. Talks to the engine through `Sheet` and
//! renders `tabl-core` snapshots; never touches polars directly.

pub mod app;
pub mod event;
pub mod ui;
pub mod viewport;

use std::{io::stdout, path::PathBuf};

use crossterm::{
    event::{DisableMouseCapture, EnableMouseCapture, Event, KeyEventKind},
    execute,
};
use ratatui::DefaultTerminal;
use tabl_core::{Error, Result};
use tabl_engine::Sheet;

use crate::app::App;

fn io_err(e: std::io::Error) -> Error {
    Error::Io(e.to_string())
}

/// Set up the terminal, run the event loop against `sheet`, and restore the
/// terminal on exit. `ratatui::init` installs a panic hook that restores the
/// terminal too, so a panic mid-loop won't leave the screen wedged.
pub fn run(sheet: Sheet, source: PathBuf) -> Result<()> {
    let mut terminal = ratatui::init();
    // Mouse reporting isn't on by default; opt in so we can navigate by click.
    // It's an enhancement — if the terminal rejects it, carry on keyboard-only.
    let _ = execute!(stdout(), EnableMouseCapture);
    let mut app = App::new(sheet, source);
    let result = event_loop(&mut terminal, &mut app);
    let _ = execute!(stdout(), DisableMouseCapture);
    ratatui::restore();
    result
}

fn event_loop(terminal: &mut DefaultTerminal, app: &mut App) -> Result<()> {
    loop {
        terminal
            .draw(|frame| ui::draw(frame, app))
            .map_err(io_err)?;
        if app.should_quit {
            break;
        }
        match crossterm::event::read().map_err(io_err)? {
            // Filter to key *presses* — some terminals also emit release/repeat.
            Event::Key(key) if key.kind == KeyEventKind::Press => event::handle_key(app, key),
            Event::Mouse(m) => event::handle_mouse(app, m),
            _ => {}
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::app::Mode;
    use crossterm::event::{
        KeyCode, KeyEvent, KeyModifiers, MouseButton, MouseEvent, MouseEventKind,
    };
    use ratatui::{Terminal, backend::TestBackend};
    use std::io::Write;
    use tabl_core::Value;

    fn press(code: KeyCode) -> KeyEvent {
        KeyEvent::new(code, KeyModifiers::NONE)
    }

    fn load_sheet(name: &str, contents: &str) -> Sheet {
        let path = std::env::temp_dir().join(name);
        let mut f = std::fs::File::create(&path).unwrap();
        write!(f, "{contents}").unwrap();
        let sheet = tabl_engine::io::load(&path).unwrap();
        let _ = std::fs::remove_file(&path);
        sheet
    }

    fn buffer_text(terminal: &Terminal<TestBackend>, w: u16, h: u16) -> String {
        let buf = terminal.backend().buffer();
        let mut text = String::new();
        for y in 0..h {
            for x in 0..w {
                if let Some(cell) = buf.cell((x, y)) {
                    text.push_str(cell.symbol());
                }
            }
            text.push('\n');
        }
        text
    }

    #[test]
    fn renders_header_cells_and_status() {
        let sheet = load_sheet("tabl_tui_render.csv", "id,name\n1,alice\n2,bob\n");
        let mut app = App::new(sheet, "test.csv".into());

        let mut terminal = Terminal::new(TestBackend::new(40, 6)).unwrap();
        terminal.draw(|frame| ui::draw(frame, &mut app)).unwrap();

        let text = buffer_text(&terminal, 40, 6);
        assert!(text.contains("name"), "header missing in:\n{text}");
        assert!(text.contains("alice"), "cell missing in:\n{text}");
        assert!(text.contains("NORMAL"), "status missing in:\n{text}");
        // The dtype row renders under the column names.
        assert!(text.contains("int"), "id dtype missing in:\n{text}");
        assert!(text.contains("str"), "name dtype missing in:\n{text}");

        // The renderer fed the visible dimensions back to the app: 2 real
        // columns plus the trailing phantom "append" slot.
        assert!(app.page_rows > 0);
        assert_eq!(app.page_cols, 3);
    }

    #[test]
    fn crosshair_marks_current_row_and_column() {
        use ratatui::style::{Color, Modifier};

        let sheet = load_sheet("tabl_tui_crosshair.csv", "a,b,c\n0,1,2\n3,4,5\n6,7,8\n");
        let mut app = App::new(sheet, "test.csv".into());
        // Move the cursor off the origin so the crosshair has clear arms.
        event::handle_key(&mut app, press(KeyCode::Char('j')));
        event::handle_key(&mut app, press(KeyCode::Char('l')));
        assert_eq!(app.viewport.sel_row, 1);
        assert_eq!(app.viewport.sel_col, 1);

        let (w, h) = (40u16, 8u16);
        let mut terminal = Terminal::new(TestBackend::new(w, h)).unwrap();
        terminal.draw(|frame| ui::draw(frame, &mut app)).unwrap();
        let buf = terminal.backend().buffer().clone();

        let on_crosshair = |x: u16, y: u16| -> bool {
            buf.cell((x, y))
                .map(|c| c.style().bg == Some(ui::table::CROSSHAIR_BG))
                .unwrap_or(false)
        };
        let reversed = |x: u16, y: u16| -> bool {
            buf.cell((x, y))
                .map(|c| c.style().add_modifier.contains(Modifier::REVERSED))
                .unwrap_or(false)
        };

        // Every painted column of the table area falls into one of three buckets:
        // the selected cell (reversed), a crosshair arm (tinted), or neither. We
        // assert all three actually occur, which means the crosshair is drawn and
        // the selected cell stays distinct from it.
        let mut saw_selected = false;
        let mut saw_crosshair = false;
        let mut saw_plain = false;
        for y in 0..h {
            for x in 0..w {
                let cell = buf.cell((x, y)).unwrap();
                if cell.symbol() == " " && cell.style().bg.is_none() {
                    continue; // unpainted background
                }
                if reversed(x, y) {
                    saw_selected = true;
                    // The selected cell must not also be tinted — it owns the
                    // intersection with the stronger reversed highlight.
                    assert!(
                        cell.style().bg != Some(ui::table::CROSSHAIR_BG),
                        "selected cell at ({x},{y}) should not carry the crosshair tint"
                    );
                } else if on_crosshair(x, y) {
                    saw_crosshair = true;
                } else {
                    saw_plain = true;
                }
            }
        }
        assert!(saw_selected, "expected a reversed selected cell");
        assert!(saw_crosshair, "expected crosshair-tinted cells");
        assert!(saw_plain, "expected untinted cells off the crosshair");

        // The row arm must be unbroken across column boundaries — the bug this
        // fixes was the inter-column spacing showing through untinted, leaving
        // the row arm segmented. Find the selected row by its reversed cell,
        // then walk that whole line: every column must be tinted or reversed,
        // with no plain gap in between.
        let sel_y = (0..h)
            .find(|&y| (0..w).any(|x| reversed(x, y)))
            .expect("a reversed cell marks the selected row");
        let first = (0..w).find(|&x| on_crosshair(x, sel_y) || reversed(x, sel_y));
        let last = (0..w)
            .rev()
            .find(|&x| on_crosshair(x, sel_y) || reversed(x, sel_y));
        let (first, last) = (first.unwrap(), last.unwrap());
        for x in first..=last {
            assert!(
                on_crosshair(x, sel_y) || reversed(x, sel_y),
                "gap in the row arm at ({x},{sel_y}): the column spacing was left untinted"
            );
        }

        // Guard against the tint defaulting to the same value as no-color: the
        // crosshair must be a real background distinct from the unset default.
        assert_ne!(ui::table::CROSSHAIR_BG, Color::Reset);
    }

    #[test]
    fn ctrl_u_clears_buffer_in_both_modes() {
        let ctrl_u = KeyEvent::new(KeyCode::Char('u'), KeyModifiers::CONTROL);
        let sheet = load_sheet("tabl_tui_ctrlu.csv", "n\n7\n");
        let mut app = App::new(sheet, "test.csv".into());

        // Command mode: type then clear, staying in Command mode.
        event::handle_key(&mut app, press(KeyCode::Char(':')));
        for ch in "filter x".chars() {
            event::handle_key(&mut app, press(KeyCode::Char(ch)));
        }
        event::handle_key(&mut app, ctrl_u);
        assert_eq!(app.mode, Mode::Command);
        assert_eq!(app.command, "");

        event::handle_key(&mut app, press(KeyCode::Esc));

        // Insert mode: buffer seeds with "7", Ctrl+U clears it, still editing.
        event::handle_key(&mut app, press(KeyCode::Char('i')));
        assert_eq!(app.edit, "7");
        event::handle_key(&mut app, ctrl_u);
        assert_eq!(app.mode, Mode::Insert);
        assert_eq!(app.edit, "");
    }

    #[test]
    fn gg_jumps_to_top_g_alone_is_a_prefix() {
        let sheet = load_sheet("tabl_tui_gg.csv", "a\n0\n1\n2\n3\n");
        let mut app = App::new(sheet, "test.csv".into());
        app.page_rows = 10;
        app.goto_bottom();
        assert_eq!(app.viewport.sel_row, 3);

        // `gg` returns to the top.
        event::handle_key(&mut app, press(KeyCode::Char('g')));
        assert_eq!(app.pending_key, Some('g'));
        event::handle_key(&mut app, press(KeyCode::Char('g')));
        assert_eq!(app.viewport.sel_row, 0);

        // `g` then a motion is not a chord — the motion still happens.
        event::handle_key(&mut app, press(KeyCode::Char('g')));
        event::handle_key(&mut app, press(KeyCode::Char('j')));
        assert_eq!(app.pending_key, None);
        assert_eq!(app.viewport.sel_row, 1);
    }

    #[test]
    fn colon_number_jumps_to_row() {
        let sheet = load_sheet("tabl_tui_goto.csv", "a\n0\n1\n2\n3\n4\n");
        let mut app = App::new(sheet, "test.csv".into());
        app.page_rows = 3;

        run_cmd(&mut app, "3");
        assert_eq!(app.viewport.sel_row, 3);

        // Out of range clamps to the last row.
        run_cmd(&mut app, "999");
        assert_eq!(app.viewport.sel_row, 4);

        run_cmd(&mut app, "0");
        assert_eq!(app.viewport.sel_row, 0);
    }

    #[test]
    fn quit_is_gated_behind_colon_q() {
        let sheet = load_sheet("tabl_tui_quit.csv", "a\n1\n");
        let mut app = App::new(sheet, "test.csv".into());

        // Bare `q` no longer quits.
        event::handle_key(&mut app, press(KeyCode::Char('q')));
        assert!(!app.should_quit);

        // `:` enters Command mode, `q` buffers, Enter executes.
        event::handle_key(&mut app, press(KeyCode::Char(':')));
        assert_eq!(app.mode, Mode::Command);
        event::handle_key(&mut app, press(KeyCode::Char('q')));
        assert_eq!(app.command, "q");
        event::handle_key(&mut app, press(KeyCode::Enter));
        assert!(app.should_quit);
        assert_eq!(app.mode, Mode::Normal);
    }

    #[test]
    fn insert_commits_edit_to_overlay() {
        // Single int column; selection starts at (0, 0).
        let sheet = load_sheet("tabl_tui_edit.csv", "n\n1\n2\n");
        let mut app = App::new(sheet, "test.csv".into());

        event::handle_key(&mut app, press(KeyCode::Char('i')));
        assert_eq!(app.mode, Mode::Insert);
        assert_eq!(app.edit, "1", "buffer seeds with the current value");

        // Clear it and type "42".
        event::handle_key(&mut app, press(KeyCode::Backspace));
        event::handle_key(&mut app, press(KeyCode::Char('4')));
        event::handle_key(&mut app, press(KeyCode::Char('2')));
        event::handle_key(&mut app, press(KeyCode::Enter));

        assert_eq!(app.mode, Mode::Normal);
        assert_eq!(app.sheet.view(0, 1).rows[0][0], Value::Int(42));
    }

    #[test]
    fn insert_rejects_bad_value_and_stays_editing() {
        let sheet = load_sheet("tabl_tui_bad.csv", "n\n1\n");
        let mut app = App::new(sheet, "test.csv".into());

        event::handle_key(&mut app, press(KeyCode::Char('i')));
        event::handle_key(&mut app, press(KeyCode::Backspace));
        event::handle_key(&mut app, press(KeyCode::Char('x'))); // not an integer
        event::handle_key(&mut app, press(KeyCode::Enter));

        assert_eq!(app.mode, Mode::Insert, "stays in Insert on parse error");
        assert!(app.message.is_some(), "surfaces an error message");
        // The cell is untouched.
        assert_eq!(app.sheet.view(0, 1).rows[0][0], Value::Int(1));

        // Esc abandons the edit cleanly.
        event::handle_key(&mut app, press(KeyCode::Esc));
        assert_eq!(app.mode, Mode::Normal);
        assert!(app.message.is_none());
    }

    #[test]
    fn write_command_persists_edits_to_given_path() {
        let sheet = load_sheet("tabl_tui_w_in.csv", "n\n1\n2\n");
        let out = std::env::temp_dir().join("tabl_tui_w_out.csv");
        let mut app = App::new(sheet, "ignored.csv".into());

        // Edit (0, 0): 1 -> 9.
        event::handle_key(&mut app, press(KeyCode::Char('i')));
        event::handle_key(&mut app, press(KeyCode::Backspace));
        event::handle_key(&mut app, press(KeyCode::Char('9')));
        event::handle_key(&mut app, press(KeyCode::Enter));

        // `:w <out>`
        event::handle_key(&mut app, press(KeyCode::Char(':')));
        for ch in format!("w {}", out.display()).chars() {
            event::handle_key(&mut app, press(KeyCode::Char(ch)));
        }
        event::handle_key(&mut app, press(KeyCode::Enter));

        assert_eq!(app.mode, Mode::Normal);
        assert!(
            app.message.as_deref().unwrap_or("").contains("wrote"),
            "expected a write confirmation, got {:?}",
            app.message
        );

        // Reload the written file and confirm the edit landed on disk.
        let reloaded = tabl_engine::io::load(&out).unwrap();
        assert_eq!(reloaded.cell(0, 0), Value::Int(9));
        let _ = std::fs::remove_file(&out);
    }

    fn run_cmd(app: &mut App, cmd: &str) {
        event::handle_key(app, press(KeyCode::Char(':')));
        for ch in cmd.chars() {
            event::handle_key(app, press(KeyCode::Char(ch)));
        }
        event::handle_key(app, press(KeyCode::Enter));
    }

    #[test]
    fn add_and_delete_columns_via_commands() {
        let sheet = load_sheet("tabl_tui_cols.csv", "a,b\n1,x\n2,y\n");
        let mut app = App::new(sheet, "test.csv".into());
        // Cursor starts on column 0 ("a").

        // `add` inserts left of the cursor; the new column becomes active.
        run_cmd(&mut app, "add id int");
        assert_eq!(app.sheet.shape(), (2, 3));
        let meta = app.sheet.column_meta();
        assert_eq!(meta[0].name, "id"); // took the cursor's slot
        assert_eq!(meta[0].dtype, tabl_core::DType::Int);
        assert_eq!(meta[1].name, "a"); // shifted right
        assert_eq!(app.viewport.sel_col, 0, "selection follows the new column");

        // Move onto "a" (now index 1) and add again; omitted dtype → str.
        app.move_right();
        assert_eq!(app.viewport.sel_col, 1);
        run_cmd(&mut app, "add mid");
        let meta = app.sheet.column_meta();
        assert_eq!(meta[1].name, "mid");
        assert_eq!(meta[1].dtype, tabl_core::DType::Str);
        assert_eq!(meta[2].name, "a"); // shifted right again
        assert_eq!(app.viewport.sel_col, 1);

        // Delete by name.
        run_cmd(&mut app, "delete b");
        assert_eq!(app.sheet.shape(), (2, 3));
        assert!(app.sheet.column_meta().iter().all(|c| c.name != "b"));

        // Unknown dtype is reported, sheet unchanged.
        let before = app.sheet.shape();
        run_cmd(&mut app, "add oops notatype");
        assert_eq!(app.sheet.shape(), before);
        assert!(app.message.as_deref().unwrap().contains("unknown dtype"));
    }

    #[test]
    fn delete_without_name_drops_active_column() {
        let sheet = load_sheet("tabl_tui_delactive.csv", "a,b,c\n1,x,9\n");
        let mut app = App::new(sheet, "test.csv".into());

        app.move_right(); // cursor on "b"
        assert_eq!(app.viewport.sel_col, 1);

        run_cmd(&mut app, "delete");
        assert_eq!(app.sheet.shape(), (1, 2));
        let names: Vec<_> = app
            .sheet
            .column_meta()
            .into_iter()
            .map(|c| c.name)
            .collect();
        assert_eq!(names, vec!["a", "c"]);
    }

    #[test]
    fn add_at_end_via_phantom_column() {
        let sheet = load_sheet("tabl_tui_phantom.csv", "a,b\n1,x\n2,y\n");
        let mut app = App::new(sheet, "test.csv".into());

        // Navigate past the last real column onto the phantom slot.
        app.move_right(); // -> b (1)
        app.move_right(); // -> phantom (2)
        assert_eq!(app.viewport.sel_col, 2);

        // Adding here appends at the far right; the new column becomes active.
        run_cmd(&mut app, "add z int");
        assert_eq!(app.sheet.shape(), (2, 3));
        let meta = app.sheet.column_meta();
        assert_eq!(meta[2].name, "z");
        assert_eq!(app.viewport.sel_col, 2, "selection lands on the new column");
    }

    #[test]
    fn enter_on_phantom_opens_add_command() {
        let sheet = load_sheet("tabl_tui_enter_phantom.csv", "a,b\n1,x\n");
        let mut app = App::new(sheet, "test.csv".into());

        app.move_right(); // -> b
        app.move_right(); // -> phantom
        assert_eq!(app.viewport.sel_col, 2);

        // Enter on the phantom drops into Command mode pre-filled with `add `.
        event::handle_key(&mut app, press(KeyCode::Enter));
        assert_eq!(app.mode, Mode::Command);
        assert_eq!(app.command, "add ");

        // Type the rest and run it.
        for ch in "z int".chars() {
            event::handle_key(&mut app, press(KeyCode::Char(ch)));
        }
        event::handle_key(&mut app, press(KeyCode::Enter));

        assert_eq!(app.sheet.shape(), (1, 3));
        assert_eq!(app.sheet.column_meta()[2].name, "z");
    }

    #[test]
    fn null_cells_render_sentinel() {
        let sheet = load_sheet("tabl_tui_null.csv", "a\n1\n2\n");
        let mut app = App::new(sheet, "test.csv".into());
        // A freshly added column is all-null.
        run_cmd(&mut app, "add c int");

        let mut terminal = Terminal::new(TestBackend::new(40, 8)).unwrap();
        terminal.draw(|frame| ui::draw(frame, &mut app)).unwrap();

        let text = buffer_text(&terminal, 40, 8);
        assert!(text.contains("<null>"), "null sentinel missing in:\n{text}");
    }

    #[test]
    fn eval_adds_overwrites_and_reports_errors() {
        let sheet = load_sheet("tabl_tui_eval.csv", "a,b\n1,10\n2,20\n");
        let mut app = App::new(sheet, "test.csv".into());
        // Cursor on column 0 ("a").

        // New column computed from others, inserted at the cursor; selection
        // follows onto it.
        run_cmd(&mut app, "eval c = a * 2 + b");
        assert_eq!(app.sheet.shape(), (2, 3));
        let c = app
            .sheet
            .column_meta()
            .iter()
            .position(|m| m.name == "c")
            .unwrap();
        assert_eq!(
            app.viewport.sel_col, c,
            "selection follows the result column"
        );
        assert_eq!(app.sheet.cell(0, c), Value::Int(12)); // 1*2 + 10

        // Overwriting an existing column keeps the shape.
        run_cmd(&mut app, "eval a = a + 100");
        assert_eq!(app.sheet.shape(), (2, 3));
        let a = app
            .sheet
            .column_meta()
            .iter()
            .position(|m| m.name == "a")
            .unwrap();
        assert_eq!(app.sheet.cell(0, a), Value::Int(101));

        // A malformed expression reports an error and leaves the sheet unchanged.
        let before = app.sheet.shape();
        run_cmd(&mut app, "eval bad = nope + 1");
        assert_eq!(app.sheet.shape(), before);
        assert!(app.message.is_some());

        // Missing `=` is a usage error.
        run_cmd(&mut app, "eval whoops");
        assert!(app.message.as_deref().unwrap().contains("usage"));
    }

    #[test]
    fn filter_is_a_toggleable_temporary_view() {
        let sheet = load_sheet("tabl_tui_filter.csv", "a,b\n1,x\n2,y\n3,z\n4,w\n");
        let mut app = App::new(sheet, "test.csv".into());

        // Filter to a > 2 → 2 rows; selection/scroll reset to top.
        run_cmd(&mut app, "filter a > 2");
        assert!(app.sheet.is_filtered());
        assert_eq!(app.sheet.shape().0, 2);
        assert_eq!(app.viewport.sel_row, 0);
        assert_eq!(app.sheet.cell(0, 0), Value::Int(3)); // first matching row

        // Re-filtering is evaluated against the full frame, never layered.
        run_cmd(&mut app, "filter a > 1");
        assert_eq!(app.sheet.shape().0, 3);

        // Bare `:filter` clears it.
        run_cmd(&mut app, "filter");
        assert!(!app.sheet.is_filtered());
        assert_eq!(app.sheet.shape().0, 4);
    }

    #[test]
    fn write_while_filtered_saves_only_visible_rows() {
        let sheet = load_sheet("tabl_tui_fw_in.csv", "a\n1\n2\n3\n4\n");
        let out = std::env::temp_dir().join("tabl_tui_fw_out.csv");
        let mut app = App::new(sheet, "ignored.csv".into());

        run_cmd(&mut app, "filter a > 2");
        run_cmd(&mut app, &format!("w {}", out.display()));

        let reloaded = tabl_engine::io::load(&out).unwrap();
        assert_eq!(reloaded.shape().0, 2, "only the filtered rows were written");
        assert_eq!(reloaded.cell(0, 0), Value::Int(3));
        assert_eq!(reloaded.cell(1, 0), Value::Int(4));
        let _ = std::fs::remove_file(&out);
    }

    #[test]
    fn add_and_edit_a_date_column() {
        let sheet = load_sheet("tabl_tui_date.csv", "a\n1\n2\n");
        let mut app = App::new(sheet, "test.csv".into());

        // New date column at the cursor.
        run_cmd(&mut app, "add when date");
        let when = app
            .sheet
            .column_meta()
            .iter()
            .position(|c| c.name == "when")
            .unwrap();
        assert_eq!(app.sheet.column_meta()[when].dtype, tabl_core::DType::Date);

        // Edit it by typing a date; it parses and renders back formatted.
        app.viewport.sel_col = when;
        event::handle_key(&mut app, press(KeyCode::Char('i')));
        for ch in "2026-07-04".chars() {
            event::handle_key(&mut app, press(KeyCode::Char(ch)));
        }
        event::handle_key(&mut app, press(KeyCode::Enter));
        assert_eq!(app.mode, Mode::Normal);
        assert_eq!(app.sheet.cell(0, when).display(), "2026-07-04");

        // A malformed date is rejected and keeps you editing.
        event::handle_key(&mut app, press(KeyCode::Char('i')));
        for ch in "nope".chars() {
            event::handle_key(&mut app, press(KeyCode::Char(ch)));
        }
        event::handle_key(&mut app, press(KeyCode::Enter));
        assert_eq!(app.mode, Mode::Insert);
        assert!(app.message.is_some());
    }

    #[test]
    fn rename_active_and_named_columns() {
        let sheet = load_sheet("tabl_tui_rename.csv", "a,b\n1,2\n");
        let mut app = App::new(sheet, "test.csv".into());

        // No-arg renames the active column (cursor on 0 = "a").
        run_cmd(&mut app, "rename id");
        assert_eq!(app.sheet.column_meta()[0].name, "id");

        // Two-arg renames by name.
        run_cmd(&mut app, "rename b value");
        assert_eq!(app.sheet.column_meta()[1].name, "value");

        // Renaming onto an existing name is rejected.
        run_cmd(&mut app, "rename value id");
        assert!(app.message.as_deref().unwrap().contains("already exists"));
    }

    #[test]
    fn aa_adds_row_and_dd_deletes_it() {
        let sheet = load_sheet("tabl_tui_rows.csv", "a\n1\n2\n3\n");
        let mut app = App::new(sheet, "test.csv".into());
        app.page_rows = 10;
        // Cursor on row 0.

        // `aa` inserts a null row below and moves onto it.
        event::handle_key(&mut app, press(KeyCode::Char('a')));
        event::handle_key(&mut app, press(KeyCode::Char('a')));
        assert_eq!(app.sheet.shape(), (4, 1));
        assert_eq!(app.viewport.sel_row, 1);
        assert!(app.sheet.cell(1, 0).is_null());

        // `dd` deletes the current (null) row, restoring the original.
        event::handle_key(&mut app, press(KeyCode::Char('d')));
        event::handle_key(&mut app, press(KeyCode::Char('d')));
        assert_eq!(app.sheet.shape(), (3, 1));
        assert_eq!(app.sheet.cell(1, 0), Value::Int(2));
    }

    #[test]
    fn incomplete_chord_falls_through() {
        let sheet = load_sheet("tabl_tui_chord.csv", "a\n1\n2\n");
        let mut app = App::new(sheet, "test.csv".into());
        app.page_rows = 10;

        // `a` then `j`: not a chord — `j` moves down, no row added.
        event::handle_key(&mut app, press(KeyCode::Char('a')));
        assert_eq!(app.pending_key, Some('a'));
        event::handle_key(&mut app, press(KeyCode::Char('j')));
        assert_eq!(app.pending_key, None);
        assert_eq!(app.sheet.shape(), (2, 1), "no row added");
        assert_eq!(app.viewport.sel_row, 1, "moved down instead");
    }

    fn left_click(column: u16, row: u16) -> MouseEvent {
        MouseEvent {
            kind: MouseEventKind::Down(MouseButton::Left),
            column,
            row,
            modifiers: KeyModifiers::NONE,
        }
    }

    #[test]
    fn left_click_moves_selection_to_the_clicked_cell() {
        let sheet = load_sheet("tabl_tui_click.csv", "a,b,c\n0,1,2\n3,4,5\n6,7,8\n");
        let mut app = App::new(sheet, "test.csv".into());

        // Draw once so the renderer captures the grid geometry the click maps
        // against. Layout at width 40: 1-wide gutter, then columns a/b/c each
        // 3 wide ("int" dtype is the widest) separated by one space — a at x=2,
        // b at x=6, c at x=10. Data rows begin at y=2 (below the 2-line header).
        let mut terminal = Terminal::new(TestBackend::new(40, 8)).unwrap();
        terminal.draw(|frame| ui::draw(frame, &mut app)).unwrap();

        // Click inside column b on the second data row.
        event::handle_mouse(&mut app, left_click(7, 3));
        assert_eq!(app.viewport.sel_row, 1);
        assert_eq!(app.viewport.sel_col, 1);

        // A click in the header band selects nothing — the cursor stays put.
        event::handle_mouse(&mut app, left_click(7, 0));
        assert_eq!(app.viewport.sel_row, 1);
        assert_eq!(app.viewport.sel_col, 1);

        // Nor does a click below the last data row (only 3 rows are present).
        event::handle_mouse(&mut app, left_click(7, 6));
        assert_eq!(app.viewport.sel_row, 1);
        assert_eq!(app.viewport.sel_col, 1);
    }

    #[test]
    fn mouse_is_ignored_outside_normal_mode() {
        let sheet = load_sheet("tabl_tui_click_insert.csv", "a,b\n0,1\n2,3\n");
        let mut app = App::new(sheet, "test.csv".into());
        let mut terminal = Terminal::new(TestBackend::new(40, 8)).unwrap();
        terminal.draw(|frame| ui::draw(frame, &mut app)).unwrap();

        // Begin editing, then click elsewhere: the edit must survive untouched.
        event::handle_key(&mut app, press(KeyCode::Char('i')));
        assert_eq!(app.mode, Mode::Insert);
        event::handle_mouse(&mut app, left_click(7, 3));
        assert_eq!(app.mode, Mode::Insert, "click must not exit Insert");
        assert_eq!(app.viewport.sel_row, 0, "selection unchanged while editing");
        assert_eq!(app.viewport.sel_col, 0);
    }

    #[test]
    fn scroll_wheel_moves_the_selection() {
        let sheet = load_sheet("tabl_tui_scroll.csv", "a\n0\n1\n2\n3\n4\n5\n6\n7\n8\n");
        let mut app = App::new(sheet, "test.csv".into());
        app.page_rows = 20; // room for every row, so the wheel just moves the cursor

        event::handle_mouse(&mut app, scroll(MouseEventKind::ScrollDown));
        assert_eq!(app.viewport.sel_row, 3, "scroll down nudges three rows");

        event::handle_mouse(&mut app, scroll(MouseEventKind::ScrollUp));
        assert_eq!(app.viewport.sel_row, 0);
    }

    fn scroll(kind: MouseEventKind) -> MouseEvent {
        MouseEvent {
            kind,
            column: 0,
            row: 0,
            modifiers: KeyModifiers::NONE,
        }
    }

    #[test]
    fn navigation_clamps_to_bounds() {
        let sheet = load_sheet("tabl_tui_nav.csv", "a\n1\n2\n3\n");
        let mut app = App::new(sheet, "test.csv".into());
        app.page_rows = 10;

        app.move_up(5);
        assert_eq!(app.viewport.sel_row, 0);

        app.move_down(100);
        assert_eq!(app.viewport.sel_row, 2, "should clamp to last row");

        // One column → right moves onto the phantom slot (index 1), then stops.
        app.move_right();
        assert_eq!(app.viewport.sel_col, 1, "moves onto the phantom slot");
        app.move_right();
        assert_eq!(app.viewport.sel_col, 1, "can't go past the phantom slot");
    }
}
