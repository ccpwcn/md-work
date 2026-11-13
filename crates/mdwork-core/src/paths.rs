use std::path::PathBuf;

use crate::{Error, Result, WORKSPACE_FORMAT};

pub const WORKSPACE_DIR_NAME: &str = "md-work";
pub const META_DIR: &str = ".mdwork";
pub const ASSETS_DIR: &str = "assets";
pub const WORKSPACE_FILE: &str = "workspace.toml";
pub const WORKSPACE_FILE_LEGACY: &str = "workspace.json";
pub const NOTEBOOK_FILE: &str = "notebook.toml";
pub const NOTEBOOK_FILE_LEGACY: &str = "notebook.json";
pub const CORE_CONFIG_FILE: &str = "core.toml";
pub const RUNTIME_CONFIG_FILE: &str = "runtime.toml";
pub const THEME_CONFIG_FILE: &str = "theme.toml";
/// Main configuration file for the pluggable theme system.
pub const THEME_EXTENSION_FILE: &str = "theme_extension.toml";
/// Directory (relative to the user config root) holding extension themes.
pub const THEMES_DIR: &str = "themes";

pub fn documents_dir() -> Result<PathBuf> {
    if let Some(path) = known_folders::get_known_folder_path(known_folders::KnownFolder::Documents)
    {
        return Ok(path);
    }
    let home = std::env::var_os("USERPROFILE")
        .map(PathBuf::from)
        .ok_or_else(|| Error::NotFound("Documents".to_string()))?;
    Ok(home.join("Documents"))
}

pub fn default_workspace_root() -> Result<PathBuf> {
    Ok(documents_dir()?.join(WORKSPACE_DIR_NAME))
}

pub fn app_data_dir() -> PathBuf {
    if let Some(local) = std::env::var_os("LOCALAPPDATA") {
        return PathBuf::from(local).join("md-work");
    }
    if let Some(home) = std::env::var_os("USERPROFILE") {
        return PathBuf::from(home).join("AppData/Local/md-work");
    }
    PathBuf::from(".md-work")
}

pub fn config_dir() -> PathBuf {
    app_data_dir().join("config")
}

pub fn core_config_path() -> PathBuf {
    config_dir().join(CORE_CONFIG_FILE)
}

pub fn runtime_config_path() -> PathBuf {
    config_dir().join(RUNTIME_CONFIG_FILE)
}

pub fn theme_config_path() -> PathBuf {
    config_dir().join(THEME_CONFIG_FILE)
}

/// Cross-platform `~/.config/md-work`, the home of the theme extension system.
///
/// On Windows `~` is `%USERPROFILE%`; on Unix `XDG_CONFIG_HOME` wins over
/// `$HOME/.config`. `MDWORK_THEME_HOME` overrides everything (used by tests and
/// by users who keep their dotfiles in a custom location).
pub fn user_config_root() -> PathBuf {
    if let Some(dir) = std::env::var_os("MDWORK_THEME_HOME") {
        return PathBuf::from(dir);
    }
    #[cfg(not(windows))]
    if let Some(dir) = std::env::var_os("XDG_CONFIG_HOME") {
        return PathBuf::from(dir).join(crate::APP_SLUG);
    }
    let home = std::env::var_os("USERPROFILE").or_else(|| std::env::var_os("HOME"));
    match home {
        Some(home) => PathBuf::from(home).join(".config").join(crate::APP_SLUG),
        None => PathBuf::from(".config").join(crate::APP_SLUG),
    }
}

/// `~/.config/md-work/theme_extension.toml`.
pub fn theme_extension_config_path() -> PathBuf {
    user_config_root().join(THEME_EXTENSION_FILE)
}

/// `~/.config/md-work/themes` — watched for new extension theme files.
pub fn extension_themes_dir() -> PathBuf {
    user_config_root().join(THEMES_DIR)
}

/// Primary config path. [`crate::AppConfig::save_to`] treats it as the
/// runtime file and writes the core/theme siblings next to it.
pub fn config_path() -> PathBuf {
    runtime_config_path()
}

/// Pre-TOML single-file configuration, migrated on first start.
pub fn legacy_config_path() -> PathBuf {
    app_data_dir().join("config.json")
}

/// Legacy, workspace-agnostic session file. Kept for migration and tools.
pub fn session_path() -> PathBuf {
    app_data_dir().join("session.toml")
}

pub fn legacy_session_path() -> PathBuf {
    app_data_dir().join("session.json")
}

pub fn workspaces_path() -> PathBuf {
    app_data_dir().join("workspaces.toml")
}

pub fn legacy_workspaces_path() -> PathBuf {
    app_data_dir().join("workspaces.json")
}

pub fn sessions_dir() -> PathBuf {
    app_data_dir().join("sessions")
}

/// Stable per-workspace session file; keyed by the normalized folder path so
/// moving or renaming a folder simply starts a fresh session there.
pub fn workspace_session_path(root: &std::path::Path) -> PathBuf {
    sessions_dir().join(format!("{}.toml", session_key(root)))
}

pub fn session_key(root: &std::path::Path) -> String {
    use sha2::{Digest, Sha256};

    let normalized = root
        .to_string_lossy()
        .replace('\\', "/")
        .trim_end_matches('/')
        .to_lowercase();
    let digest = Sha256::digest(normalized.as_bytes());
    digest
        .iter()
        .take(10)
        .map(|byte| format!("{byte:02x}"))
        .collect()
}

pub fn notebook_prefs_path(notebook_id: &uuid::Uuid) -> PathBuf {
    app_data_dir()
        .join("notebooks")
        .join(notebook_id.to_string())
        .join("prefs.toml")
}

/// Machine-managed dirty-note registry shared with the MCP server.
pub fn dirty_path() -> PathBuf {
    app_data_dir().join("dirty.toml")
}

pub fn recovery_dir() -> PathBuf {
    app_data_dir().join("recovery")
}

pub fn cache_dir() -> PathBuf {
    app_data_dir().join("cache")
}

pub fn exports_dir() -> PathBuf {
    app_data_dir().join("exports")
}

pub fn logs_dir() -> PathBuf {
    app_data_dir().join("logs")
}

pub fn crashes_dir() -> PathBuf {
    app_data_dir().join("crashes")
}

pub fn workspace_meta_dir(root: &std::path::Path) -> PathBuf {
    root.join(META_DIR)
}

pub fn registry_path(root: &std::path::Path) -> PathBuf {
    workspace_meta_dir(root).join(WORKSPACE_FILE)
}

pub fn default_workspace_format() -> &'static str {
    WORKSPACE_FORMAT
}
