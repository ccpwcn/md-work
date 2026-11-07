//! MCP token scopes, notebook/note ACLs, and path sandbox helpers.
//!
//! Everything here is pure data plus deterministic checks so the same rules
//! run inside the GUI (token editor), the stdio server and the HTTP server.

use std::time::{SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::doc::sha256_hex;

pub const TOKEN_PREFIX: &str = "mdt";

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Scope {
    Read,
    Write,
    Search,
    Delete,
    #[serde(rename = "delete:permanent")]
    DeletePermanent,
    Export,
    Admin,
}

impl Scope {
    pub const ALL: [Scope; 7] = [
        Scope::Read,
        Scope::Write,
        Scope::Search,
        Scope::Delete,
        Scope::DeletePermanent,
        Scope::Export,
        Scope::Admin,
    ];

    pub fn as_str(self) -> &'static str {
        match self {
            Self::Read => "read",
            Self::Write => "write",
            Self::Search => "search",
            Self::Delete => "delete",
            Self::DeletePermanent => "delete:permanent",
            Self::Export => "export",
            Self::Admin => "admin",
        }
    }

    pub fn parse(raw: &str) -> Option<Self> {
        let raw = raw.trim();
        Self::ALL.into_iter().find(|scope| scope.as_str() == raw)
    }
}

/// A stored MCP token. Only the SHA-256 of the secret half is kept, so the
/// plaintext token exists just once, in the response that created it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TokenRecord {
    pub id: Uuid,
    pub name: String,
    /// Lowercase hex SHA-256 of the secret half.
    pub hash: String,
    #[serde(default)]
    pub scopes: Vec<Scope>,
    /// Notebook ids this token may touch. Empty means no notebook access.
    #[serde(default)]
    pub notebooks: Vec<Uuid>,
    /// Note path globs; when non-empty every path must match at least one.
    #[serde(default)]
    pub note_allow: Vec<String>,
    /// Note path globs; matching paths are always rejected.
    #[serde(default)]
    pub note_deny: Vec<String>,
    #[serde(default)]
    pub created: u64,
    #[serde(default)]
    pub expires: Option<u64>,
    #[serde(default)]
    pub last_used: Option<u64>,
    #[serde(default)]
    pub revoked: bool,
}

impl TokenRecord {
    pub fn new(
        name: impl Into<String>,
        hash: String,
        scopes: Vec<Scope>,
        notebooks: Vec<Uuid>,
    ) -> Self {
        Self {
            id: Uuid::new_v4(),
            name: name.into(),
            hash,
            scopes,
            notebooks,
            note_allow: Vec::new(),
            note_deny: Vec::new(),
            created: now_unix(),
            expires: None,
            last_used: None,
            revoked: false,
        }
    }

    pub fn has(&self, scope: Scope) -> bool {
        self.scopes.contains(&Scope::Admin) || self.scopes.contains(&scope)
    }

    pub fn is_active(&self, now: u64) -> bool {
        !self.revoked && self.expires.is_none_or(|expires| expires > now)
    }

    pub fn allows_notebook(&self, notebook_id: &Uuid) -> bool {
        self.notebooks.contains(notebook_id)
    }

    /// `rel_path` must already be normalized by the notebook (no `..`, no
    /// absolute paths); deny globs win over allow globs.
    pub fn allows_note(&self, notebook_id: &Uuid, rel_path: &str) -> bool {
        if !self.allows_notebook(notebook_id) {
            return false;
        }
        let path = normalize_acl_path(rel_path);
        if self
            .note_deny
            .iter()
            .any(|pattern| glob_match(pattern, &path))
        {
            return false;
        }
        if self.note_allow.is_empty() {
            return true;
        }
        self.note_allow
            .iter()
            .any(|pattern| glob_match(pattern, &path))
    }
}

/// Creates the plaintext half of a token: 32 random bytes from two v4 UUIDs.
pub fn new_secret() -> String {
    let first = Uuid::new_v4();
    let second = Uuid::new_v4();
    let mut bytes = [0u8; 32];
    bytes[..16].copy_from_slice(first.as_bytes());
    bytes[16..].copy_from_slice(second.as_bytes());
    hex(&bytes)
}

pub fn hash_secret(secret: &str) -> String {
    sha256_hex(secret.trim().as_bytes())
}

/// Full wire format handed to clients: `mdt_<uuid>_<secret>`.
pub fn format_token(id: &Uuid, secret: &str) -> String {
    format!("{TOKEN_PREFIX}_{}_{}", id.simple(), secret)
}

pub fn parse_token(raw: &str) -> Option<(Uuid, &str)> {
    let rest = raw.trim().strip_prefix(TOKEN_PREFIX)?.strip_prefix('_')?;
    let (id, secret) = rest.split_once('_')?;
    if secret.is_empty() {
        return None;
    }
    let id = Uuid::parse_str(id).ok()?;
    Some((id, secret))
}

/// Compares a stored hash against the presented secret without leaking the
/// comparison position through early exit.
pub fn secret_matches(stored_hash: &str, secret: &str) -> bool {
    let candidate = hash_secret(secret);
    let a = candidate.as_bytes();
    let b = stored_hash.trim().to_ascii_lowercase();
    let b = b.as_bytes();
    if a.len() != b.len() {
        return false;
    }
    let mut diff = 0u8;
    for (left, right) in a.iter().zip(b) {
        diff |= left ^ right;
    }
    diff == 0
}

/// Normalizes a notebook-relative path for ACL matching: slashes forward,
/// lowercase (Windows semantics), no leading `./`.
pub fn normalize_acl_path(rel_path: &str) -> String {
    let mut path = rel_path.replace('\\', "/").to_lowercase();
    while let Some(rest) = path.strip_prefix("./") {
        path = rest.to_string();
    }
    path.trim_start_matches('/').to_string()
}

/// Glob matching for note paths, case-insensitive:
/// `*` and `?` stay inside one path segment, `**` crosses segments, and
/// `**/` also matches zero segments so `**/x.md` accepts a root note.
pub fn glob_match(pattern: &str, path: &str) -> bool {
    let pattern: Vec<char> = pattern.replace('\\', "/").to_lowercase().chars().collect();
    let path: Vec<char> = path.replace('\\', "/").to_lowercase().chars().collect();
    let mut memo = vec![vec![None; path.len() + 1]; pattern.len() + 1];
    matches(&pattern, 0, &path, 0, &mut memo)
}

fn matches(
    pattern: &[char],
    p: usize,
    path: &[char],
    s: usize,
    memo: &mut [Vec<Option<bool>>],
) -> bool {
    if let Some(value) = memo[p][s] {
        return value;
    }
    let result = if p == pattern.len() {
        s == path.len()
    } else if pattern[p] == '*' && pattern.get(p + 1) == Some(&'*') {
        if pattern.get(p + 2) == Some(&'/') {
            // `**/` consumes zero or more whole segments.
            matches(pattern, p + 3, path, s, memo)
                || path[s..]
                    .iter()
                    .position(|c| *c == '/')
                    .is_some_and(|offset| matches(pattern, p, path, s + offset + 1, memo))
        } else {
            (s..=path.len()).any(|index| matches(pattern, p + 2, path, index, memo))
        }
    } else if pattern[p] == '*' {
        matches(pattern, p + 1, path, s, memo)
            || (s < path.len() && path[s] != '/' && matches(pattern, p, path, s + 1, memo))
    } else if pattern[p] == '?' {
        s < path.len() && path[s] != '/' && matches(pattern, p + 1, path, s + 1, memo)
    } else {
        s < path.len() && pattern[p] == path[s] && matches(pattern, p + 1, path, s + 1, memo)
    };
    memo[p][s] = Some(result);
    result
}

pub fn now_unix() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_secs())
        .unwrap_or_default()
}

fn hex(bytes: &[u8]) -> String {
    let mut out = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        use std::fmt::Write as _;
        let _ = write!(out, "{byte:02x}");
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn record() -> TokenRecord {
        TokenRecord::new(
            "测试令牌",
            hash_secret("secret"),
            vec![Scope::Read, Scope::Write, Scope::Search],
            vec![],
        )
    }

    #[test]
    fn scope_strings_round_trip() {
        for scope in Scope::ALL {
            assert_eq!(Scope::parse(scope.as_str()), Some(scope));
        }
        assert_eq!(Scope::parse("unknown"), None);
        assert!(record().has(Scope::Read));
        assert!(record().has(Scope::Write));
        assert!(!record().has(Scope::Delete));
        assert!(!record().has(Scope::Export));
    }

    #[test]
    fn admin_implies_every_scope() {
        let mut token = record();
        token.scopes = vec![Scope::Admin];
        for scope in Scope::ALL {
            assert!(token.has(scope), "{}", scope.as_str());
        }
    }

    #[test]
    fn token_wire_format_round_trips() {
        let id = Uuid::new_v4();
        let secret = new_secret();
        assert_eq!(secret.len(), 64);
        let wire = format_token(&id, &secret);
        let (parsed_id, parsed_secret) = parse_token(&wire).unwrap();
        assert_eq!(parsed_id, id);
        assert_eq!(parsed_secret, secret);
        assert_eq!(secret.len(), 64);
        assert!(parse_token("bare").is_none());
        assert!(parse_token("mdt_not-a-uuid_abc").is_none());
        assert!(parse_token(&format!("mdt_{}_", id.simple())).is_none());
    }

    #[test]
    fn secret_comparison_and_hashing() {
        let hash = hash_secret("hello");
        assert!(secret_matches(&hash, "hello"));
        assert!(!secret_matches(&hash, "hellp"));
        assert!(!secret_matches("短", "hello"));
    }

    #[test]
    fn expiry_and_revocation() {
        let mut token = record();
        assert!(token.is_active(now_unix()));
        token.expires = Some(now_unix() + 60);
        assert!(token.is_active(now_unix()));
        token.expires = Some(now_unix().saturating_sub(1));
        assert!(!token.is_active(now_unix()));
        token.expires = None;
        token.revoked = true;
        assert!(!token.is_active(now_unix()));
    }

    #[test]
    fn notebook_allowlist_defaults_to_deny() {
        let token = record();
        let id = Uuid::new_v4();
        assert!(!token.allows_notebook(&id));
        assert!(!token.allows_note(&id, "a.md"));
    }

    #[test]
    fn note_globs_allow_deny_and_nesting() {
        let id = Uuid::new_v4();
        let mut token = record();
        token.notebooks = vec![id];
        assert!(token.allows_note(&id, "工作/a.md"));

        token.note_allow = vec!["工作/*.md".to_string()];
        assert!(token.allows_note(&id, "工作/a.md"));
        assert!(!token.allows_note(&id, "工作/子/b.md"));
        assert!(!token.allows_note(&id, "生活/a.md"));

        token.note_allow = vec!["工作/**".to_string()];
        assert!(token.allows_note(&id, "工作/子/深/c.md"));

        token.note_deny = vec!["**/私密*".to_string()];
        assert!(!token.allows_note(&id, "工作/私密日记.md"));
        assert!(!token.allows_note(&id, "工作/子/私密.md"));
        assert!(token.allows_note(&id, "工作/公开.md"));

        token.note_allow = vec!["**".to_string()];
        token.note_deny = vec!["工作/私密.md".to_string()];
        assert!(token.allows_note(&id, "工作/公开.md"));
        assert!(!token.allows_note(&id, "工作/私密.md"));
    }

    #[test]
    fn acl_paths_are_case_and_separator_insensitive() {
        let id = Uuid::new_v4();
        let mut token = record();
        token.notebooks = vec![id];
        token.note_allow = vec![r"工作\**".to_string()];
        token.note_deny = vec!["工作/私密*".to_string()];
        assert!(token.allows_note(&id, "工作\\公开.md"));
        assert!(!token.allows_note(&id, "工作\\私密.md"));
        assert!(token.allows_note(&id, "./工作/公开.md"));
    }

    #[test]
    fn glob_specials() {
        assert!(glob_match("a?c.md", "abc.md"));
        assert!(!glob_match("a?c.md", "a/c.md"));
        assert!(glob_match("**", "任意/深层/笔记.md"));
        assert!(glob_match("**/*.md", "笔记.md"));
        assert!(glob_match("**/私密*", "x/y/私密笔记.md"));
        assert!(!glob_match("*", "x/y.md"));
        assert!(glob_match("工作/**/设计.md", "工作/项目/设计.md"));
        assert!(glob_match("工作/**/设计.md", "工作/设计.md"));
        assert!(!glob_match("工作/*", "工作/子/文件.md"));
        assert!(glob_match("A*.MD", "abc.md"));
    }

    #[test]
    fn json_shape_is_stable() {
        let mut token = record();
        token.expires = Some(123);
        let value = serde_json::to_value(&token).unwrap();
        assert_eq!(value["scopes"][0], "read");
        assert_eq!(value["revoked"], false);
        assert_eq!(value["expires"], 123);
        let back: TokenRecord = serde_json::from_value(value).unwrap();
        assert_eq!(back, token);
    }
}
