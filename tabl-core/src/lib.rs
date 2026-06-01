//! Polars-free domain model shared across the tabl crates.
//!
//! `tabl-engine` maps polars types to and from these so the UI never touches
//! polars directly.

pub mod column;
pub mod edit;
pub mod error;
pub mod value;
pub mod view;

pub use column::ColumnMeta;
pub use edit::{CellAddr, EditOverlay};
pub use error::{Error, Result};
pub use value::{DType, Value};
pub use view::Snapshot;
