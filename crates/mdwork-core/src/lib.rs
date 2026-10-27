//! Headless core: workspace/notebook model, file services, encoding, search.

pub mod archive;
pub mod atomic;
pub mod bookmarks;
pub mod config;
pub mod dirty;
pub mod doc;
pub mod image;
pub mod notebook;
pub mod paths;
pub mod permissions;
pub mod search;
pub mod text;
pub mod watch;
pub mod workspace;

pub use bookmarks::{Bookmark, Bookmarks};
pub use config::{AppConfig, Session, ThemePreference};
pub use dirty::{DirtyDoc, DirtyState};
pub use doc::{DecodedDoc, Eol, TextEncoding};
pub use image::{ImageHost, ImageKind};
pub use permissions::{Scope, TokenRecord};
pub use search::{search_notebook, search_notebook_filtered, SearchHit, SearchOutcome, SearchQuery};
pub use text::{TextStats, text_stats};
pub use notebook::{Entry, EntryKind, Notebook};
pub use workspace::{ScanIssue, Workspace};

use std::path::PathBuf;

pub const APP_NAME: &str = "墨笺";
pub const APP_SLUG: &str = "md-work";
pub const NOTEBOOK_FORMAT: &str = "md-work/notebook@1";
pub const WORKSPACE_FORMAT: &str = "md-work/workspace@1";

pub fn version() -> &'static str {
    env!("CARGO_PKG_VERSION")
}

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("I/O error: {0}")]
    Io(#[from] std::io::Error),

    #[error("file watcher error: {0}")]
    Watch(#[from] notify::Error),

    #[error("invalid JSON in {path}: {source}")]
    Json {
        path: PathBuf,
        #[source]
        source: serde_json::Error,
    },

    #[error("not a md-work notebook: {0}")]
    NotNotebook(PathBuf),

    #[error("unsupported format {found:?} in {path}")]
    UnsupportedFormat { path: PathBuf, found: String },

    #[error("invalid relative path: {0}")]
    InvalidPath(String),

    #[error("reserved name: {0}")]
    ReservedName(String),

    #[error("path escapes the notebook: {0}")]
    Escape(String),

    #[error("already exists: {0}")]
    Exists(String),

    #[error("not found: {0}")]
    NotFound(String),

    #[error("text cannot be encoded as {encoding}")]
    Unencodable { encoding: &'static str },

    #[error("invalid notebook archive: {0}")]
    InvalidArchive(String),

    #[error("archive exceeds limits: {0}")]
    ArchiveLimit(String),

    #[error("unsupported image: {0}")]
    UnsupportedImage(String),

    #[error("image too large: {size} bytes (limit {limit} bytes)")]
    ImageTooLarge { size: u64, limit: u64 },

    #[error("{0}")]
    Upload(String),
}

pub type Result<T> = std::result::Result<T, Error>;
