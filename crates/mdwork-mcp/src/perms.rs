//! MCP 权限配置存储：TOML 明文 + HMAC-SHA256 分离签名，密钥 DPAPI 加密。
//!
//! 外部程序可以改写 permissions.toml，但没有 DPAPI 保护的密钥就无法伪造
//! 签名；加载验签失败一律失效关闭（全部隐藏），篡改无法用来提权。

use std::fs;
use std::path::{Path, PathBuf};

use anyhow::{Context as _, Result};
use hmac::{Hmac, Mac};
use mdwork_core::atomic::write_atomic;
use mdwork_core::mcp_permissions::{PERMISSIONS_FORMAT, PermissionTree};
use mdwork_core::paths::app_data_dir;
use sha2::Sha256;
use uuid::Uuid;

use crate::auth::{protect, unprotect};

/// 验签结果：tampered 时 tree 为全隐藏默认树，原文件保留供检查。
#[derive(Debug, Clone)]
pub struct LoadedPermissions {
    /// 生效的权限树；验签失败时为全隐藏默认树。
    pub tree: PermissionTree,
    /// 文件存在但验签/解析失败（可能被篡改）。
    pub tampered: bool,
}

/// 签名权限存储：permissions.toml + permissions.toml.sig + permissions.key。
pub struct PermissionStore {
    path: PathBuf,
}

impl PermissionStore {
    /// `path` 指向 permissions.toml 本体。
    pub fn new(path: impl Into<PathBuf>) -> Self {
        Self { path: path.into() }
    }

    /// 默认位置：%LOCALAPPDATA%/md-work/mcp/permissions.toml。
    pub fn default_path() -> PathBuf {
        app_data_dir().join("mcp").join("permissions.toml")
    }

    /// 权限文件本体路径。
    pub fn path(&self) -> &Path {
        &self.path
    }

    /// 加载并验签；任何校验失败都失效关闭（全部隐藏 + tampered 标记）。
    pub fn load(&self) -> LoadedPermissions {
        match self.try_load() {
            Ok(Some(tree)) => LoadedPermissions {
                tree,
                tampered: false,
            },
            Ok(None) => LoadedPermissions {
                tree: PermissionTree::default(),
                tampered: false,
            },
            Err(err) => {
                tracing::warn!(
                    path = %self.path.display(),
                    error = %err,
                    "MCP 权限文件校验失败，按全部隐藏处理"
                );
                LoadedPermissions {
                    tree: PermissionTree::default(),
                    tampered: true,
                }
            }
        }
    }

    /// 保存并重新签名；密钥不存在时生成并以 DPAPI 密封落盘。
    pub fn save(&self, tree: &PermissionTree) -> Result<()> {
        let key = self.ensure_key()?;
        let text = toml::to_string_pretty(tree).context("序列化权限配置")?;
        if let Some(parent) = self.path.parent() {
            fs::create_dir_all(parent)?;
        }
        write_atomic(&self.path, text.as_bytes())?;
        // 先写本体后写签名：中途崩溃会让下次加载验签失败，方向是失效关闭。
        write_atomic(&self.sig_path(), &sign(&key, text.as_bytes()))?;
        Ok(())
    }

    fn sig_path(&self) -> PathBuf {
        let mut name = self
            .path
            .file_name()
            .map(|name| name.to_os_string())
            .unwrap_or_default();
        name.push(".sig");
        self.path.with_file_name(name)
    }

    fn key_path(&self) -> PathBuf {
        self.path
            .parent()
            .unwrap_or(Path::new("."))
            .join("permissions.key")
    }

    fn load_key(&self) -> Result<Option<[u8; 32]>> {
        let bytes = match fs::read(self.key_path()) {
            Ok(bytes) => bytes,
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => return Ok(None),
            Err(err) => {
                return Err(err).with_context(|| format!("读取密钥 {}", self.key_path().display()));
            }
        };
        let plain = unprotect(&bytes).context("解密权限签名密钥")?;
        let key: [u8; 32] = plain
            .try_into()
            .map_err(|_| anyhow::anyhow!("权限签名密钥长度非法"))?;
        Ok(Some(key))
    }

    fn ensure_key(&self) -> Result<[u8; 32]> {
        if let Some(key) = self.load_key()? {
            return Ok(key);
        }
        let mut key = [0u8; 32];
        key[..16].copy_from_slice(Uuid::new_v4().as_bytes());
        key[16..].copy_from_slice(Uuid::new_v4().as_bytes());
        let sealed = protect(&key).context("DPAPI 加密权限签名密钥")?;
        if let Some(parent) = self.key_path().parent() {
            fs::create_dir_all(parent)?;
        }
        write_atomic(&self.key_path(), &sealed)?;
        Ok(key)
    }

    fn try_load(&self) -> Result<Option<PermissionTree>> {
        let bytes = match fs::read(&self.path) {
            Ok(bytes) => bytes,
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => return Ok(None),
            Err(err) => return Err(err).with_context(|| format!("读取 {}", self.path.display())),
        };
        let sig = fs::read(self.sig_path())
            .with_context(|| format!("读取签名 {}", self.sig_path().display()))?;
        let key = self.load_key()?.context("签名密钥不存在")?;
        let mut mac = Hmac::<Sha256>::new_from_slice(&key).expect("HMAC 接受任意长度密钥");
        mac.update(&bytes);
        mac.verify_slice(&sig)
            .map_err(|_| anyhow::anyhow!("权限文件签名不匹配"))?;
        let text = std::str::from_utf8(&bytes).context("权限文件不是 UTF-8")?;
        let tree: PermissionTree = toml::from_str(text).context("解析权限文件")?;
        if tree.format != PERMISSIONS_FORMAT {
            anyhow::bail!("权限文件格式不受支持：{}", tree.format);
        }
        Ok(Some(tree))
    }
}

fn sign(key: &[u8; 32], bytes: &[u8]) -> [u8; 32] {
    let mut mac = Hmac::<Sha256>::new_from_slice(key).expect("HMAC 接受任意长度密钥");
    mac.update(bytes);
    mac.finalize().into_bytes().into()
}

#[cfg(test)]
mod tests {
    use super::*;
    use mdwork_core::mcp_permissions::{NodeKey, PermLevel};

    fn temp_store() -> (PathBuf, PermissionStore) {
        let dir = std::env::temp_dir().join(format!("mdwork-perms-{}", Uuid::new_v4()));
        let store = PermissionStore::new(dir.join("permissions.toml"));
        (dir, store)
    }

    fn sample_tree() -> PermissionTree {
        let mut tree = PermissionTree::default();
        tree.set_level(&NodeKey::notebook(Uuid::new_v4()), PermLevel::Read, &[]);
        tree
    }

    #[test]
    fn missing_file_loads_default_hidden() {
        let (dir, store) = temp_store();
        let loaded = store.load();
        assert!(!loaded.tampered);
        assert_eq!(loaded.tree, PermissionTree::default());
        let _ = fs::remove_dir_all(dir);
    }

    #[test]
    fn save_and_load_round_trip() {
        let (dir, store) = temp_store();
        let tree = sample_tree();
        store.save(&tree).unwrap();
        // TOML 本体保持明文可读。
        let text = fs::read_to_string(store.path()).unwrap();
        assert!(text.contains("\"read\""));
        assert!(store.sig_path().is_file());
        assert!(store.key_path().is_file());

        let loaded = store.load();
        assert!(!loaded.tampered);
        assert_eq!(loaded.tree, tree);
        fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn tampered_toml_fails_closed() {
        let (dir, store) = temp_store();
        store.save(&sample_tree()).unwrap();
        // 外部程序把只读改成删除：TOML 仍合法，但签名不再匹配。
        let text = fs::read_to_string(store.path()).unwrap();
        let forged = text.replace("\"read\"", "\"delete\"");
        assert_ne!(forged, text);
        fs::write(store.path(), &forged).unwrap();

        let loaded = store.load();
        assert!(loaded.tampered);
        assert_eq!(loaded.tree, PermissionTree::default());
        // 原文件保留供用户检查，不被自动改写。
        assert_eq!(fs::read_to_string(store.path()).unwrap(), forged);
        fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn tampered_or_missing_sig_fails_closed() {
        let (dir, store) = temp_store();
        store.save(&sample_tree()).unwrap();
        let mut sig = fs::read(store.sig_path()).unwrap();
        sig[0] ^= 0xff;
        fs::write(store.sig_path(), &sig).unwrap();
        assert!(store.load().tampered);

        fs::remove_file(store.sig_path()).unwrap();
        assert!(store.load().tampered);
        fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn missing_key_fails_closed() {
        let (dir, store) = temp_store();
        store.save(&sample_tree()).unwrap();
        fs::remove_file(store.key_path()).unwrap();
        let loaded = store.load();
        assert!(loaded.tampered);
        assert_eq!(loaded.tree, PermissionTree::default());
        fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn resave_after_tamper_restores() {
        let (dir, store) = temp_store();
        store.save(&sample_tree()).unwrap();
        fs::write(store.path(), "forged").unwrap();
        assert!(store.load().tampered);

        let tree = sample_tree();
        store.save(&tree).unwrap();
        let loaded = store.load();
        assert!(!loaded.tampered);
        assert_eq!(loaded.tree, tree);
        fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn key_persists_across_store_instances() {
        let (dir, store) = temp_store();
        let tree = sample_tree();
        store.save(&tree).unwrap();
        let second = PermissionStore::new(store.path());
        let loaded = second.load();
        assert!(!loaded.tampered);
        assert_eq!(loaded.tree, tree);
        fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn invalid_toml_with_valid_signature_fails_closed() {
        let (dir, store) = temp_store();
        fs::create_dir_all(&dir).unwrap();
        let key = store.ensure_key().unwrap();
        let garbage = b"not [ valid toml";
        fs::write(store.path(), garbage).unwrap();
        fs::write(store.sig_path(), sign(&key, garbage)).unwrap();
        assert!(store.load().tampered);
        fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn wrong_format_fails_closed() {
        let (dir, store) = temp_store();
        let mut tree = sample_tree();
        tree.format = "other-format".to_string();
        store.save(&tree).unwrap();
        assert!(store.load().tampered);
        fs::remove_dir_all(dir).unwrap();
    }
}
