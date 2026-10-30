//! MCP token storage: SHA-256 hashes at rest, DPAPI user-scoped encryption.

use std::fs;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use anyhow::{Context as _, Result};
use mdwork_core::atomic::write_atomic;
use mdwork_core::paths::app_data_dir;
use mdwork_core::permissions::{self, Scope, TokenRecord};
use serde::{Deserialize, Serialize};
use uuid::Uuid;
use windows::Win32::Foundation::{HLOCAL, LocalFree};
use windows::Win32::Security::Cryptography::{
    CRYPT_INTEGER_BLOB, CRYPTPROTECT_UI_FORBIDDEN, CryptProtectData, CryptUnprotectData,
};
use windows::core::PCWSTR;

pub const TOKENS_FORMAT: &str = "md-work/mcp-tokens@1";
/// Writes `last_used` back to disk at most once per minute per token.
const LAST_USED_REFRESH_SECS: u64 = 60;

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
struct TokensFile {
    format: String,
    tokens: Vec<TokenRecord>,
}

impl Default for TokensFile {
    fn default() -> Self {
        Self {
            format: TOKENS_FORMAT.to_string(),
            tokens: Vec::new(),
        }
    }
}

pub struct TokenStore {
    path: PathBuf,
}

impl TokenStore {
    pub fn new(path: impl Into<PathBuf>) -> Self {
        Self { path: path.into() }
    }

    pub fn default_path() -> PathBuf {
        app_data_dir().join("mcp").join("tokens.bin")
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    pub fn list(&self) -> Result<Vec<TokenRecord>> {
        Ok(self.load()?.tokens)
    }

    pub fn create(
        &self,
        name: &str,
        scopes: Vec<Scope>,
        notebooks: Vec<Uuid>,
        note_allow: Vec<String>,
        note_deny: Vec<String>,
    ) -> Result<(TokenRecord, String)> {
        let name = name.trim();
        if name.is_empty() {
            anyhow::bail!("令牌名称不能为空");
        }
        let secret = permissions::new_secret();
        let mut record =
            TokenRecord::new(name, permissions::hash_secret(&secret), scopes, notebooks);
        record.note_allow = note_allow;
        record.note_deny = note_deny;
        let mut file = self.load()?;
        file.tokens.push(record.clone());
        self.save(&file)?;
        let wire = permissions::format_token(&record.id, &secret);
        Ok((record, wire))
    }

    pub fn update(&self, record: TokenRecord) -> Result<()> {
        let mut file = self.load()?;
        let Some(existing) = file.tokens.iter_mut().find(|entry| entry.id == record.id) else {
            anyhow::bail!("令牌不存在");
        };
        *existing = record;
        self.save(&file)
    }

    pub fn revoke(&self, id: &Uuid) -> Result<()> {
        let mut file = self.load()?;
        let Some(existing) = file.tokens.iter_mut().find(|entry| entry.id == *id) else {
            anyhow::bail!("令牌不存在");
        };
        existing.revoked = true;
        self.save(&file)
    }

    pub fn delete(&self, id: &Uuid) -> Result<()> {
        let mut file = self.load()?;
        file.tokens.retain(|entry| entry.id != *id);
        self.save(&file)
    }

    /// Resolves a bearer token to its record and refreshes `last_used`
    /// opportunistically.
    pub fn verify(&self, raw: &str) -> Result<TokenRecord> {
        let (id, secret) = permissions::parse_token(raw).context("令牌格式无效")?;
        let mut file = self.load()?;
        let now = permissions::now_unix();
        let Some(record) = file.tokens.iter_mut().find(|entry| entry.id == id) else {
            anyhow::bail!("令牌无效");
        };
        if !record.is_active(now) {
            anyhow::bail!("令牌已过期或已吊销");
        }
        if !permissions::secret_matches(&record.hash, secret) {
            anyhow::bail!("令牌无效");
        }
        let stale = record
            .last_used
            .is_none_or(|last| now.saturating_sub(last) >= LAST_USED_REFRESH_SECS);
        if stale {
            record.last_used = Some(now);
            let snapshot = record.clone();
            let _ = self.save(&file);
            return Ok(snapshot);
        }
        Ok(record.clone())
    }

    fn load(&self) -> Result<TokensFile> {
        let bytes = match fs::read(&self.path) {
            Ok(bytes) => bytes,
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => {
                return Ok(TokensFile::default());
            }
            Err(err) => return Err(err).with_context(|| format!("读取 {}", self.path.display())),
        };
        let plain =
            unprotect(&bytes).with_context(|| format!("解密令牌文件 {}", self.path.display()))?;
        let file: TokensFile = serde_json::from_slice(&plain)
            .with_context(|| format!("解析令牌文件 {}", self.path.display()))?;
        if file.format != TOKENS_FORMAT {
            anyhow::bail!("令牌文件格式不受支持：{}", file.format);
        }
        Ok(file)
    }

    fn save(&self, file: &TokensFile) -> Result<()> {
        if let Some(parent) = self.path.parent() {
            fs::create_dir_all(parent)?;
        }
        let plain = serde_json::to_vec(file)?;
        let sealed = protect(&plain).context("DPAPI 加密失败")?;
        write_atomic(&self.path, &sealed)?;
        Ok(())
    }
}

fn protect(data: &[u8]) -> Result<Vec<u8>> {
    let input = CRYPT_INTEGER_BLOB {
        cbData: data.len() as u32,
        pbData: data.as_ptr() as *mut u8,
    };
    let mut output = CRYPT_INTEGER_BLOB::default();
    unsafe {
        CryptProtectData(
            &input,
            PCWSTR::null(),
            None,
            None,
            None,
            CRYPTPROTECT_UI_FORBIDDEN,
            &mut output,
        )?;
        let bytes = std::slice::from_raw_parts(output.pbData, output.cbData as usize).to_vec();
        let _ = LocalFree(Some(HLOCAL(output.pbData as *mut core::ffi::c_void)));
        Ok(bytes)
    }
}

fn unprotect(data: &[u8]) -> Result<Vec<u8>> {
    let input = CRYPT_INTEGER_BLOB {
        cbData: data.len() as u32,
        pbData: data.as_ptr() as *mut u8,
    };
    let mut output = CRYPT_INTEGER_BLOB::default();
    unsafe {
        CryptUnprotectData(
            &input,
            None,
            None,
            None,
            None,
            CRYPTPROTECT_UI_FORBIDDEN,
            &mut output,
        )?;
        let bytes = std::slice::from_raw_parts(output.pbData, output.cbData as usize).to_vec();
        let _ = LocalFree(Some(HLOCAL(output.pbData as *mut core::ffi::c_void)));
        Ok(bytes)
    }
}

/// Seconds since the Unix epoch; exposed for audit timestamps.
pub fn now_unix() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_secs())
        .unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_store() -> (PathBuf, TokenStore) {
        let dir = std::env::temp_dir().join(format!("mdwork-tokens-{}", Uuid::new_v4()));
        let store = TokenStore::new(dir.join("tokens.bin"));
        (dir, store)
    }

    #[test]
    fn dpapi_round_trip_hides_plaintext() {
        let (dir, store) = temp_store();
        let id = Uuid::new_v4();
        let (_, wire) = store
            .create("测试", vec![Scope::Read], vec![id], vec![], vec![])
            .unwrap();
        let raw = fs::read(store.path()).unwrap();
        assert!(!raw.windows(11).any(|window| window == b"md-work/mcp"));
        let (parsed, _) = permissions::parse_token(&wire).unwrap();
        assert_eq!(parsed, store.list().unwrap()[0].id);
        fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn verify_accepts_valid_token_and_rejects_tampering() {
        let (dir, store) = temp_store();
        let notebook = Uuid::new_v4();
        let (record, wire) = store
            .create(
                "代理",
                vec![Scope::Read, Scope::Search],
                vec![notebook],
                vec![],
                vec![],
            )
            .unwrap();
        let found = store.verify(&wire).unwrap();
        assert_eq!(found.id, record.id);
        assert!(found.last_used.is_some());

        let tampered = format!("{}x", &wire[..wire.len() - 1]);
        assert!(store.verify(&tampered).is_err());
        assert!(store.verify("mdt_deadbeef_abc").is_err());

        store.revoke(&record.id).unwrap();
        assert!(store.verify(&wire).is_err());
        fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn update_and_delete_round_trip() {
        let (dir, store) = temp_store();
        let (mut record, _) = store
            .create("A", vec![Scope::Read], vec![], vec![], vec![])
            .unwrap();
        record.name = "B".to_string();
        record.scopes.push(Scope::Export);
        store.update(record.clone()).unwrap();
        let listed = store.list().unwrap();
        assert_eq!(listed[0].name, "B");
        assert_eq!(listed[0].scopes, vec![Scope::Read, Scope::Export]);
        store.delete(&record.id).unwrap();
        assert!(store.list().unwrap().is_empty());
        fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn missing_file_starts_empty() {
        let (dir, store) = temp_store();
        assert!(store.list().unwrap().is_empty());
        assert!(store.verify("mdt_x_y").is_err());
        let _ = fs::remove_dir_all(dir);
    }
}
