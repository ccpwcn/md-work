use std::path::PathBuf;

use crate::{Error, Result, WORKSPACE_FORMAT};

pub const WORKSPACE_DIR_NAME: &str = "md-work";
pub const META_DIR: &str = ".mdwork";
pub const ASSETS_DIR: &str = "assets";
pub const WORKSPACE_FILE: &str = "workspace.json";
pub const NOTEBOOK_FILE: &str = "notebook.json";

pub fn documents_dir() -> Result<PathBuf> {
    if let Some(path) = known_folders::get_known_folder_path(known_folders::KnownFolder::Documents) {
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

pub fn config_path() -> PathBuf {
    app_data_dir().join("config.json")
}

pub fn session_path() -> PathBuf {
    app_data_dir().join("session.json")
}

pub fn notebook_prefs_path(notebook_id: &uuid::Uuid) -> PathBuf {
    app_data_dir()
        .join("notebooks")
        .join(notebook_id.to_string())
        .join("prefs.json")
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

pub fn workspace_meta_dir(root: &std::path::Path) -> PathBuf {
    root.join(META_DIR)
}

pub fn registry_path(root: &std::path::Path) -> PathBuf {
    workspace_meta_dir(root).join(WORKSPACE_FILE)
}

pub fn default_workspace_format() -> &'static str {
    WORKSPACE_FORMAT
}
