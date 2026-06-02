# CLAUDE.md

Orientation for agents working in this repo. For the user-facing feature set
(every key binding, every `:command`, expression/filter semantics) read
`README.md` — it is thorough and kept current; don't duplicate it here.

## What this is

`tabl` is a terminal spreadsheet for data files: it opens Parquet/CSV/JSON/NDJSON
in an interactive ratatui grid you can browse and edit, and also runs headless
(`show`/`describe`/`convert`). Polars does the heavy lifting (read/write,
SQL expressions, filters).

## Workspace layout and the one invariant that matters

A Cargo workspace: three library crates plus the `tabl` binary at the root. The
crates are layered specifically to **quarantine the polars dependency**:

- **`tabl-core`** — polars-free domain model. `Value`/`DType` (cell values),
  `ColumnMeta`, `EditOverlay`/`CellAddr` (pending edits), `Snapshot` (a
  render-ready row window). Light, no heavy deps.
- **`tabl-engine`** — the **only** crate that depends on polars. `Sheet` wraps an
  immutable `DataFrame` + edit overlay + optional filter; `io` loads/saves;
  `convert`/`format` handle formats. Maps polars types ↔ `tabl-core` types.
- **`tabl-tui`** — ratatui/crossterm front end: `App` state, key handling
  (`event`), rendering (`ui/`). Talks to the engine through `Sheet` and renders
  `Snapshot`s.
- **`tabl` (root `src/`)** — `main.rs` (clap CLI) + `show.rs` (headless printing).

**Invariant: only `tabl-engine` may `use polars`.** `tabl-tui` and the binary
speak `tabl-core` types exclusively. If you find yourself reaching for a polars
type above the engine, add a method to `Sheet` instead.

## Core architecture concepts

- **Edit overlay, not in-place mutation.** Cell edits live in `EditOverlay`
  (a `HashMap<CellAddr, Value>`) layered over the read-only `DataFrame`. They're
  baked into a new frame only on save (`materialize`) — this decouples edit
  latency from frame size. Structural ops (add/drop column, insert/delete row,
  `eval`) materialize the overlay first so indices stay consistent. No undo yet.
- **Filter is a display map, not a mutation.** `Sheet.filter: Option<Vec<usize>>`
  maps display-row → true frame-row. Always evaluated against the full frame,
  never layered. Saving while filtered writes only visible rows
  (`materialize_view`). `true_row()` does the translation.
- **`Snapshot` is a windowed view.** `Sheet::view(offset, len)` returns only the
  rows in `[offset, offset+len)` with the overlay applied — the UI never holds
  the whole frame. `Snapshot.rows` is row-major (`rows[r][c]`).
- **Temporal values** are stored polars-style: `Date` = days since epoch,
  `Datetime` = microseconds since epoch. Conversion helpers live in
  `tabl-core/src/value.rs`.

## TUI specifics

- Three modes: `Normal`, `Command`, `Insert` (`app::Mode`). Key dispatch is in
  `event.rs` (one fn per mode); state transitions are methods on `App` in
  `app.rs`.
- **Commands** (`:q`, `:add`, `:eval`, `:filter`, …) are parsed in
  `App::run_command` — that match is the source of truth for the command surface.
  `:N` jumps to a row. Commands are gated behind `:`; bare `q` does not quit.
- **Chords**: `aa` (add row), `dd` (delete row), `gg` (top) via `app.pending_key`.
- **Rendering** is in `ui/table.rs`. ratatui's `Table` does **not** scroll columns
  horizontally, so we window columns ourselves from `Viewport.col_offset` and
  return the visible count back to `App`. Header is two lines (name + dtype), so
  `page_rows = grid_area.height - 2` (see `ui/mod.rs`) — and that equality means
  the selected row is always visible, which several things rely on.
- The trailing **phantom `+` column** (`ncols`) is a navigable append slot;
  `i`/Enter there starts an `:add` instead of editing.
- The current-row/column **crosshair** is drawn via the `Table`'s built-in
  `row_highlight_style`/`column_highlight_style`/`cell_highlight_style` +
  `TableState` (not per-cell), because the row highlight spans the inter-column
  spacing — painting it per-cell leaves gaps. Selection indices are absolute and
  get translated to window-relative (with `+1` for the gutter column).

## Build, test, lint

```
cargo build --workspace
cargo test --workspace      # also: cargo test -p <crate>, or -p tabl-tui <name>
cargo clippy --workspace
cargo run -- data/sample.csv          # open the TUI on a sample file
cargo run -- describe data/sample.csv # headless
```

`data/` holds sample files in each supported format for quick manual checks.

**Pre-commit hooks** (`.pre-commit-config.yaml`) are the gate, not CI (there is no
CI yet):
- `cargo +nightly fmt` — formatting uses **nightly** (`.rustfmt.toml` enables
  `unstable_features`, `imports_granularity = "Crate"`). Plain `cargo fmt` may
  differ; use nightly.
- `clippy --all-targets --all -- -D clippy::all -D warnings` — **warnings are
  errors**. Keep clippy clean.
- `cargo test --all` runs on **pre-push**.

Edition is **2024** (Rust 1.85+). The code uses 2024 idioms like `let`-chains
(`if let … && …`) — see `event.rs`/`lib.rs`.

## Conventions

- **Comments explain *why*, densely.** This codebase has unusually thoughtful
  rationale comments (e.g. why the overlay exists, why `head` can't be a clap
  default, why columns are windowed manually). Match that density and tone in new
  code — explain the non-obvious decision, not the obvious mechanics.
- Errors flow through `tabl_core::Error` (a `thiserror` enum) / `Result`. The
  engine wraps polars errors as `Error::Backend(...)`. In the TUI, recoverable
  failures set `app.message` (shown in the status bar) and leave data unchanged
  rather than panicking.
- Tests live inline in `#[cfg(test)] mod tests`. TUI behavior is tested by driving
  `App` with synthetic key events and, for rendering, ratatui's `TestBackend`
  (inspect the cell buffer). Helpers `load_sheet`/`run_cmd` in `tabl-tui/src/lib.rs`
  build a `Sheet` from inline CSV via a temp file — reuse them.

## Status / known limits

Modeled dtypes: bool, int, float, str, date, datetime. Other polars types (list,
struct, duration, decimal) are read and shown as text. JSON must be flat or
newline-delimited. Loading is eager (whole frame in memory). No undo for
structural changes.
