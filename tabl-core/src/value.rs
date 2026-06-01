//! Polars-free cell value and dtype model.

use chrono::{DateTime, NaiveDate, NaiveDateTime, TimeDelta};

/// A single cell value. Temporal values are stored the way polars stores them:
/// `Date` as days since the Unix epoch, `Datetime` as microseconds since it.
#[derive(Debug, Clone, PartialEq)]
pub enum Value {
    Null,
    Bool(bool),
    Int(i64),
    Float(f64),
    Str(String),
    /// Days since 1970-01-01.
    Date(i32),
    /// Microseconds since 1970-01-01T00:00:00 (treated as UTC/naive).
    Datetime(i64),
}

impl Value {
    pub fn is_null(&self) -> bool {
        matches!(self, Value::Null)
    }

    /// Display rendering for the grid. Nulls render as the caller's sentinel.
    pub fn display(&self) -> String {
        match self {
            Value::Null => String::new(),
            Value::Bool(b) => b.to_string(),
            Value::Int(i) => i.to_string(),
            Value::Float(f) => f.to_string(),
            Value::Str(s) => s.clone(),
            Value::Date(d) => format_date(*d),
            Value::Datetime(us) => format_datetime(*us),
        }
    }
}

/// Logical column type, mapped from polars `DataType` by the engine.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DType {
    Bool,
    Int,
    Float,
    Str,
    Date,
    Datetime,
    /// Anything not yet modelled (struct, list, …).
    Unknown,
}

impl std::fmt::Display for DType {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let s = match self {
            DType::Bool => "bool",
            DType::Int => "int",
            DType::Float => "float",
            DType::Str => "str",
            DType::Date => "date",
            DType::Datetime => "datetime",
            DType::Unknown => "?",
        };
        f.write_str(s)
    }
}

fn epoch() -> NaiveDate {
    NaiveDate::from_ymd_opt(1970, 1, 1).expect("valid epoch date")
}

const DATETIME_FORMATS: [&str; 4] = [
    "%Y-%m-%d %H:%M:%S%.f",
    "%Y-%m-%dT%H:%M:%S%.f",
    "%Y-%m-%d %H:%M:%S",
    "%Y-%m-%dT%H:%M:%S",
];

/// Format days-since-epoch as `YYYY-MM-DD`.
pub fn format_date(days: i32) -> String {
    epoch()
        .checked_add_signed(TimeDelta::days(days as i64))
        .map(|d| d.format("%Y-%m-%d").to_string())
        .unwrap_or_default()
}

/// Parse `YYYY-MM-DD` into days since the epoch.
pub fn parse_date(s: &str) -> Option<i32> {
    let date = NaiveDate::parse_from_str(s.trim(), "%Y-%m-%d").ok()?;
    Some((date - epoch()).num_days() as i32)
}

/// Format microseconds-since-epoch as `YYYY-MM-DD HH:MM:SS[.ffffff]`.
pub fn format_datetime(micros: i64) -> String {
    DateTime::from_timestamp_micros(micros)
        .map(|dt| dt.naive_utc().format("%Y-%m-%d %H:%M:%S%.f").to_string())
        .unwrap_or_default()
}

/// Parse a datetime (date with time, space- or `T`-separated, optional
/// fractional seconds) into microseconds since the epoch.
pub fn parse_datetime(s: &str) -> Option<i64> {
    let s = s.trim();
    DATETIME_FORMATS
        .iter()
        .find_map(|fmt| NaiveDateTime::parse_from_str(s, fmt).ok())
        .map(|dt| dt.and_utc().timestamp_micros())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn date_round_trips() {
        let days = parse_date("2026-04-14").unwrap();
        assert_eq!(format_date(days), "2026-04-14");
        // Pre-epoch dates work too.
        let old = parse_date("1965-02-01").unwrap();
        assert!(old < 0);
        assert_eq!(format_date(old), "1965-02-01");
        assert!(parse_date("nope").is_none());
    }

    #[test]
    fn datetime_round_trips() {
        let us = parse_datetime("2026-04-14 13:30:00").unwrap();
        assert_eq!(format_datetime(us), "2026-04-14 13:30:00");
        // `T` separator is accepted.
        assert_eq!(parse_datetime("2026-04-14T13:30:00"), Some(us));
        assert!(parse_datetime("not a datetime").is_none());
    }
}
