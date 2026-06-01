use thiserror::Error;

#[derive(Debug, Error)]
pub enum Error {
    #[error("io error: {0}")]
    Io(String),

    #[error("unsupported format: {0}")]
    UnsupportedFormat(String),

    #[error("backend error: {0}")]
    Backend(String),

    #[error("{0}")]
    Other(String),
}

pub type Result<T> = std::result::Result<T, Error>;
