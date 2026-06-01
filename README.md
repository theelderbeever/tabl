# tabl

A terminal spreadsheet for data files. `tabl` opens Parquet, CSV, JSON, and
newline-delimited JSON in an interactive grid where you can browse, edit, add and
remove columns and rows, derive columns from expressions, filter, and save back
out — including converting between formats on the way. It is backed by
[polars](https://pola.rs/), so reading and writing real-world files is fast and
the supported formats come for free.

It is meant for the common case where you have a data file on disk and want to
look at it or make a few changes without opening a notebook or a heavyweight
spreadsheet application.

## Installation

Requires a recent Rust toolchain (edition 2024, Rust 1.85 or newer).

```
git clone <repo-url> tabl
cd tabl
cargo build --release
```

The binary is then at `target/release/tabl`. The examples below use `cargo run --`
for convenience; substitute the built binary as needed.

## Usage

### Open a file in the viewer

```
tabl path/to/file.parquet
```

This launches the full-screen TUI. The format is inferred from the file
extension (`.parquet`/`.pq`, `.csv`, `.json`, `.ndjson`/`.jsonl`).

CSV has no type information, so date columns load as text by default. Pass
`-d`/`--parse-dates` to infer date and datetime columns when reading CSV (off by
default, since a column that merely looks date-like can be mis-typed):

```
tabl sample.csv --parse-dates
```

The flag is available on the viewer and on `show`, `describe`, and `convert`. It
has no effect on Parquet, which already carries types.

### Headless commands

```
tabl show <file> [-H N] [-T N]   # print the first N and/or last N rows
tabl describe <file>             # print summary statistics
tabl convert <in> <out>          # convert between formats by extension
```

- `show` with no flags prints the first 10 rows. `-H/--head` and `-T/--tail` can
  be combined; when both are given and they do not overlap, an ellipsis row marks
  the gap.
- `convert` reads `<in>` and writes `<out>`; the output format is taken from the
  `<out>` extension, so `tabl convert data.csv data.parquet` converts.
- `describe` reports count, null count, mean, standard deviation, min, median, and
  max per column (non-numeric statistics are left blank).

## The TUI

The grid shows the data with a two-line header (column name over its type), a
dimmed row-index gutter, and per-column colors to keep wide tables readable. Null
values render as a dimmed `<null>`. The selected cell is highlighted. A trailing
`+` column past the last real column is a navigable slot for appending a new
column.

### Navigation (Normal mode)

| Key                 | Action                          |
| ------------------- | ------------------------------- |
| `h` `j` `k` `l` / arrows | Move the cursor            |
| `g` / `G`           | First / last row                |
| `0` / `$`           | First / last column             |
| PageUp / PageDown / Space | Page up / down            |
| `i` or Enter        | Edit the selected cell          |
| `aa`                | Insert a row below the cursor   |
| `dd`                | Delete the current row          |
| `:`                 | Enter a command                 |

Pressing `i` or Enter on the trailing `+` column starts an `:add` command instead
of editing.

### Editing cells (Insert mode)

`i` or Enter begins editing the selected cell with an empty buffer; typing
replaces the whole value. Enter commits, Esc cancels. The input is parsed
according to the column's type (an integer column rejects non-numbers and keeps
you in Insert mode with an error). An empty value commits as null, which is the
way to clear a cell.

### Commands

Commands are entered after `:` and run on Enter. Esc cancels.

| Command                     | Action                                                       |
| --------------------------- | ------------------------------------------------------------ |
| `:N`                        | Jump to row N (the gutter's 0-based number)                  |
| `:q` / `:quit`              | Quit                                                         |
| `:w [path]` / `:write [path]` | Save; with no path, write back to the opened file          |
| `:wq [path]`                | Save then quit                                               |
| `:add <name> [type]`        | Insert a column left of the cursor (type: `str`/`int`/`float`/`bool`/`date`/`datetime`, default `str`) |
| `:delete [name]` / `:drop`  | Drop the named column, or the active one if no name          |
| `:rename <new>`             | Rename the active column                                     |
| `:rename <old> <new>`       | Rename a column by name                                      |
| `:eval <col> = <expr>`      | Compute a column from a SQL expression                       |
| `:filter <clause>`          | Show only rows matching a boolean SQL expression             |
| `:filter`                   | Clear the filter                                             |

Saving writes by the path's extension, so `:w out.parquet` while viewing a CSV
both saves and converts. Errors (a bad path, an unknown column, a malformed
expression) are reported in the status bar and leave the data unchanged.

### Expressions and filters

`:eval` and `:filter` take SQL expressions, parsed and evaluated by polars. This
means arithmetic, comparisons, functions, and `CASE` all work:

```
:eval total = quantity * unit_price
:eval grade = CASE WHEN score >= 90 THEN 'A' ELSE 'B' END
:filter quantity > 100 AND fulfilled
```

Two SQL conventions apply on the right-hand side: `=` means equality (the first
`=` in an `:eval` is the assignment), and column names containing spaces must be
double-quoted, for example `"unit price"`.

This is also how you convert a text column to a date or datetime, after which it is
recognized as a `date`/`datetime` type:

```
:eval order_date = DATE(order_date)                   # ISO strings -> date
:eval order_date = DATE(order_date, '%m/%d/%Y')       # custom format -> date
:eval ts = STRPTIME(ts, '%Y-%m-%d %H:%M:%S')          # -> datetime
```

A filter is a temporary, non-destructive view. It is always evaluated against the
full data, never layered on a previous filter, and re-running `:filter` with no
clause returns to the full data. Edits made while filtered apply to the
underlying rows. Saving while a filter is active writes only the visible rows, so
filtering and then `:w subset.csv` exports a subset. Adding or deleting a row
clears the filter.

## Generating sample data

The repository's examples use a file produced with the
[`fake`](https://pypi.org/project/fake/) CLI:

```
fake -n 300 \
  -c "order_id,customer,email,department,quantity,unit_price,city,country,order_date,priority,fulfilled" \
  "pyint,name,email,job,pyint,pyfloat,city,country_code,date_this_year,pyint,pybool" \
  > sample.csv
```

Then `cargo run -- sample.csv` to open it, or `cargo run -- describe sample.csv`.

## How it works

The project is a Cargo workspace split into three library crates and a binary,
chosen so that the heavy dependency stays isolated:

- `tabl-core` — the data model with no polars dependency: cell values, column
  types, the pending-edit overlay, and the render snapshot. This keeps the model
  light and lets the UI depend on it without pulling in polars.
- `tabl-engine` — the only crate that depends on polars. It loads and saves files,
  evaluates expressions and filters, and translates between polars types and the
  core model.
- `tabl-tui` — the ratatui/crossterm front end: application state, key handling,
  and rendering. It talks to the engine through the `Sheet` type and never touches
  polars directly.
- `tabl` (the binary) — argument parsing and the headless `show`/`describe`/
  `convert` subcommands.

Cell edits are kept in an overlay rather than mutating the data frame in place;
they are applied when you save, and structural changes (adding or removing columns
and rows, evaluating a column) bake the overlay in first so indices stay
consistent. The filter is a separate view that maps displayed rows back to the
underlying rows, so it never mutates the data.

## Status

`tabl` is a working tool but young. The data model covers booleans, integers,
floats, strings, dates, and datetimes directly; remaining polars types (lists,
structs, durations, decimals) are read and displayed as text. Datetimes are
normalized to microseconds for display and editing. JSON input is expected to be
flat records or newline-delimited; nested JSON does not map onto a grid. There is
no undo for structural changes yet.

## Development

```
cargo test --workspace      # run the test suite
cargo clippy --workspace    # lint
```
