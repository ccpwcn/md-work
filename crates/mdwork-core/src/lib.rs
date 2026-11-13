//! Headless core: workspace/notebook model, file services, encoding, search.

pub mod ai;
pub mod archive;
pub mod asset_cleanup;
pub mod atomic;
pub mod bookmarks;
pub mod config;
pub mod crash;
pub mod dirty;
pub mod doc;
pub mod image;
pub mod mcp_permissions;
pub mod notebook;
pub mod order;
pub mod paths;
pub mod permissions;
pub mod recovery;
pub mod search;
pub mod text;
pub mod theme_ext;
pub mod toml_io;
pub mod watch;
pub mod workspace;
pub mod workspace_registry;

pub use ai::{
    AiMessage, AiProvider, AiRole, AiUsage, CompletionRequest, CompletionResponse, ProviderRegistry,
};
pub use bookmarks::{Bookmark, Bookmarks};
pub use config::{AppConfig, BookmarkMarker, Session, ThemePreference};
pub use dirty::{DirtyDoc, DirtyState};
pub use doc::{DecodedDoc, EncodingHint, Eol, TextEncoding};
pub use image::{ImageHost, ImageKind};
pub use notebook::{Entry, EntryKind, Notebook};
pub use permissions::{Scope, TokenRecord};
pub use search::{
    SearchHit, SearchOutcome, SearchQuery, search_notebook, search_notebook_filtered,
};
pub use text::{TextStats, text_stats};
pub use workspace::{FolderKind, ScanIssue, Workspace, has_notebook_marker};
pub use workspace_registry::{WorkspaceEntry, WorkspaceRegistry};

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

    #[error("invalid TOML in {path}: {source}")]
    Toml {
        path: PathBuf,
        // TOML 解析错误体积很大，装箱后 Result 的错误分支只有指针大小。
        #[source]
        source: Box<toml::de::Error>,
    },

    #[error("cannot write TOML {path}: {source}")]
    TomlWrite {
        path: PathBuf,
        #[source]
        source: toml::ser::Error,
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

    #[error("AI provider error: {0}")]
    Ai(String),
}

pub type Result<T> = std::result::Result<T, Error>;
