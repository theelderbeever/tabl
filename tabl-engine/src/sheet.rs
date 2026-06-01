//! A loaded file: an immutable polars `DataFrame` plus a pending-edit overlay.

use std::collections::HashMap;

use polars::{prelude::*, sql::sql_expr};
use tabl_core::{CellAddr, ColumnMeta, DType, EditOverlay, Error, Result, Snapshot, Value};

pub struct Sheet {
    frame: DataFrame,
    edits: EditOverlay,
    /// When `Some`, a temporary view: the true frame-row indices that are
    /// visible, in order. `None` means all rows. Purely a display map — edits
    /// and `materialize` always address the full frame.
    filter: Option<Vec<usize>>,
}

impl Sheet {
    pub fn new(frame: DataFrame) -> Self {
        Self {
            frame,
            edits: EditOverlay::new(),
            filter: None,
        }
    }

    /// `(rows, cols)`. Rows reflect the active filter (visible count).
    pub fn shape(&self) -> (usize, usize) {
        (self.visible_rows(), self.frame.width())
    }

    pub fn edits_mut(&mut self) -> &mut EditOverlay {
        &mut self.edits
    }

    pub fn is_filtered(&self) -> bool {
        self.filter.is_some()
    }

    /// Number of visible rows (filtered count, or full height).
    fn visible_rows(&self) -> usize {
        self.filter
            .as_ref()
            .map_or(self.frame.height(), |rows| rows.len())
    }

    /// Map a display-row index to its true frame-row index (identity when
    /// unfiltered). Out-of-range yields an index that reads as `Null`.
    pub fn true_row(&self, display: usize) -> usize {
        match &self.filter {
            Some(rows) => rows.get(display).copied().unwrap_or(self.frame.height()),
            None => display,
        }
    }

    /// A render-ready window of rows `[offset, offset + len)` with the overlay
    /// applied on top of the frame's values.
    pub fn view(&self, offset: usize, len: usize) -> Snapshot {
        let total_rows = self.visible_rows();
        let columns = self.column_meta();
        let cols = self.frame.columns();

        let end = offset.saturating_add(len).min(total_rows);
        let mut rows = Vec::with_capacity(end.saturating_sub(offset));

        for display_row in offset..end {
            let abs_row = self.true_row(display_row);
            let mut row = Vec::with_capacity(cols.len());
            for (c, col) in cols.iter().enumerate() {
                let value = match self.edits.get(CellAddr::new(abs_row, c)) {
                    // An overlay edit shadows the frame's value.
                    Some(edited) => edited.clone(),
                    None => col
                        .get(abs_row)
                        .map(|av| any_value_to_value(&av))
                        .unwrap_or(Value::Null),
                };
                row.push(value);
            }
            rows.push(row);
        }

        Snapshot {
            columns,
            row_offset: offset,
            total_rows,
            rows,
        }
    }

    /// The value at display `(row, col)`, overlay edit taking precedence.
    /// Out-of-range indices yield `Value::Null`.
    pub fn cell(&self, row: usize, col: usize) -> Value {
        let abs_row = self.true_row(row);
        if let Some(edited) = self.edits.get(CellAddr::new(abs_row, col)) {
            return edited.clone();
        }
        self.frame
            .columns()
            .get(col)
            .and_then(|c| c.get(abs_row).ok())
            .map(|av| any_value_to_value(&av))
            .unwrap_or(Value::Null)
    }

    /// Set an overlay edit at display `(row, col)`, mapped to the true frame row
    /// so it persists correctly even under an active filter.
    pub fn set_cell(&mut self, row: usize, col: usize, value: Value) {
        let abs_row = self.true_row(row);
        self.edits.set(CellAddr::new(abs_row, col), value);
    }

    /// Logical dtype of column `col`, if it exists.
    pub fn dtype_at(&self, col: usize) -> Option<DType> {
        self.frame.columns().get(col).map(|c| map_dtype(c.dtype()))
    }

    pub fn column_meta(&self) -> Vec<ColumnMeta> {
        self.frame
            .columns()
            .iter()
            .map(|c| ColumnMeta {
                name: c.name().to_string(),
                dtype: map_dtype(c.dtype()),
            })
            .collect()
    }

    /// Insert a new all-null column named `name` of `dtype` at column `index`
    /// (clamped to `[0, width]`).
    ///
    /// Structural edits bake any pending cell edits into the frame first and
    /// then reset the overlay — otherwise the overlay's column indices would
    /// silently misalign with the shifted columns.
    pub fn add_column(&mut self, name: &str, dtype: DType, index: usize) -> Result<()> {
        let mut df = self.materialize()?;
        if df.get_column_index(name).is_some() {
            return Err(Error::Other(format!("column `{name}` already exists")));
        }

        let series = Series::full_null(name.into(), df.height(), &dtype_to_polars(dtype));
        let at = index.min(df.width());
        df.insert_column(at, series.into_column())
            .map_err(|e| Error::Backend(e.to_string()))?;

        self.frame = df;
        self.edits = EditOverlay::new();
        Ok(())
    }

    /// Drop the column named `name`. See [`add_column`](Self::add_column) on why
    /// the overlay is reset.
    pub fn drop_column(&mut self, name: &str) -> Result<()> {
        let df = self.materialize()?;
        if df.get_column_index(name).is_none() {
            return Err(Error::Other(format!("no column `{name}`")));
        }

        self.frame = df.drop(name).map_err(|e| Error::Backend(e.to_string()))?;
        self.edits = EditOverlay::new();
        Ok(())
    }

    /// Insert an all-null row at `index` (clamped to `[0, height]`). Bakes
    /// pending edits and resets the overlay, like the column ops.
    pub fn insert_row(&mut self, index: usize) -> Result<()> {
        let df = self.materialize()?;
        if df.width() == 0 {
            return Err(Error::Other("add a column before adding rows".into()));
        }

        let at = index.min(df.height());
        let null = null_row(&df)?;

        let mut out = df.slice(0, at);
        let backend = |e: PolarsError| Error::Backend(e.to_string());
        out.vstack_mut(&null).map_err(backend)?;
        out.vstack_mut(&df.slice(at as i64, df.height() - at))
            .map_err(backend)?;

        self.frame = out;
        self.edits = EditOverlay::new();
        self.filter = None; // row indices shifted — the temporary view is gone
        Ok(())
    }

    /// Drop the row at `index`. Bakes pending edits and resets the overlay.
    pub fn delete_row(&mut self, index: usize) -> Result<()> {
        let df = self.materialize()?;
        if index >= df.height() {
            return Err(Error::Other(format!("no row {index}")));
        }

        let mut out = df.slice(0, index);
        let tail = df.slice((index + 1) as i64, df.height() - index - 1);
        out.vstack_mut(&tail)
            .map_err(|e| Error::Backend(e.to_string()))?;

        self.frame = out;
        self.edits = EditOverlay::new();
        self.filter = None; // row indices shifted — the temporary view is gone
        Ok(())
    }

    /// Compute a column from a SQL expression (`name = expr_sql`). If `name`
    /// already exists it is overwritten in place; otherwise it is inserted at
    /// `index` (clamped to `[0, width]`). Bakes pending edits and resets the
    /// overlay, like the other structural ops.
    ///
    /// The expression is parsed by polars' SQL layer, so it supports the full
    /// SQL expression grammar (arithmetic, comparisons, functions, `CASE` …).
    /// It must yield one value per row — aggregates that collapse rows are
    /// rejected.
    pub fn eval_column(&mut self, name: &str, expr_sql: &str, index: usize) -> Result<()> {
        let mut df = self.materialize()?;

        let expr = sql_expr(expr_sql).map_err(|e| Error::Other(format!("bad expression: {e}")))?;

        // Evaluate against the original frame, so self-reference (`a = a * 2`)
        // reads the pre-edit values.
        let computed = df
            .clone()
            .lazy()
            .select([expr.alias(name)])
            .collect()
            .map_err(|e| Error::Backend(e.to_string()))?;
        let series = computed
            .column(name)
            .map_err(|e| Error::Backend(e.to_string()))?
            .as_materialized_series()
            .clone();

        if series.len() != df.height() {
            return Err(Error::Other(format!(
                "expression must yield one value per row (got {}, need {})",
                series.len(),
                df.height()
            )));
        }

        let backend = |e: PolarsError| Error::Backend(e.to_string());
        if df.get_column_index(name).is_some() {
            df.with_column(series.into_column()).map_err(backend)?;
        } else {
            df.insert_column(index.min(df.width()), series.into_column())
                .map_err(backend)?;
        }

        self.frame = df;
        self.edits = EditOverlay::new();
        Ok(())
    }

    /// Rename column `old` to `new`. Position and data are unchanged, so the
    /// overlay (keyed by column index) stays valid — no reset needed.
    pub fn rename_column(&mut self, old: &str, new: &str) -> Result<()> {
        if self.frame.get_column_index(old).is_none() {
            return Err(Error::Other(format!("no column `{old}`")));
        }
        if old != new && self.frame.get_column_index(new).is_some() {
            return Err(Error::Other(format!("column `{new}` already exists")));
        }
        self.frame
            .rename(old, new.into())
            .map_err(|e| Error::Backend(e.to_string()))?;
        Ok(())
    }

    /// Set or clear the temporary filter view. `Some(clause)` evaluates a boolean
    /// SQL expression against the full frame and shows only matching rows;
    /// `None` clears it. Always evaluated against the full frame — never layered
    /// on a previous filter.
    pub fn set_filter(&mut self, clause: Option<&str>) -> Result<()> {
        let Some(clause) = clause else {
            self.filter = None;
            return Ok(());
        };

        let df = self.materialize()?;
        let expr = sql_expr(clause).map_err(|e| Error::Other(format!("bad filter: {e}")))?;
        let masked = df
            .lazy()
            .select([expr.alias("__tabl_filter")])
            .collect()
            .map_err(|e| Error::Backend(e.to_string()))?;
        let series = masked
            .column("__tabl_filter")
            .map_err(|e| Error::Backend(e.to_string()))?
            .as_materialized_series();
        let mask = series
            .bool()
            .map_err(|_| Error::Other("filter must be a boolean expression".to_string()))?;

        let rows = mask
            .iter()
            .enumerate()
            .filter_map(|(i, v)| (v == Some(true)).then_some(i))
            .collect();
        self.filter = Some(rows);
        Ok(())
    }

    /// A summary-statistics sheet, like polars' `describe`: a leading
    /// `statistic` column (count, null_count, mean, std, min, median, max) and
    /// one column per source column with the formatted values. Non-numeric stats
    /// (mean/std/median on a string column) render blank.
    ///
    /// (The Rust `DataFrame` has no `describe`; it's a Python-only convenience,
    /// so we assemble it from the per-`Series` aggregations.)
    pub fn describe(&self) -> Result<Sheet> {
        const STATS: [&str; 7] = ["count", "null_count", "mean", "std", "min", "median", "max"];

        let df = self.materialize()?;
        let mut columns: Vec<Column> = Vec::with_capacity(df.width() + 1);
        columns.push(Series::new("statistic".into(), STATS.as_slice()).into_column());

        for col in df.columns() {
            let s = col.as_materialized_series();
            // For temporal columns the raw mean/std/median are day/microsecond
            // counts rather than dates, so leave them blank (min/max still
            // format correctly through the scalar path).
            let temporal = matches!(map_dtype(s.dtype()), DType::Date | DType::Datetime);
            let values: Vec<String> = STATS
                .iter()
                .map(|stat| match *stat {
                    "count" => (s.len() - s.null_count()).to_string(),
                    "null_count" => s.null_count().to_string(),
                    "mean" if !temporal => fmt_opt_f64(s.mean()),
                    "std" if !temporal => fmt_opt_f64(s.std(1)),
                    "median" if !temporal => fmt_opt_f64(s.median()),
                    "min" => scalar_str(s.min_reduce()),
                    "max" => scalar_str(s.max_reduce()),
                    _ => String::new(),
                })
                .collect();
            columns.push(Series::new(s.name().clone(), values.as_slice()).into_column());
        }

        let stats =
            DataFrame::new(STATS.len(), columns).map_err(|e| Error::Backend(e.to_string()))?;
        Ok(Sheet::new(stats))
    }

    /// Apply the overlay and return the resulting frame (used on save).
    ///
    /// No edits → a cheap clone (columns are `Arc`-shared). Otherwise only the
    /// columns that actually have edits are rebuilt, by reading them out as
    /// `AnyValue`s, overwriting the edited rows, and reconstructing the series
    /// at the original dtype.
    pub fn materialize(&self) -> Result<DataFrame> {
        if self.edits.is_empty() {
            return Ok(self.frame.clone());
        }

        let mut by_col: HashMap<usize, Vec<(usize, &Value)>> = HashMap::new();
        for (addr, value) in self.edits.iter() {
            by_col.entry(addr.col).or_default().push((addr.row, value));
        }

        let mut df = self.frame.clone();
        for (col_idx, cell_edits) in by_col {
            let series = df.columns()[col_idx].as_materialized_series().rechunk();
            let dtype = series.dtype().clone();
            let name = series.name().clone();
            let len = series.len();

            let mut values: Vec<AnyValue> = (0..len)
                .map(|i| {
                    series
                        .get(i)
                        .map(|av| av.into_static())
                        .map_err(|e| Error::Backend(e.to_string()))
                })
                .collect::<Result<_>>()?;

            for (row, value) in cell_edits {
                if row < len {
                    values[row] = value_to_any_value(value);
                }
            }

            // strict = false so an edit can widen/coerce into the column dtype.
            let rebuilt = Series::from_any_values_and_dtype(name, &values, &dtype, false)
                .map_err(|e| Error::Backend(e.to_string()))?;
            df.with_column(rebuilt.into_column())
                .map_err(|e| Error::Backend(e.to_string()))?;
        }

        Ok(df)
    }

    /// Like [`materialize`](Self::materialize), but restricted to the visible
    /// rows when a filter is active — so saving a filtered view writes just the
    /// subset. Identical to `materialize` when unfiltered.
    pub fn materialize_view(&self) -> Result<DataFrame> {
        let df = self.materialize()?;
        match &self.filter {
            None => Ok(df),
            Some(rows) => {
                let idx = IdxCa::from_vec(
                    PlSmallStr::EMPTY,
                    rows.iter().map(|&i| i as IdxSize).collect(),
                );
                df.take(&idx).map_err(|e| Error::Backend(e.to_string()))
            }
        }
    }
}

/// Map a polars `DataType` onto the polars-free [`DType`].
fn map_dtype(dt: &DataType) -> DType {
    match dt {
        DataType::Boolean => DType::Bool,
        DataType::Date => DType::Date,
        DataType::Datetime(_, _) => DType::Datetime,
        dt if dt.is_integer() => DType::Int,
        dt if dt.is_float() => DType::Float,
        DataType::String => DType::Str,
        _ => DType::Unknown,
    }
}

/// Convert a polars datetime value in `unit` to microseconds.
fn datetime_micros(value: i64, unit: TimeUnit) -> i64 {
    match unit {
        TimeUnit::Nanoseconds => value / 1_000,
        TimeUnit::Microseconds => value,
        TimeUnit::Milliseconds => value * 1_000,
    }
}

/// Format an optional float for the describe table (blank for `None`, integral
/// values without a decimal, otherwise up to 4 trimmed decimals).
fn fmt_opt_f64(v: Option<f64>) -> String {
    let Some(x) = v else {
        return String::new();
    };
    if x.is_finite() && x == x.trunc() {
        return format!("{x:.0}");
    }
    let s = format!("{x:.4}");
    s.trim_end_matches('0').trim_end_matches('.').to_string()
}

/// Format a reduction scalar (min/max) for the describe table; blank on error.
fn scalar_str(reduced: PolarsResult<Scalar>) -> String {
    match reduced {
        Ok(scalar) => any_value_to_value(scalar.value()).display(),
        Err(_) => String::new(),
    }
}

/// Build a one-row, all-null `DataFrame` matching `df`'s schema, for insertion.
fn null_row(df: &DataFrame) -> Result<DataFrame> {
    let cols: Vec<Column> = df
        .columns()
        .iter()
        .map(|c| Series::full_null(c.name().clone(), 1, c.dtype()).into_column())
        .collect();
    DataFrame::new(1, cols).map_err(|e| Error::Backend(e.to_string()))
}

/// Map a polars-free [`DType`] onto the polars `DataType` for a new column.
/// `Unknown` falls back to `String`, the most permissive for later editing.
fn dtype_to_polars(d: DType) -> DataType {
    match d {
        DType::Bool => DataType::Boolean,
        DType::Int => DataType::Int64,
        DType::Float => DataType::Float64,
        DType::Date => DataType::Date,
        DType::Datetime => DataType::Datetime(TimeUnit::Microseconds, None),
        DType::Str | DType::Unknown => DataType::String,
    }
}

/// Map a single polars [`AnyValue`] onto the polars-free [`Value`].
///
/// Mirrors [`map_dtype`]: anything not modelled is stringified via its `Display`
/// so it still renders (durations, nested, …).
fn any_value_to_value(av: &AnyValue) -> Value {
    match av {
        AnyValue::Null => Value::Null,
        AnyValue::Boolean(b) => Value::Bool(*b),
        AnyValue::Int8(v) => Value::Int(*v as i64),
        AnyValue::Int16(v) => Value::Int(*v as i64),
        AnyValue::Int32(v) => Value::Int(*v as i64),
        AnyValue::Int64(v) => Value::Int(*v),
        AnyValue::UInt8(v) => Value::Int(*v as i64),
        AnyValue::UInt16(v) => Value::Int(*v as i64),
        AnyValue::UInt32(v) => Value::Int(*v as i64),
        AnyValue::UInt64(v) => Value::Int(*v as i64),
        AnyValue::Float32(v) => Value::Float(*v as f64),
        AnyValue::Float64(v) => Value::Float(*v),
        AnyValue::String(s) => Value::Str(s.to_string()),
        AnyValue::StringOwned(s) => Value::Str(s.to_string()),
        AnyValue::Date(d) => Value::Date(*d),
        AnyValue::Datetime(v, unit, _) => Value::Datetime(datetime_micros(*v, *unit)),
        AnyValue::DatetimeOwned(v, unit, _) => Value::Datetime(datetime_micros(*v, *unit)),
        other => Value::Str(other.to_string()),
    }
}

/// Map a polars-free [`Value`] back onto an owned [`AnyValue`] for materializing
/// overlay edits. The target column dtype handles any final coercion.
fn value_to_any_value(v: &Value) -> AnyValue<'static> {
    match v {
        Value::Null => AnyValue::Null,
        Value::Bool(b) => AnyValue::Boolean(*b),
        Value::Int(i) => AnyValue::Int64(*i),
        Value::Float(f) => AnyValue::Float64(*f),
        Value::Str(s) => AnyValue::StringOwned(s.as_str().into()),
        Value::Date(d) => AnyValue::Date(*d),
        Value::Datetime(us) => AnyValue::Datetime(*us, TimeUnit::Microseconds, None),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn view_maps_values_and_overlay_wins() {
        let frame = df![
            "a" => [1i64, 2, 3],
            "b" => ["x", "y", "z"],
        ]
        .unwrap();
        let mut sheet = Sheet::new(frame);

        let snap = sheet.view(0, 2);
        assert_eq!(snap.total_rows, 3);
        assert_eq!(snap.rows.len(), 2);
        assert_eq!(snap.rows[0][0], Value::Int(1));
        assert_eq!(snap.rows[1][1], Value::Str("y".into()));

        // A windowed read past the end clamps rather than panicking.
        let tail = sheet.view(2, 10);
        assert_eq!(tail.rows.len(), 1);
        assert_eq!(tail.row_offset, 2);

        // An overlay edit shadows the underlying frame value.
        sheet.edits_mut().set(CellAddr::new(0, 0), Value::Int(99));
        assert_eq!(sheet.view(0, 1).rows[0][0], Value::Int(99));
    }

    #[test]
    fn materialize_applies_overlay() {
        let frame = df![
            "a" => [1i64, 2, 3],
            "b" => ["x", "y", "z"],
        ]
        .unwrap();
        let mut sheet = Sheet::new(frame);

        // No edits → unchanged frame.
        assert_eq!(sheet.materialize().unwrap().shape(), (3, 2));

        sheet.edits_mut().set(CellAddr::new(0, 0), Value::Int(99));
        sheet
            .edits_mut()
            .set(CellAddr::new(2, 1), Value::Str("zz".into()));

        let out = sheet.materialize().unwrap();
        assert_eq!(
            out.column("a").unwrap().get(0).unwrap(),
            AnyValue::Int64(99)
        );
        assert_eq!(
            out.column("b").unwrap().get(2).unwrap(),
            AnyValue::String("zz")
        );
        // Dtypes are preserved through the rebuild.
        assert_eq!(out.column("a").unwrap().dtype(), &DataType::Int64);
    }

    #[test]
    fn add_and_drop_columns() {
        let frame = df!["a" => [1i64, 2], "b" => ["x", "y"]].unwrap();
        let mut sheet = Sheet::new(frame);

        // Insert a null str column at the front.
        sheet.add_column("c", DType::Str, 0).unwrap();
        assert_eq!(sheet.shape(), (2, 3));
        assert_eq!(sheet.column_meta()[0].name, "c");
        assert_eq!(sheet.column_meta()[0].dtype, DType::Str);
        assert!(sheet.cell(0, 0).is_null());

        // Duplicate name is rejected.
        assert!(sheet.add_column("a", DType::Int, 0).is_err());

        // Drop it again.
        sheet.drop_column("c").unwrap();
        assert_eq!(sheet.shape(), (2, 2));
        assert!(sheet.drop_column("nope").is_err());
    }

    #[test]
    fn structural_ops_bake_pending_edits() {
        let frame = df!["a" => [1i64, 2]].unwrap();
        let mut sheet = Sheet::new(frame);

        // Pending edit on column 0, then a structural insert at the front that
        // would shift column 0 to index 1.
        sheet.edits_mut().set(CellAddr::new(0, 0), Value::Int(99));
        sheet.add_column("z", DType::Int, 0).unwrap();

        // The edit was baked into the (now shifted) original column, not left
        // dangling at the old index.
        assert_eq!(sheet.column_meta()[1].name, "a");
        assert_eq!(sheet.cell(0, 1), Value::Int(99));
    }

    #[test]
    fn insert_and_delete_rows() {
        let frame = df!["a" => [1i64, 2, 3], "b" => ["x", "y", "z"]].unwrap();
        let mut sheet = Sheet::new(frame);

        sheet.insert_row(1).unwrap();
        assert_eq!(sheet.shape(), (4, 2));
        assert_eq!(sheet.cell(0, 0), Value::Int(1));
        assert!(sheet.cell(1, 0).is_null()); // the new null row
        assert_eq!(sheet.cell(2, 0), Value::Int(2)); // shifted down

        sheet.delete_row(0).unwrap();
        assert_eq!(sheet.shape(), (3, 2));
        assert!(sheet.cell(0, 0).is_null()); // the inserted row is now first

        assert!(sheet.delete_row(99).is_err());
    }

    #[test]
    fn eval_adds_overwrites_and_typechecks() {
        let frame = df!["a" => [1i64, 2, 3], "b" => [10i64, 20, 30]].unwrap();
        let mut sheet = Sheet::new(frame);

        // New column at index 0 (cursor), computed from others.
        sheet.eval_column("c", "a * 2 + b", 0).unwrap();
        assert_eq!(sheet.shape(), (3, 3));
        assert_eq!(sheet.column_meta()[0].name, "c"); // inserted at the index
        assert_eq!(sheet.cell(0, 0), Value::Int(12)); // 1*2 + 10

        // Overwriting an existing column keeps its position; self-reference reads
        // the pre-eval values.
        sheet.eval_column("a", "a * 10", 0).unwrap();
        assert_eq!(sheet.shape(), (3, 3));
        let a = sheet
            .column_meta()
            .iter()
            .position(|c| c.name == "a")
            .unwrap();
        assert_eq!(sheet.cell(0, a), Value::Int(10));

        // A comparison yields a boolean column.
        sheet.eval_column("big", "b > 15", 0).unwrap();
        let meta = sheet.column_meta();
        let big = meta.iter().position(|c| c.name == "big").unwrap();
        assert_eq!(meta[big].dtype, DType::Bool);
        assert_eq!(sheet.cell(0, big), Value::Bool(false)); // b=10 is not > 15
        assert_eq!(sheet.cell(1, big), Value::Bool(true)); // b=20 is > 15

        // Garbage and unknown columns are rejected, leaving the sheet intact.
        let shape = sheet.shape();
        assert!(sheet.eval_column("x", "this is not sql @#", 0).is_err());
        assert!(sheet.eval_column("x", "nope + 1", 0).is_err());
        assert_eq!(sheet.shape(), shape);
    }

    #[test]
    fn filter_is_a_temporary_view_with_true_row_edits() {
        let frame = df!["a" => [1i64, 2, 3, 4], "b" => [10i64, 20, 30, 40]].unwrap();
        let mut sheet = Sheet::new(frame);

        // Filter to even `a`: rows 1 (a=2) and 3 (a=4).
        sheet.set_filter(Some("a % 2 = 0")).unwrap();
        assert!(sheet.is_filtered());
        assert_eq!(sheet.shape(), (2, 2));
        assert_eq!(sheet.cell(0, 0), Value::Int(2)); // display row 0 -> true row 1
        assert_eq!(sheet.cell(1, 0), Value::Int(4)); // display row 1 -> true row 3
        assert_eq!(sheet.true_row(1), 3);

        // Editing display row 0 writes the TRUE row (1); it survives in the full
        // frame after the filter is cleared.
        sheet.set_cell(0, 1, Value::Int(99));
        sheet.set_filter(None).unwrap();
        assert!(!sheet.is_filtered());
        assert_eq!(sheet.shape(), (4, 2));
        assert_eq!(sheet.cell(1, 1), Value::Int(99));

        // Re-filtering is evaluated fresh against the full frame, never layered.
        sheet.set_filter(Some("a > 2")).unwrap();
        assert_eq!(sheet.shape(), (2, 2)); // rows a=3, a=4

        // A non-boolean expression is rejected.
        assert!(sheet.set_filter(Some("a + 1")).is_err());
    }

    #[test]
    fn materialize_view_writes_only_filtered_rows() {
        let frame = df!["a" => [1i64, 2, 3, 4]].unwrap();
        let mut sheet = Sheet::new(frame);

        // Unfiltered → full frame.
        assert_eq!(sheet.materialize_view().unwrap().height(), 4);

        sheet.set_filter(Some("a > 2")).unwrap();
        let view = sheet.materialize_view().unwrap();
        assert_eq!(view.height(), 2);
        let col = view.column("a").unwrap();
        assert_eq!(col.get(0).unwrap(), AnyValue::Int64(3));
        assert_eq!(col.get(1).unwrap(), AnyValue::Int64(4));

        // The full frame is untouched (filter is a view, not a mutation).
        assert_eq!(sheet.materialize().unwrap().height(), 4);
    }

    #[test]
    fn dates_are_first_class() {
        let frame = df!["d" => ["2026-01-02", "2026-03-04"]].unwrap();
        let mut sheet = Sheet::new(frame);

        // Converting a string column to a date is recognized as a Date type.
        sheet.eval_column("d", "DATE(d)", 0).unwrap();
        assert_eq!(sheet.dtype_at(0), Some(DType::Date));
        assert_eq!(sheet.cell(0, 0).display(), "2026-01-02");

        // Editing a date cell and materializing keeps the column a real Date.
        let days = tabl_core::value::parse_date("2030-12-31").unwrap();
        sheet.set_cell(0, 0, Value::Date(days));
        assert_eq!(sheet.cell(0, 0).display(), "2030-12-31");

        let out = sheet.materialize().unwrap();
        assert_eq!(out.column("d").unwrap().dtype(), &DataType::Date);

        // A fresh `date` column is a real Date too.
        sheet.add_column("born", DType::Date, 1).unwrap();
        assert_eq!(sheet.dtype_at(1), Some(DType::Date));
    }

    #[test]
    fn rename_column_keeps_position_and_data() {
        let frame = df!["a" => [1i64, 2], "b" => [3i64, 4]].unwrap();
        let mut sheet = Sheet::new(frame);

        sheet.rename_column("a", "x").unwrap();
        assert_eq!(sheet.column_meta()[0].name, "x");
        assert_eq!(sheet.cell(0, 0), Value::Int(1)); // data intact, position 0

        assert!(sheet.rename_column("nope", "y").is_err()); // unknown source
        assert!(sheet.rename_column("x", "b").is_err()); // target already exists
    }

    #[test]
    fn describe_produces_a_stats_sheet() {
        let frame = df!["a" => [1i64, 2, 3], "b" => [10i64, 20, 30]].unwrap();
        let sheet = Sheet::new(frame);

        let stats = sheet.describe().unwrap();
        // First column labels the statistic; original columns follow.
        let names: Vec<_> = stats.column_meta().into_iter().map(|c| c.name).collect();
        assert!(names.iter().any(|n| n == "a"));
        assert!(names.iter().any(|n| n == "b"));
        assert!(stats.shape().0 >= 1); // at least one statistic row
    }

    #[test]
    fn row_ops_bake_pending_edits() {
        let frame = df!["a" => [1i64, 2]].unwrap();
        let mut sheet = Sheet::new(frame);

        sheet.edits_mut().set(CellAddr::new(1, 0), Value::Int(99));
        sheet.insert_row(0).unwrap(); // null row on top shifts the rest down

        assert!(sheet.cell(0, 0).is_null());
        assert_eq!(sheet.cell(2, 0), Value::Int(99)); // edit followed its row
    }
}
