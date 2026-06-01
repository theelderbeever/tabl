use crate::value::DType;

/// Metadata for one column, mapped from a polars `Series`/`Field`.
#[derive(Debug, Clone)]
pub struct ColumnMeta {
    pub name: String,
    pub dtype: DType,
}
