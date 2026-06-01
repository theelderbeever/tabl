use std::path::Path;

use tabl_core::{Error, Result};

/// A supported on-disk data format. JSON is split into nested `Json` and
/// line-delimited `NdJson`; for the MVP the grid only handles flat records.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Format {
    Parquet,
    Csv,
    Json,
    NdJson,
}

impl Format {
    pub fn from_path(path: &Path) -> Result<Self> {
        let ext = path
            .extension()
            .and_then(|e| e.to_str())
            .unwrap_or_default()
            .to_ascii_lowercase();

        match ext.as_str() {
            "parquet" | "pq" => Ok(Format::Parquet),
            "csv" => Ok(Format::Csv),
            "json" => Ok(Format::Json),
            "ndjson" | "jsonl" => Ok(Format::NdJson),
            other => Err(Error::UnsupportedFormat(other.to_string())),
        }
    }
}
