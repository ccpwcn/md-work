use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::atomic::write_atomic;
use crate::image::ImageHost;
use crate::paths::{config_path, default_workspace_root, session_path};
use crate::{Error, Result};

pub const CONFIG_FORMAT: &str = "md-work/config@1";
pub const SESSION_FORMAT: &str = "md-work/session@1";

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum ThemePreference {
    Light,
    Dark,
    #[default]
    System,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum PanelDock {
    #[default]
    Left,
    Right,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum PanelArrangement {
    #[default]
    Horizontal,
    Vertical,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct AppConfig {
    pub format: String,
    pub workspace_root: String,
    pub language: String,
    pub theme: ThemePreference,
    pub panel_dock: PanelDock,
    pub panel_arrangement: PanelArrangement,
    /// Sidebar width in logical pixels.
    pub panel_width: f32,
    pub ui_font: String,
    pub note_font: String,
    pub ui_font_size: f32,
    pub note_font_size: f32,
    pub autosave: bool,
    pub code_wrap: bool,
    /// Where pasted/dropped images go; `Local` keeps them in the notebook.
    pub image_host: ImageHost,
    /// Keeps the loopback MCP server running while md-work is open.
    pub mcp_http: bool,
    pub mcp_port: u16,
}

impl Default for AppConfig {
    fn default() -> Self {
        Self {
            format: CONFIG_FORMAT.to_string(),
            workspace_root: String::new(),
            language: "zh-CN".to_string(),
            theme: ThemePreference::System,
            panel_dock: PanelDock::Left,
            panel_arrangement: PanelArrangement::Horizontal,
            panel_width: 300.0,
            ui_font: String::new(),
            note_font: String::new(),
            ui_font_size: 15.0,
            note_font_size: 16.0,
            autosave: true,
            code_wrap: true,
            image_host: ImageHost::Local,
            mcp_http: false,
            mcp_port: 11026,
        }
    }
}

impl AppConfig {
    pub fn load_or_default() -> Self {
        let path = config_path();
        match Self::load_from(&path) {
            Ok(config) => config,
            Err(err) => {
                tracing::warn!(
                    path = %path.display(),
                    error = %err,
                    "配置读取失败，使用默认值"
                );
                Self::default()
            }
        }
    }

    pub fn load_from(path: &Path) -> Result<Self> {
        match fs::read(path) {
            Ok(bytes) => {
                let config: Self =
                    serde_json::from_slice(&bytes).map_err(|source| Error::Json {
                        path: path.to_path_buf(),
                        source,
                    })?;
                Ok(config)
            }
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => Ok(Self::default()),
            Err(err) => Err(Error::Io(err)),
        }
    }

    pub fn save(&self) -> Result<()> {
        self.save_to(&config_path())
    }

    pub fn save_to(&self, path: &Path) -> Result<()> {
        let result = self.write_to(path);
        match &result {
            Ok(()) => tracing::info!(
                path = %path.display(),
                theme = ?self.theme,
                language = %self.language,
                ui_font = %self.ui_font,
                note_font = %self.note_font,
                ui_font_size = self.ui_font_size,
                note_font_size = self.note_font_size,
                autosave = self.autosave,
                code_wrap = self.code_wrap,
                panel_dock = ?self.panel_dock,
                panel_arrangement = ?self.panel_arrangement,
                image_host = ?self.image_host,
                mcp_http = self.mcp_http,
                mcp_port = self.mcp_port,
                workspace_root = %self.workspace_root,
                "保存设置"
            ),
            Err(err) => tracing::error!(
                path = %path.display(),
                error = %err,
                "保存设置失败"
            ),
        }
        result
    }

    fn write_to(&self, path: &Path) -> Result<()> {
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent)?;
        }
        let bytes = serde_json::to_vec_pretty(self).map_err(|source| Error::Json {
            path: path.to_path_buf(),
            source,
        })?;
        write_atomic(path, &bytes)?;
        Ok(())
    }

    pub fn resolve_workspace_root(&self) -> Result<PathBuf> {
        let raw = self.workspace_root.trim();
        if raw.is_empty() {
            default_workspace_root()
        } else {
            Ok(PathBuf::from(raw))
        }
    }

    pub fn set_workspace_root(&mut self, root: &Path) {
        self.workspace_root = root.display().to_string();
    }
}

/// Zero-based caret position inside a note.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct CursorPos {
    pub line: u32,
    pub column: u32,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct NotebookSession {
    pub id: Uuid,
    pub open_notes: Vec<String>,
    pub active_note: Option<String>,
    pub cursors: BTreeMap<String, CursorPos>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Session {
    pub format: String,
    pub active_notebook: Option<Uuid>,
    pub notebooks: Vec<NotebookSession>,
    pub external_notes: Vec<String>,
    pub active_external: Option<String>,
    pub external_cursors: BTreeMap<String, CursorPos>,
}

impl Default for Session {
    fn default() -> Self {
        Self {
            format: SESSION_FORMAT.to_string(),
            active_notebook: None,
            notebooks: Vec::new(),
            external_notes: Vec::new(),
            active_external: None,
            external_cursors: BTreeMap::new(),
        }
    }
}

impl Session {
    pub fn load_or_default() -> Self {
        Self::load_from(&session_path()).unwrap_or_default()
    }

    pub fn load_from(path: &Path) -> Result<Self> {
        match fs::read(path) {
            Ok(bytes) => serde_json::from_slice(&bytes).map_err(|source| Error::Json {
                path: path.to_path_buf(),
                source,
            }),
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => Ok(Self::default()),
            Err(err) => Err(Error::Io(err)),
        }
    }

    pub fn save(&self) -> Result<()> {
        self.save_to(&session_path())
    }

    pub fn save_to(&self, path: &Path) -> Result<()> {
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent)?;
        }
        let bytes = serde_json::to_vec_pretty(self).map_err(|source| Error::Json {
            path: path.to_path_buf(),
            source,
        })?;
        write_atomic(path, &bytes)?;
        Ok(())
    }

    pub fn notebook(&self, id: &Uuid) -> Option<&NotebookSession> {
        self.notebooks.iter().find(|entry| entry.id == *id)
    }

    pub fn notebook_mut(&mut self, id: &Uuid) -> &mut NotebookSession {
        if let Some(index) = self.notebooks.iter().position(|entry| entry.id == *id) {
            return &mut self.notebooks[index];
        }
        self.notebooks.push(NotebookSession {
            id: *id,
            ..Default::default()
        });
        self.notebooks.last_mut().expect("pushed")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn missing_config_uses_defaults() {
        let path = std::env::temp_dir().join(format!("mdwork-config-{}.json", Uuid::new_v4()));
        let config = AppConfig::load_from(&path).unwrap();
        assert_eq!(config, AppConfig::default());
        assert_eq!(config.language, "zh-CN");
        assert!(config.resolve_workspace_root().is_ok());
    }

    #[test]
    fn config_round_trip() {
        let path = std::env::temp_dir().join(format!("mdwork-config-{}.json", Uuid::new_v4()));
        let mut config = AppConfig::default();
        config.language = "en".to_string();
        config.theme = ThemePreference::Dark;
        config.set_workspace_root(Path::new(r"D:\notes"));
        config.save_to(&path).unwrap();
        let loaded = AppConfig::load_from(&path).unwrap();
        assert_eq!(loaded, config);
        fs::remove_file(path).unwrap();
    }

    #[test]
    fn session_round_trip() {
        let path = std::env::temp_dir().join(format!("mdwork-session-{}.json", Uuid::new_v4()));
        let mut session = Session::default();
        let id = Uuid::new_v4();
        session.active_notebook = Some(id);
        let entry = session.notebook_mut(&id);
        entry.open_notes.push("a.md".to_string());
        entry.active_note = Some("a.md".to_string());
        session.save_to(&path).unwrap();
        let loaded = Session::load_from(&path).unwrap();
        assert_eq!(loaded, session);
        fs::remove_file(path).unwrap();
    }
}
