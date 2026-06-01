//! The `show` subcommand: print head/tail rows of a file as a text table.
//!
//! Consumes the engine's `Snapshot` exactly like the TUI will, so this doubles
//! as an end-to-end check of the `load → view` path against real files.

use std::{ops::Range, path::Path};

use tabl_core::{ColumnMeta, Result, Value};

/// Columns wider than this are truncated with an ellipsis.
const MAX_COL_WIDTH: usize = 40;

/// `head`/`tail` are already resolved by the caller (neither given → head 10).
pub fn run(
    file: &Path,
    head: Option<usize>,
    tail: Option<usize>,
    opts: tabl_engine::io::LoadOptions,
) -> Result<()> {
    let sheet = tabl_engine::io::load_with(file, opts)?;
    let total = sheet.shape().0;
    let cols = sheet.column_meta();

    let (head_range, tail_range) = resolve_ranges(head, tail, total);

    let rows_for = |range: Range<usize>| -> Vec<Line> {
        let snap = sheet.view(range.start, range.len());
        snap.rows
            .iter()
            .enumerate()
            .map(|(j, row)| {
                Line::Row(
                    snap.row_offset + j,
                    row.iter().map(Value::display).collect(),
                )
            })
            .collect()
    };

    let mut lines = rows_for(head_range);
    if let Some(tail_range) = tail_range {
        // A second range only exists when there's a gap between head and tail.
        lines.push(Line::Gap);
        lines.extend(rows_for(tail_range));
    }

    print_table(&cols, &lines, total);
    Ok(())
}

/// Print every row of a sheet as a table (used by `describe`).
pub fn print_sheet(sheet: &tabl_engine::Sheet) {
    let total = sheet.shape().0;
    let cols = sheet.column_meta();
    let snap = sheet.view(0, total);
    let lines: Vec<Line> = snap
        .rows
        .iter()
        .enumerate()
        .map(|(j, row)| {
            Line::Row(
                snap.row_offset + j,
                row.iter().map(Value::display).collect(),
            )
        })
        .collect();
    print_table(&cols, &lines, total);
}

enum Line {
    Row(usize, Vec<String>),
    Gap,
}

/// Resolve head/tail into the row range(s) to print: a head range plus an
/// optional tail range. The tail range is only `Some` when head and tail don't
/// meet — i.e. when a gap (`…`) should be shown between them. When they meet or
/// overlap they collapse into one contiguous block.
fn resolve_ranges(
    head: Option<usize>,
    tail: Option<usize>,
    total: usize,
) -> (Range<usize>, Option<Range<usize>>) {
    let head_end = head.map(|h| h.min(total));
    let tail_start = tail.map(|t| total - t.min(total));

    match (head_end, tail_start) {
        (Some(he), Some(ts)) if he < ts => (0..he, Some(ts..total)),
        (Some(_), Some(_)) => (0..total, None),
        (Some(he), None) => (0..he, None),
        (None, Some(ts)) => (ts..total, None),
        // Caller guarantees at least one bound; fall back to head 10 defensively.
        (None, None) => (0..total.min(10), None),
    }
}

fn print_table(cols: &[ColumnMeta], lines: &[Line], total: usize) {
    let ncols = cols.len();

    let mut widths: Vec<usize> = cols.iter().map(|c| c.name.chars().count()).collect();
    for line in lines {
        if let Line::Row(_, cells) = line {
            for (c, cell) in cells.iter().enumerate().take(ncols) {
                widths[c] = widths[c].max(cell.chars().count());
            }
        }
    }
    for w in &mut widths {
        *w = (*w).min(MAX_COL_WIDTH);
    }

    // Index gutter sized to the widest absolute row number.
    let idx_w = total.saturating_sub(1).to_string().len().max(1);

    let header: Vec<String> = cols.iter().map(|c| c.name.clone()).collect();
    print_row(idx_w, "#", &widths, &header);

    let rule: Vec<String> = widths.iter().map(|w| "-".repeat(*w)).collect();
    print_row(idx_w, &"-".repeat(idx_w), &widths, &rule);

    for line in lines {
        match line {
            Line::Gap => {
                let dots = vec!["…".to_string(); ncols];
                print_row(idx_w, "…", &widths, &dots);
            }
            Line::Row(idx, cells) => print_row(idx_w, &idx.to_string(), &widths, cells),
        }
    }

    println!("\n{total} rows × {ncols} cols");
}

fn print_row(idx_w: usize, idx: &str, widths: &[usize], cells: &[String]) {
    let mut out = pad(idx, idx_w);
    for (c, &w) in widths.iter().enumerate() {
        out.push_str("  ");
        let cell = cells.get(c).map(String::as_str).unwrap_or("");
        out.push_str(&pad(&truncate(cell, w), w));
    }
    println!("{}", out.trim_end());
}

fn truncate(s: &str, width: usize) -> String {
    if s.chars().count() <= width {
        return s.to_string();
    }
    if width == 0 {
        return String::new();
    }
    let mut t: String = s.chars().take(width - 1).collect();
    t.push('…');
    t
}

fn pad(s: &str, width: usize) -> String {
    let len = s.chars().count();
    let mut out = s.to_string();
    if len < width {
        out.extend(std::iter::repeat_n(' ', width - len));
    }
    out
}
