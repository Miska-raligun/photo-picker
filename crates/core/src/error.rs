use std::path::PathBuf;
use thiserror::Error;

#[derive(Debug, Error)]
pub enum Error {
    #[error("I/O error at {path}: {source}")]
    Io {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },

    #[error("image decode failed at {path}: {source}")]
    Decode {
        path: PathBuf,
        #[source]
        source: image::ImageError,
    },

    #[error("EXIF parse failed at {path}: {source}")]
    Exif {
        path: PathBuf,
        #[source]
        source: exif::Error,
    },

    #[error("invalid configuration: {0}")]
    Config(String),

    #[error("scan produced zero usable photos under {root}")]
    EmptyScan { root: PathBuf },
}

pub type Result<T> = std::result::Result<T, Error>;
