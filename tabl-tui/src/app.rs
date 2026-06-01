use std::path::PathBuf;

use tabl_core::{DType, Value};
use tabl_engine::{Format, Sheet, io};

use crate::viewport::Viewport;

/// Modal input, vim-style: navigate in Normal, type into a cell in Insert,
/// enter `:`-style commands (save, convert, quit) in Command.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Mode {
    Normal,
    Insert,
    Command,
}

pub struct App {
    pub sheet: Sheet,
    /// File the sheet was loaded from; `:w` with no argument writes back here.
    pub source: PathBuf,
    pub mode: Mode,
    pub viewport: Viewport,
    pub should_quit: bool,

    /// Text typed after `:` while in Command mode (excludes the leading colon).
    pub command: String,

    /// In-progress text for the cell being edited in Insert mode.
    pub edit: String,

    /// Transient feedback shown in the status bar (e.g. a parse error).
    pub message: Option<String>,

    /// First key of a pending two-key chord (e.g. `a` of `aa`), if any.
    pub pending_key: Option<char>,

    /// Visible data rows / columns, refreshed by the renderer each frame and
    /// read back here to keep the selection on screen while scrolling.
    pub page_rows: usize,
    pub page_cols: usize,
}

impl App {
    pub fn new(sheet: Sheet, source: PathBuf) -> Self {
        Self {
            sheet,
            source,
            mode: Mode::Normal,
            viewport: Viewport::default(),
            should_quit: false,
            command: String::new(),
            edit: String::new(),
            message: None,
            pending_key: None,
            page_rows: 0,
            page_cols: 0,
        }
    }

    fn rows(&self) -> usize {
        self.sheet.shape().0
    }

    fn cols(&self) -> usize {
        self.sheet.shape().1
    }

    pub fn quit(&mut self) {
        self.should_quit = true;
    }

    /// Enter Command mode (`:`), starting with an empty buffer.
    pub fn enter_command(&mut self) {
        self.start_command("");
    }

    /// Enter Command mode with `prefill` already in the buffer.
    fn start_command(&mut self, prefill: &str) {
        self.mode = Mode::Command;
        self.command = prefill.to_string();
        self.message = None;
    }

    pub fn cancel_command(&mut self) {
        self.mode = Mode::Normal;
        self.command.clear();
    }

    pub fn push_command(&mut self, c: char) {
        self.command.push(c);
    }

    pub fn backspace_command(&mut self) {
        // Backspacing past the start cancels the command (vim-like).
        if self.command.pop().is_none() {
            self.cancel_command();
        }
    }

    /// Ctrl+U: clear the command buffer but stay in Command mode.
    pub fn clear_command(&mut self) {
        self.command.clear();
    }

    /// Execute the buffered command and return to Normal mode.
    ///
    /// Supported: `:q`/`:quit`, `:w [path]`/`:write [path]`, `:wq [path]`,
    /// `:add <name> [dtype]`, `:delete <name>`.
    pub fn run_command(&mut self) {
        let input = self.command.trim().to_string();
        self.mode = Mode::Normal;
        self.command.clear();

        // `rest` keeps the raw argument string (spaces and `=` intact) for
        // commands like `:eval`; `args` is the whitespace-split form for the rest.
        let (cmd, rest) = match input.split_once(char::is_whitespace) {
            Some((cmd, rest)) => (cmd, rest.trim()),
            None => (input.as_str(), ""),
        };
        let args: Vec<&str> = rest.split_whitespace().collect();

        // `:N` jumps to row N (the gutter's 0-based number), clamped to range.
        if let Ok(row) = cmd.parse::<usize>() {
            self.goto_row(row);
            return;
        }

        match cmd {
            "" => {}
            "q" | "quit" => self.quit(),
            "w" | "write" => {
                self.write_file(args.first().copied());
            }
            "wq" => {
                if self.write_file(args.first().copied()) {
                    self.quit();
                }
            }
            "add" => self.add_column(&args),
            "delete" | "del" | "drop" => self.delete_column(&args),
            "rename" | "ren" => self.rename_column(&args),
            "eval" => self.eval_column(rest),
            "filter" => self.filter_view(rest),
            other => self.message = Some(format!("unknown command: {other}")),
        }
    }

    /// `:add <name> [dtype]` — insert a new null column immediately left of the
    /// cursor, shifting the current column right. The new column becomes active.
    fn add_column(&mut self, args: &[&str]) {
        let (name, dtype) = match args {
            [name] => (*name, DType::Str),
            [name, dtype] => match parse_dtype(dtype) {
                Some(d) => (*name, d),
                None => {
                    self.message = Some(format!("unknown dtype `{dtype}` (str/int/float/bool)"));
                    return;
                }
            },
            _ => {
                self.message = Some("usage: :add <name> [dtype]".to_string());
                return;
            }
        };

        // Insert at the cursor's index: the new column takes that slot, the old
        // one shifts right, and the (unchanged) selection now points at the new
        // column.
        let index = self.viewport.sel_col;
        match self.sheet.add_column(name, dtype, index) {
            Ok(()) => {
                self.clamp_selection();
                self.message = Some(format!("added column `{name}` ({dtype})"));
            }
            Err(err) => self.message = Some(err.to_string()),
        }
    }

    /// `:delete [name]` — drop the named column, or the active one if no name.
    fn delete_column(&mut self, args: &[&str]) {
        let name = match args {
            [] => match self.sheet.column_meta().get(self.viewport.sel_col) {
                Some(col) => col.name.clone(),
                None => {
                    self.message = Some("no column under the cursor".to_string());
                    return;
                }
            },
            [name] => (*name).to_string(),
            _ => {
                self.message = Some("usage: :delete [name]".to_string());
                return;
            }
        };

        match self.sheet.drop_column(&name) {
            Ok(()) => {
                self.clamp_selection();
                self.message = Some(format!("deleted column `{name}`"));
            }
            Err(err) => self.message = Some(err.to_string()),
        }
    }

    /// `:eval <col> = <expr>` — compute a column from a SQL expression. Overwrites
    /// `col` if it exists, else inserts it at the cursor. The cursor then moves
    /// onto the result column.
    fn eval_column(&mut self, rest: &str) {
        let Some((lhs, rhs)) = rest.split_once('=') else {
            self.message = Some("usage: :eval <col> = <expr>".to_string());
            return;
        };
        let (name, expr) = (lhs.trim(), rhs.trim());
        if name.is_empty() || expr.is_empty() {
            self.message = Some("usage: :eval <col> = <expr>".to_string());
            return;
        }

        // The cursor index is only used when the column is new.
        match self.sheet.eval_column(name, expr, self.viewport.sel_col) {
            Ok(()) => {
                if let Some(i) = self.sheet.column_meta().iter().position(|c| c.name == name) {
                    self.viewport.sel_col = i;
                }
                self.clamp_selection();
                self.message = Some(format!("evaluated `{name}`"));
            }
            Err(err) => self.message = Some(err.to_string()),
        }
    }

    /// `:rename <new>` renames the active column; `:rename <old> <new>` renames
    /// by name.
    fn rename_column(&mut self, args: &[&str]) {
        let (old, new) = match args {
            [new] => match self.sheet.column_meta().get(self.viewport.sel_col) {
                Some(col) => (col.name.clone(), (*new).to_string()),
                None => {
                    self.message = Some("no column under the cursor".to_string());
                    return;
                }
            },
            [old, new] => ((*old).to_string(), (*new).to_string()),
            _ => {
                self.message = Some("usage: :rename [old] <new>".to_string());
                return;
            }
        };

        match self.sheet.rename_column(&old, &new) {
            Ok(()) => self.message = Some(format!("renamed `{old}` → `{new}`")),
            Err(err) => self.message = Some(err.to_string()),
        }
    }

    /// `:filter <clause>` shows only rows matching a boolean SQL expression;
    /// `:filter` with no clause clears it. Never layered — always evaluated
    /// against the full frame.
    fn filter_view(&mut self, rest: &str) {
        let clause = rest.trim();
        let arg = (!clause.is_empty()).then_some(clause);

        match self.sheet.set_filter(arg) {
            Ok(()) => {
                // Reset the view to the top of the (possibly smaller) result.
                self.viewport.sel_row = 0;
                self.viewport.row_offset = 0;
                self.clamp_selection();
                self.message = Some(match arg {
                    Some(_) => format!("filter: {} rows", self.sheet.shape().0),
                    None => "filter cleared".to_string(),
                });
            }
            Err(err) => self.message = Some(err.to_string()),
        }
    }

    /// `aa` — insert a new null row just below the cursor and move onto it.
    /// (A row op clears any active filter, so we work in true-row space.)
    pub fn add_row(&mut self) {
        if self.cols() == 0 {
            self.message = Some("add a column first".to_string());
            return;
        }
        let at = self.sheet.true_row(self.viewport.sel_row) + 1;
        match self.sheet.insert_row(at) {
            Ok(()) => {
                self.viewport.sel_row = at;
                self.clamp_selection();
                self.follow_row();
                self.message = Some("added row".to_string());
            }
            Err(err) => self.message = Some(err.to_string()),
        }
    }

    /// `dd` — delete the row under the cursor.
    pub fn delete_row(&mut self) {
        if self.rows() == 0 {
            self.message = Some("no rows to delete".to_string());
            return;
        }
        // Map to the true row, and re-anchor there since the op clears the filter.
        let true_row = self.sheet.true_row(self.viewport.sel_row);
        self.viewport.sel_row = true_row;
        match self.sheet.delete_row(true_row) {
            Ok(()) => {
                self.clamp_selection();
                self.follow_row();
                self.message = Some("deleted row".to_string());
            }
            Err(err) => self.message = Some(err.to_string()),
        }
    }

    /// Pull the selection (and scroll offsets) back in range after the frame's
    /// shape changes.
    fn clamp_selection(&mut self) {
        let (rows, cols) = self.sheet.shape();
        let vp = &mut self.viewport;

        vp.sel_col = vp.sel_col.min(cols); // `cols` is the phantom "append" slot
        vp.sel_row = vp.sel_row.min(rows.saturating_sub(1));
        vp.col_offset = vp.col_offset.min(vp.sel_col);
        vp.row_offset = vp.row_offset.min(vp.sel_row);
    }

    /// Write the sheet to `arg` if given, else back to the source file. The
    /// output format is inferred from the path's extension, so `:w out.parquet`
    /// also converts. Returns whether the write succeeded.
    fn write_file(&mut self, arg: Option<&str>) -> bool {
        let path = arg
            .map(PathBuf::from)
            .unwrap_or_else(|| self.source.clone());

        let result = Format::from_path(&path).and_then(|fmt| io::save(&self.sheet, fmt, &path));

        match result {
            Ok(()) => {
                self.message = Some(format!("wrote {}", path.display()));
                true
            }
            Err(err) => {
                self.message = Some(format!("write failed: {err}"));
                false
            }
        }
    }

    /// Primary action on the current cell: edit it, or — on the phantom
    /// "append" column — open an `:add ` command awaiting the column name.
    pub fn activate(&mut self) {
        if self.viewport.sel_col >= self.cols() {
            self.start_command("add ");
        } else {
            self.begin_edit();
        }
    }

    /// Start editing the selected cell, seeding the buffer with its current
    /// value so you amend rather than retype; the caret sits at the end.
    /// No-op on an empty sheet or the phantom "append" column.
    pub fn begin_edit(&mut self) {
        let (rows, cols) = self.sheet.shape();
        if rows == 0 || cols == 0 || self.viewport.sel_col >= cols {
            return;
        }
        self.edit = self
            .sheet
            .cell(self.viewport.sel_row, self.viewport.sel_col)
            .display();
        self.mode = Mode::Insert;
        self.message = None;
    }

    pub fn push_edit(&mut self, c: char) {
        self.edit.push(c);
        self.message = None;
    }

    pub fn backspace_edit(&mut self) {
        self.edit.pop();
        self.message = None;
    }

    /// Ctrl+U: clear the edit buffer but stay in Insert mode.
    pub fn clear_edit(&mut self) {
        self.edit.clear();
        self.message = None;
    }

    pub fn cancel_edit(&mut self) {
        self.mode = Mode::Normal;
        self.edit.clear();
        self.message = None;
    }

    /// Parse the buffer against the column's dtype and commit it to the overlay.
    /// On a parse error, stay in Insert mode and surface the message so the user
    /// can fix the input rather than silently corrupting the column.
    pub fn commit_edit(&mut self) {
        let (row, col) = (self.viewport.sel_row, self.viewport.sel_col);
        let dtype = self.sheet.dtype_at(col).unwrap_or(DType::Str);

        match parse_value(&self.edit, dtype) {
            Ok(value) => {
                // `set_cell` maps the display row to the true frame row (filter-aware).
                self.sheet.set_cell(row, col, value);
                self.mode = Mode::Normal;
                self.edit.clear();
                self.message = None;
            }
            Err(err) => self.message = Some(err),
        }
    }

    pub fn move_down(&mut self, n: usize) {
        let rows = self.rows();
        if rows == 0 {
            return;
        }
        self.viewport.sel_row = (self.viewport.sel_row + n).min(rows - 1);
        self.follow_row();
    }

    pub fn move_up(&mut self, n: usize) {
        self.viewport.sel_row = self.viewport.sel_row.saturating_sub(n);
        self.follow_row();
    }

    pub fn move_right(&mut self) {
        // Allow one slot past the last column: the phantom "append" cell.
        let cols = self.cols();
        self.viewport.sel_col = (self.viewport.sel_col + 1).min(cols);
        self.follow_col();
    }

    pub fn move_left(&mut self) {
        self.viewport.sel_col = self.viewport.sel_col.saturating_sub(1);
        self.follow_col();
    }

    pub fn page_down(&mut self) {
        self.move_down(self.page_rows.max(1));
    }

    pub fn page_up(&mut self) {
        self.move_up(self.page_rows.max(1));
    }

    pub fn goto_top(&mut self) {
        self.viewport.sel_row = 0;
        self.follow_row();
    }

    pub fn goto_bottom(&mut self) {
        let rows = self.rows();
        if rows > 0 {
            self.viewport.sel_row = rows - 1;
            self.follow_row();
        }
    }

    /// Jump to a specific row by its (0-based) number, clamped to range.
    pub fn goto_row(&mut self, row: usize) {
        let rows = self.rows();
        if rows == 0 {
            return;
        }
        self.viewport.sel_row = row.min(rows - 1);
        self.follow_row();
    }

    pub fn goto_first_col(&mut self) {
        self.viewport.sel_col = 0;
        self.follow_col();
    }

    pub fn goto_last_col(&mut self) {
        let cols = self.cols();
        if cols > 0 {
            self.viewport.sel_col = cols - 1;
            self.follow_col();
        }
    }

    /// Scroll vertically so the selected row stays within the visible window.
    fn follow_row(&mut self) {
        let page = self.page_rows;
        let vp = &mut self.viewport;
        if vp.sel_row < vp.row_offset {
            vp.row_offset = vp.sel_row;
        } else if page > 0 && vp.sel_row >= vp.row_offset + page {
            vp.row_offset = vp.sel_row + 1 - page;
        }
    }

    /// Scroll horizontally so the selected column stays visible.
    fn follow_col(&mut self) {
        let page = self.page_cols;
        let vp = &mut self.viewport;
        if vp.sel_col < vp.col_offset {
            vp.col_offset = vp.sel_col;
        } else if page > 0 && vp.sel_col >= vp.col_offset + page {
            vp.col_offset = vp.sel_col + 1 - page;
        }
    }
}

/// Parse a dtype name from a `:add` argument.
fn parse_dtype(s: &str) -> Option<DType> {
    match s.to_ascii_lowercase().as_str() {
        "str" | "string" | "text" | "utf8" => Some(DType::Str),
        "int" | "integer" | "i64" => Some(DType::Int),
        "float" | "f64" | "double" | "number" => Some(DType::Float),
        "bool" | "boolean" => Some(DType::Bool),
        "date" => Some(DType::Date),
        "datetime" | "timestamp" => Some(DType::Datetime),
        _ => None,
    }
}

/// Parse typed text into a [`Value`] for a column of the given dtype.
///
/// Blank input means `Null` (the way to clear a cell). String/unknown columns
/// take the text verbatim; numeric and boolean columns reject unparseable input.
fn parse_value(input: &str, dtype: DType) -> Result<Value, String> {
    let trimmed = input.trim();
    if trimmed.is_empty() {
        return Ok(Value::Null);
    }

    match dtype {
        DType::Bool => match trimmed.to_ascii_lowercase().as_str() {
            "true" | "t" | "1" | "yes" | "y" => Ok(Value::Bool(true)),
            "false" | "f" | "0" | "no" | "n" => Ok(Value::Bool(false)),
            _ => Err(format!("expected a boolean, got `{trimmed}`")),
        },
        DType::Int => trimmed
            .parse::<i64>()
            .map(Value::Int)
            .map_err(|_| format!("expected an integer, got `{trimmed}`")),
        DType::Float => trimmed
            .parse::<f64>()
            .map(Value::Float)
            .map_err(|_| format!("expected a number, got `{trimmed}`")),
        DType::Date => tabl_core::value::parse_date(trimmed)
            .map(Value::Date)
            .ok_or_else(|| format!("expected a date YYYY-MM-DD, got `{trimmed}`")),
        DType::Datetime => tabl_core::value::parse_datetime(trimmed)
            .map(Value::Datetime)
            .ok_or_else(|| format!("expected a datetime YYYY-MM-DD HH:MM:SS, got `{trimmed}`")),
        DType::Str | DType::Unknown => Ok(Value::Str(input.to_string())),
    }
}
