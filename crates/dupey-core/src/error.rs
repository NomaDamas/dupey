use std::path::PathBuf;

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("unsupported format: {path}")]
    UnsupportedFormat { path: PathBuf },
    #[error("io error for {path}: {source}")]
    Io {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
    #[error("file is not valid UTF-8 text: {path}")]
    InvalidUtf8 { path: PathBuf },
    #[error("failed to extract {path}: {message}")]
    Extract { path: PathBuf, message: String },
}

pub type Result<T> = std::result::Result<T, Error>;
