//! TOML persistence for user-editable local files.
//!
//! Loading prefers `foo.toml`. When only the legacy `foo.json` sibling exists
//! it is parsed once, rewritten as TOML and renamed to `foo.json.migrated`, so
//! users only have to read and edit one file from then on.

use std::fs;
use std::path::{Path, PathBuf};

use serde::Serialize;
use serde::de::DeserializeOwned;

use crate::atomic::write_atomic;
use crate::{Error, Result};

pub fn legacy_path(path: &Path) -> PathBuf {
    path.with_extension("json")
}

pub fn read_table(path: &Path) -> Result<Option<toml::Table>> {
    let text = match fs::read_to_string(path) {
        Ok(text) => text,
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(err) => return Err(Error::Io(err)),
    };
    let table = toml::from_str::<toml::Table>(&text).map_err(|source| Error::Toml {
        path: path.to_path_buf(),
        source: Box::new(source),
    })?;
    Ok(Some(table))
}

pub fn load<T: DeserializeOwned>(path: &Path) -> Result<Option<T>> {
    let Some(table) = read_table(path)? else {
        return Ok(None);
    };
    let value = table.try_into().map_err(|source| Error::Toml {
        path: path.to_path_buf(),
        source: Box::new(source),
    })?;
    Ok(Some(value))
}

/// Loads `path`, falling back to the legacy JSON sibling exactly once.
/// A corrupt TOML file also falls back to the legacy JSON: a half-written
/// file must not silently turn into an empty configuration.
pub fn load_with_legacy<T: DeserializeOwned + Serialize>(path: &Path) -> Result<Option<T>> {
    let toml_error = match load(path) {
        Ok(Some(value)) => return Ok(Some(value)),
        Ok(None) => None,
        Err(err) => {
            tracing::warn!(path = %path.display(), error = %err, "TOML 配置解析失败，尝试旧 JSON 备份");
            Some(err)
        }
    };
    let legacy = legacy_path(path);
    if legacy == path {
        return match toml_error {
            Some(err) => Err(err),
            None => Ok(None),
        };
    }
    let bytes = match fs::read(&legacy) {
        Ok(bytes) => bytes,
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => {
            return match toml_error {
                Some(err) => Err(err),
                None => Ok(None),
            };
        }
        Err(err) => return Err(Error::Io(err)),
    };
    let value: T = serde_json::from_slice(&bytes).map_err(|source| Error::Json {
        path: legacy.clone(),
        source,
    })?;
    if toml_error.is_some() {
        backup_corrupt(path);
    }
    match save(path, &value) {
        Ok(()) => {
            archive_legacy(&legacy);
            tracing::info!(legacy = %legacy.display(), path = %path.display(), "已迁移 JSON 配置为 TOML");
        }
        Err(err) => {
            tracing::warn!(
                legacy = %legacy.display(),
                path = %path.display(),
                error = %err,
                "旧 JSON 配置已读取，但写入 TOML 失败"
            );
        }
    }
    Ok(Some(value))
}

pub fn save<T: Serialize>(path: &Path, value: &T) -> Result<()> {
    let text = toml::to_string_pretty(value).map_err(|source| Error::TomlWrite {
        path: path.to_path_buf(),
        source,
    })?;
    save_text(path, &text)
}

pub fn save_with_header<T: Serialize>(path: &Path, header: &str, value: &T) -> Result<()> {
    let text = toml::to_string_pretty(value).map_err(|source| Error::TomlWrite {
        path: path.to_path_buf(),
        source,
    })?;
    let body = if header.trim().is_empty() {
        text
    } else {
        format!("{}\n{text}", header.trim_end())
    };
    save_text(path, &body)
}

pub fn save_text(path: &Path, text: &str) -> Result<()> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    write_atomic(path, text.as_bytes())?;
    Ok(())
}

fn backup_corrupt(path: &Path) {
    let mut name = path.as_os_str().to_os_string();
    name.push(".corrupt");
    let backup = PathBuf::from(name);
    if !backup.exists()
        && let Err(err) = fs::rename(path, &backup)
    {
        tracing::warn!(path = %path.display(), error = %err, "备份损坏的 TOML 失败");
    }
}

/// Renames a migrated JSON file to `<name>.migrated` so it is not re-read.
pub fn archive_legacy(path: &Path) {
    let Some(name) = path
        .file_name()
        .map(|name| name.to_string_lossy().into_owned())
    else {
        return;
    };
    let archived = path.with_file_name(format!("{name}.migrated"));
    if archived.exists() {
        return;
    }
    if let Err(err) = fs::rename(path, &archived) {
        tracing::warn!(path = %path.display(), error = %err, "归档旧 JSON 配置失败");
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde::Deserialize;

    #[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
    #[serde(default)]
    struct Sample {
        format: String,
        enabled: bool,
    }

    impl Default for Sample {
        fn default() -> Self {
            Self {
                format: "md-work/sample@1".to_string(),
                enabled: true,
            }
        }
    }

    fn temp_dir() -> PathBuf {
        std::env::temp_dir().join(format!("mdwork-toml-{}", uuid::Uuid::new_v4()))
    }

    #[test]
    fn round_trip() {
        let dir = temp_dir();
        let path = dir.join("sample.toml");
        let sample = Sample {
            enabled: false,
            ..Default::default()
        };
        save(&path, &sample).unwrap();
        assert_eq!(load::<Sample>(&path).unwrap(), Some(sample));
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn legacy_json_is_migrated_once() {
        let dir = temp_dir();
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("sample.toml");
        let legacy = dir.join("sample.json");
        std::fs::write(&legacy, br#"{"format":"md-work/sample@1","enabled":false}"#).unwrap();

        let loaded = load_with_legacy::<Sample>(&path).unwrap().unwrap();
        assert!(!loaded.enabled);
        assert!(path.is_file());
        assert!(!legacy.exists());
        assert!(dir.join("sample.json.migrated").is_file());
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn missing_file_is_none() {
        let dir = temp_dir();
        assert_eq!(load::<Sample>(&dir.join("nope.toml")).unwrap(), None);
    }

    #[test]
    fn corrupt_toml_falls_back_to_legacy_json() {
        let dir = temp_dir();
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("sample.toml");
        let legacy = dir.join("sample.json");
        std::fs::write(&path, b"this = [not valid").unwrap();
        std::fs::write(&legacy, br#"{"format":"md-work/sample@1","enabled":false}"#).unwrap();

        let loaded = load_with_legacy::<Sample>(&path).unwrap().unwrap();
        assert!(!loaded.enabled);
        assert_eq!(load::<Sample>(&path).unwrap(), Some(loaded));
        assert!(dir.join("sample.toml.corrupt").is_file());
        assert!(dir.join("sample.json.migrated").is_file());
        std::fs::remove_dir_all(dir).unwrap();
    }
}
