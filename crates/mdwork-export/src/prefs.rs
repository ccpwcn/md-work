//! Per-notebook preferences stored in the application data directory.

use std::fs;
use std::path::{Path, PathBuf};

use anyhow::Result;
use mdwork_core::atomic::write_atomic;
use mdwork_core::paths::notebook_prefs_path;
use mdwork_render::typst::ExportOptions;
use serde::{Deserialize, Serialize};
use uuid::Uuid;

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct NotebookPrefs {
    pub last_export_pdf_dir: Option<String>,
    pub last_export_png_dir: Option<String>,
    pub last_zip_dir: Option<String>,
    pub last_import_dir: Option<String>,
    pub export: ExportOptions,
}

impl NotebookPrefs {
    pub fn load(notebook_id: &Uuid) -> Self {
        Self::load_from(&notebook_prefs_path(notebook_id)).unwrap_or_default()
    }

    pub fn load_from(path: &Path) -> Result<Self> {
        match fs::read(path) {
            Ok(bytes) => Ok(serde_json::from_slice(&bytes)?),
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => Ok(Self::default()),
            Err(err) => Err(err.into()),
        }
    }

    pub fn save(&self, notebook_id: &Uuid) -> Result<()> {
        self.save_to(&notebook_prefs_path(notebook_id))
    }

    pub fn save_to(&self, path: &Path) -> Result<()> {
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent)?;
        }
        write_atomic(path, &serde_json::to_vec_pretty(self)?)?;
        Ok(())
    }

    pub fn export_dir(&self, pdf: bool) -> Option<PathBuf> {
        let saved = if pdf {
            &self.last_export_pdf_dir
        } else {
            &self.last_export_png_dir
        };
        saved
            .as_deref()
            .map(PathBuf::from)
            .filter(|path| path.is_dir())
    }

    pub fn set_export_dir(&mut self, pdf: bool, dir: &Path) {
        let value = Some(dir.display().to_string());
        if pdf {
            self.last_export_pdf_dir = value;
        } else {
            self.last_export_png_dir = value;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trip_prefs() {
        let path = std::env::temp_dir().join(format!("mdwork-prefs-{}.json", Uuid::new_v4()));
        let mut prefs = NotebookPrefs::default();
        prefs.last_export_pdf_dir = Some(r"D:\out".to_string());
        prefs.export.cover = true;
        prefs.export.toc_depth = 5;
        prefs.save_to(&path).unwrap();
        let loaded = NotebookPrefs::load_from(&path).unwrap();
        assert_eq!(loaded.last_export_pdf_dir, prefs.last_export_pdf_dir);
        assert!(loaded.export.cover);
        assert_eq!(loaded.export.toc_depth, 5);
        fs::remove_file(path).unwrap();
    }

    #[test]
    fn missing_prefs_use_defaults() {
        let path = std::env::temp_dir().join(format!("mdwork-prefs-{}.json", Uuid::new_v4()));
        let prefs = NotebookPrefs::load_from(&path).unwrap();
        assert_eq!(prefs.export, ExportOptions::default());
    }

    #[test]
    fn missing_directories_are_ignored() {
        let mut prefs = NotebookPrefs::default();
        prefs.last_export_pdf_dir = Some(r"Z:\definitely-missing\folder".to_string());
        assert!(prefs.export_dir(true).is_none());
    }
}
