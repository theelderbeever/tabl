//! Loading and saving via polars readers/writers.

use std::{fs::File, path::Path};

use polars::prelude::*;
use tabl_core::{Error, Result};

use crate::{format::Format, sheet::Sheet};

fn open(path: &Path) -> Result<File> {
    File::open(path).map_err(|e| Error::Io(e.to_string()))
}

fn backend<T, E: std::fmt::Display>(r: std::result::Result<T, E>) -> Result<T> {
    r.map_err(|e| Error::Backend(e.to_string()))
}

/// Options controlling how a file is read.
#[derive(Debug, Clone, Copy, Default)]
pub struct LoadOptions {
    /// Infer date/datetime columns from CSV text (off by default — a column that
    /// merely looks date-like can otherwise be mis-typed). Ignored for formats
    /// that already carry types (Parquet) or where it does not apply.
    pub parse_dates: bool,
}

/// Load a file into a [`Sheet`] with default options.
pub fn load(path: &Path) -> Result<Sheet> {
    load_with(path, LoadOptions::default())
}

/// Load a file into a [`Sheet`], inferring the format from its extension.
///
/// Eager for the MVP — the whole frame lives in memory. Swap individual arms
/// for `LazyFrame::scan_*` later if streaming large files becomes a goal.
pub fn load_with(path: &Path, opts: LoadOptions) -> Result<Sheet> {
    let frame = match Format::from_path(path)? {
        Format::Parquet => backend(ParquetReader::new(open(path)?).finish())?,

        Format::Csv => {
            let reader = backend(
                CsvReadOptions::default()
                    .with_has_header(true)
                    .with_parse_options(
                        CsvParseOptions::default().with_try_parse_dates(opts.parse_dates),
                    )
                    .try_into_reader_with_file_path(Some(path.to_path_buf())),
            )?;
            backend(reader.finish())?
        }

        Format::Json => backend(
            JsonReader::new(open(path)?)
                .with_json_format(JsonFormat::Json)
                .finish(),
        )?,

        Format::NdJson => backend(
            JsonReader::new(open(path)?)
                .with_json_format(JsonFormat::JsonLines)
                .finish(),
        )?,
    };

    Ok(Sheet::new(frame))
}

/// Materialize the sheet's edit overlay and write it as `fmt` to `path`.
pub fn save(sheet: &Sheet, fmt: Format, path: &Path) -> Result<()> {
    // `materialize_view` writes only the visible rows when a filter is active.
    let mut df = sheet.materialize_view()?;
    let file = File::create(path).map_err(|e| Error::Io(e.to_string()))?;

    let result = match fmt {
        Format::Parquet => ParquetWriter::new(file).finish(&mut df).map(|_| ()),
        Format::Csv => CsvWriter::new(file).finish(&mut df),
        Format::Json => JsonWriter::new(file)
            .with_json_format(JsonFormat::Json)
            .finish(&mut df),
        Format::NdJson => JsonWriter::new(file)
            .with_json_format(JsonFormat::JsonLines)
            .finish(&mut df),
    };

    backend(result)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    #[test]
    fn loads_csv() {
        let path = std::env::temp_dir().join("tabl_load_smoke.csv");
        let mut f = File::create(&path).unwrap();
        write!(f, "a,b\n1,x\n2,y\n").unwrap();

        let sheet = load(&path).unwrap();
        assert_eq!(sheet.shape(), (2, 2));

        let meta = sheet.column_meta();
        assert_eq!(meta[0].name, "a");
        assert_eq!(meta[1].name, "b");

        let _ = std::fs::remove_file(&path);
    }
}
