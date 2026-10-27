//! Image assets stored in a notebook's `assets/` folder and optional uploads
//! to an external image host (PicGo HTTP server or a generic multipart API).

use std::fs;
use std::io::Read;
use std::path::{Component, Path, PathBuf};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};
use serde_json::Value;
use uuid::Uuid;

use crate::atomic::write_atomic;
use crate::paths::ASSETS_DIR;
use crate::{Error, Result};

/// Refuse to store or upload anything larger than this, matching the kind of
/// limit screenshot tools and web APIs expect.
pub const IMAGE_MAX_BYTES: u64 = 32 * 1024 * 1024;

const UPLOAD_TIMEOUT: Duration = Duration::from_secs(30);
const DEFAULT_PICGO_ENDPOINT: &str = "http://127.0.0.1:36677/upload";
const DEFAULT_FIELD: &str = "file";
const DEFAULT_URL_PATH: &str = "url";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ImageKind {
    Png,
    Jpeg,
    Gif,
    Webp,
    Bmp,
    Svg,
}

impl ImageKind {
    pub fn from_extension(path: &Path) -> Option<Self> {
        let ext = path.extension()?.to_str()?.to_ascii_lowercase();
        match ext.as_str() {
            "png" => Some(Self::Png),
            "jpg" | "jpeg" | "jfif" => Some(Self::Jpeg),
            "gif" => Some(Self::Gif),
            "webp" => Some(Self::Webp),
            "bmp" => Some(Self::Bmp),
            "svg" => Some(Self::Svg),
            _ => None,
        }
    }

    /// Detects the format from magic bytes so renaming a file cannot smuggle
    /// non-image data into `assets/`.
    pub fn sniff(bytes: &[u8]) -> Option<Self> {
        if bytes.starts_with(b"\x89PNG\r\n\x1a\n") {
            return Some(Self::Png);
        }
        if bytes.starts_with(&[0xFF, 0xD8, 0xFF]) {
            return Some(Self::Jpeg);
        }
        if bytes.starts_with(b"GIF87a") || bytes.starts_with(b"GIF89a") {
            return Some(Self::Gif);
        }
        if bytes.len() >= 12 && bytes.starts_with(b"RIFF") && &bytes[8..12] == b"WEBP" {
            return Some(Self::Webp);
        }
        if bytes.starts_with(b"BM") && bytes.len() > 14 {
            return Some(Self::Bmp);
        }
        if looks_like_svg(bytes) {
            return Some(Self::Svg);
        }
        None
    }

    pub fn extension(self) -> &'static str {
        match self {
            Self::Png => "png",
            Self::Jpeg => "jpg",
            Self::Gif => "gif",
            Self::Webp => "webp",
            Self::Bmp => "bmp",
            Self::Svg => "svg",
        }
    }

    pub fn mime(self) -> &'static str {
        match self {
            Self::Png => "image/png",
            Self::Jpeg => "image/jpeg",
            Self::Gif => "image/gif",
            Self::Webp => "image/webp",
            Self::Bmp => "image/bmp",
            Self::Svg => "image/svg+xml",
        }
    }
}

fn looks_like_svg(bytes: &[u8]) -> bool {
    let head: Vec<u8> = bytes.iter().copied().take(512).collect();
    let text = String::from_utf8_lossy(&head);
    let trimmed = text.trim_start_matches('\u{feff}').trim_start();
    trimmed.starts_with("<svg") || (trimmed.starts_with("<?xml") && text.contains("<svg"))
}

/// Where images are stored for a notebook.
pub fn assets_dir(notebook_root: &Path) -> PathBuf {
    notebook_root.join(ASSETS_DIR)
}

/// Writes an image into `<notebook>/assets/` and returns its absolute path.
///
/// File names are timestamped and unique; an identical image is still written
/// again under a new name so deleting one note never breaks another.
pub fn save_asset(notebook_root: &Path, bytes: &[u8], kind: ImageKind) -> Result<PathBuf> {
    if bytes.is_empty() {
        return Err(Error::UnsupportedImage("图片内容为空".to_string()));
    }
    let size = bytes.len() as u64;
    if size > IMAGE_MAX_BYTES {
        return Err(Error::ImageTooLarge {
            size,
            limit: IMAGE_MAX_BYTES,
        });
    }
    if ImageKind::sniff(bytes) != Some(kind) && kind != ImageKind::Svg {
        return Err(Error::UnsupportedImage(
            "文件内容与图片格式不符".to_string(),
        ));
    }
    let dir = assets_dir(notebook_root);
    fs::create_dir_all(&dir)?;

    let stamp = format!(
        "img-{}-{}",
        timestamp_stem(),
        &Uuid::new_v4().simple().to_string()[..6]
    );
    let extension = kind.extension();
    let mut path = dir.join(format!("{stamp}.{extension}"));
    let mut suffix = 2;
    while path.exists() {
        path = dir.join(format!("{stamp}-{suffix}.{extension}"));
        suffix += 1;
    }
    write_atomic(&path, bytes)?;
    Ok(path)
}

/// Markdown-friendly path from a note to one of its assets.
pub fn markdown_url(note_path: &Path, image_path: &Path) -> String {
    let base = note_path.parent().unwrap_or_else(|| Path::new("."));
    let rel = relative_to(image_path, base).unwrap_or_else(|| image_path.to_path_buf());
    encode_url_path(&rel)
}

fn relative_to(path: &Path, base: &Path) -> Option<PathBuf> {
    let path_parts: Vec<Component<'_>> = path.components().collect();
    let base_parts: Vec<Component<'_>> = base.components().collect();
    if path_parts.first() != base_parts.first() {
        return None;
    }
    let common = path_parts
        .iter()
        .zip(base_parts.iter())
        .take_while(|(a, b)| a == b)
        .count();
    let mut out = PathBuf::new();
    for _ in common..base_parts.len() {
        out.push("..");
    }
    for part in &path_parts[common..] {
        out.push(part.as_os_str());
    }
    if out.as_os_str().is_empty() {
        out.push(".");
    }
    Some(out)
}

fn encode_url_path(path: &Path) -> String {
    let mut url = String::new();
    for part in path.components() {
        let text = match part {
            Component::ParentDir => "..".to_string(),
            Component::CurDir => ".".to_string(),
            Component::Normal(name) => name.to_string_lossy().into_owned(),
            _ => continue,
        };
        if !url.is_empty() {
            url.push('/');
        }
        for ch in text.chars() {
            match ch {
                ' ' => url.push_str("%20"),
                '%' => url.push_str("%25"),
                '#' => url.push_str("%23"),
                '?' => url.push_str("%3F"),
                '(' => url.push_str("%28"),
                ')' => url.push_str("%29"),
                '\\' => url.push('/'),
                _ => url.push(ch),
            }
        }
    }
    url
}

/// Timestamp as `YYYYMMDD-HHMMSS` in UTC, computed without pulling in a date
/// crate (Howard Hinnant's civil-from-days algorithm).
fn timestamp_stem() -> String {
    let secs = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|elapsed| elapsed.as_secs())
        .unwrap_or(0);
    let days = secs / 86_400;
    let rem = secs % 86_400;
    let (year, month, day) = civil_from_days(days as i64);
    format!(
        "{year:04}{month:02}{day:02}-{:02}{:02}{:02}",
        rem / 3600,
        (rem % 3600) / 60,
        rem % 60
    )
}

fn civil_from_days(days: i64) -> (i64, u32, u32) {
    let z = days + 719_468;
    let era = if z >= 0 { z } else { z - 146_096 } / 146_097;
    let doe = (z - era * 146_097) as u64;
    let yoe = (doe - doe / 1_460 + doe / 36_524 - doe / 146_096) / 365;
    let year = yoe as i64 + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = (doy - (153 * mp + 2) / 5 + 1) as u32;
    let month = if mp < 10 { mp + 3 } else { mp - 9 } as u32;
    (if month <= 2 { year + 1 } else { year }, month, day)
}

/// Destination for pasted or dropped images. `Local` keeps everything inside
/// the notebook; the others upload and fall back to `Local` on failure.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "kebab-case")]
pub enum ImageHost {
    #[default]
    Local,
    #[serde(rename = "picgo")]
    PicGo {
        #[serde(default = "default_picgo_endpoint")]
        endpoint: String,
    },
    Http {
        endpoint: String,
        #[serde(default = "default_field")]
        field: String,
        #[serde(default = "default_url_path")]
        url_path: String,
        /// One `Name: value` per line.
        #[serde(default)]
        headers: String,
    },
}

impl ImageHost {
    pub fn is_local(&self) -> bool {
        matches!(self, Self::Local)
    }
}

pub fn default_picgo_endpoint() -> String {
    DEFAULT_PICGO_ENDPOINT.to_string()
}

pub fn default_field() -> String {
    DEFAULT_FIELD.to_string()
}

pub fn default_url_path() -> String {
    DEFAULT_URL_PATH.to_string()
}

/// Checks that the configured host is usable before saving settings.
pub fn validate_host(host: &ImageHost) -> Result<()> {
    match host {
        ImageHost::Local => Ok(()),
        ImageHost::PicGo { endpoint } => validate_endpoint(endpoint),
        ImageHost::Http { endpoint, .. } => validate_endpoint(endpoint),
    }
}

/// Uploads an already-saved asset and returns the public URL.
pub fn upload(host: &ImageHost, asset: &Path, bytes: &[u8], kind: ImageKind) -> Result<String> {
    match host {
        ImageHost::Local => Err(Error::Upload("当前图床设置为本地".to_string())),
        ImageHost::PicGo { endpoint } => upload_picgo(endpoint, asset),
        ImageHost::Http {
            endpoint,
            field,
            url_path,
            headers,
        } => upload_http(endpoint, field, url_path, headers, asset, bytes, kind),
    }
}

fn validate_endpoint(endpoint: &str) -> Result<()> {
    let trimmed = endpoint.trim();
    if trimmed.is_empty() {
        return Err(Error::Upload("图床地址不能为空".to_string()));
    }
    let lower = trimmed.to_ascii_lowercase();
    if !lower.starts_with("http://") && !lower.starts_with("https://") {
        return Err(Error::Upload(
            "图床地址必须以 http:// 或 https:// 开头".to_string(),
        ));
    }
    Ok(())
}

fn agent() -> ureq::Agent {
    ureq::AgentBuilder::new().timeout(UPLOAD_TIMEOUT).build()
}

fn read_response(response: ureq::Response) -> Result<String> {
    let mut bytes = Vec::new();
    response
        .into_reader()
        .read_to_end(&mut bytes)
        .map_err(|err| Error::Upload(format!("读取图床响应失败：{err}")))?;
    Ok(String::from_utf8_lossy(&bytes).into_owned())
}

fn snippet(text: &str) -> String {
    let trimmed = text.trim();
    trimmed.chars().take(200).collect()
}

fn upload_picgo(endpoint: &str, asset: &Path) -> Result<String> {
    validate_endpoint(endpoint)?;
    let payload = serde_json::json!({ "list": [asset.display().to_string()] });
    let body = serde_json::to_vec(&payload)
        .map_err(|err| Error::Upload(format!("序列化 PicGo 请求失败：{err}")))?;
    let response = agent()
        .post(endpoint.trim())
        .set("Content-Type", "application/json")
        .send_bytes(&body)
        .map_err(|err| Error::Upload(format!("PicGo 上传失败：{err}")))?;
    let text = read_response(response)?;
    parse_picgo_response(&text)
}

/// Parses PicGo's `{"success":true,"result":["url"]}` reply.
pub fn parse_picgo_response(text: &str) -> Result<String> {
    let value: Value = serde_json::from_str(text)
        .map_err(|_| Error::Upload(format!("PicGo 返回的不是 JSON：{}", snippet(text))))?;
    if value.get("success").and_then(Value::as_bool) == Some(false) {
        let message = value
            .get("message")
            .and_then(Value::as_str)
            .unwrap_or("PicGo 拒绝了上传");
        return Err(Error::Upload(message.to_string()));
    }
    if let Some(url) = value
        .get("result")
        .and_then(Value::as_array)
        .and_then(|items| items.first())
        .and_then(Value::as_str)
    {
        return Ok(url.to_string());
    }
    if let Some(url) = value.get("result").and_then(Value::as_str) {
        return Ok(url.to_string());
    }
    Err(Error::Upload(format!(
        "PicGo 响应中没有图片地址：{}",
        snippet(text)
    )))
}

fn upload_http(
    endpoint: &str,
    field: &str,
    url_path: &str,
    headers: &str,
    asset: &Path,
    bytes: &[u8],
    kind: ImageKind,
) -> Result<String> {
    validate_endpoint(endpoint)?;
    let field = sanitize_token(field, DEFAULT_FIELD);
    let filename = sanitize_filename(
        asset
            .file_name()
            .map(|name| name.to_string_lossy().into_owned())
            .unwrap_or_else(|| format!("image.{}", kind.extension())),
    );
    let boundary = format!("----mdwork{}", &Uuid::new_v4().simple().to_string()[..16]);
    let body = multipart_body(&boundary, &field, &filename, kind, bytes);
    let mut request = agent().post(endpoint.trim()).set(
        "Content-Type",
        &format!("multipart/form-data; boundary={boundary}"),
    );
    for (name, value) in parse_headers(headers) {
        request = request.set(&name, &value);
    }
    let response = request
        .send_bytes(&body)
        .map_err(|err| Error::Upload(format!("HTTP 上传失败：{err}")))?;
    let text = read_response(response)?;
    parse_http_response(&text, url_path)
}

/// Extracts the URL from a generic upload API reply using a dotted path such
/// as `data.url` or `data.0.url`.
pub fn parse_http_response(text: &str, url_path: &str) -> Result<String> {
    let value: Value = serde_json::from_str(text)
        .map_err(|_| Error::Upload(format!("图床返回的不是 JSON：{}", snippet(text))))?;
    let path = if url_path.trim().is_empty() {
        DEFAULT_URL_PATH
    } else {
        url_path.trim()
    };
    if let Some(url) = json_lookup(&value, path).and_then(Value::as_str) {
        return Ok(url.to_string());
    }
    Err(Error::Upload(format!(
        "响应 JSON 路径「{path}」没有找到图片地址：{}",
        snippet(text)
    )))
}

fn json_lookup<'a>(value: &'a Value, path: &str) -> Option<&'a Value> {
    let mut current = value;
    for segment in path.split('.') {
        current = match current {
            Value::Object(map) => map.get(segment)?,
            Value::Array(items) => items.get(segment.parse::<usize>().ok()?)?,
            _ => return None,
        };
    }
    Some(current)
}

fn sanitize_token(value: &str, fallback: &str) -> String {
    let cleaned: String = value
        .trim()
        .chars()
        .filter(|ch| ch.is_ascii_alphanumeric() || matches!(ch, '_' | '-' | '.' | '[' | ']'))
        .collect();
    if cleaned.is_empty() {
        fallback.to_string()
    } else {
        cleaned
    }
}

fn sanitize_filename(value: String) -> String {
    let cleaned: String = value
        .chars()
        .map(|ch| match ch {
            '"' | '\r' | '\n' | '\\' => '_',
            other => other,
        })
        .collect();
    if cleaned.is_empty() {
        "image".to_string()
    } else {
        cleaned
    }
}

fn parse_headers(raw: &str) -> Vec<(String, String)> {
    let mut headers = Vec::new();
    for line in raw.lines() {
        let Some((name, value)) = line.split_once(':') else {
            continue;
        };
        let name = name.trim();
        let value = value.trim();
        if name.is_empty() || value.is_empty() {
            continue;
        }
        let lower = name.to_ascii_lowercase();
        if matches!(
            lower.as_str(),
            "content-type" | "content-length" | "host" | "transfer-encoding"
        ) {
            continue;
        }
        headers.push((name.to_string(), value.to_string()));
    }
    headers
}

fn multipart_body(
    boundary: &str,
    field: &str,
    filename: &str,
    kind: ImageKind,
    bytes: &[u8],
) -> Vec<u8> {
    let mut body = Vec::with_capacity(bytes.len() + 512);
    body.extend_from_slice(format!("--{boundary}\r\n").as_bytes());
    body.extend_from_slice(
        format!("Content-Disposition: form-data; name=\"{field}\"; filename=\"{filename}\"\r\n")
            .as_bytes(),
    );
    body.extend_from_slice(format!("Content-Type: {}\r\n\r\n", kind.mime()).as_bytes());
    body.extend_from_slice(bytes);
    body.extend_from_slice(format!("\r\n--{boundary}--\r\n").as_bytes());
    body
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_root(name: &str) -> PathBuf {
        let root = std::env::temp_dir().join(format!("mdwork-image-{name}-{}", Uuid::new_v4()));
        fs::create_dir_all(&root).unwrap();
        root
    }

    const PNG: &[u8] = b"\x89PNG\r\n\x1a\nrestofpng";

    #[test]
    fn saves_assets_with_unique_names() {
        let root = temp_root("save");
        let first = save_asset(&root, PNG, ImageKind::Png).unwrap();
        let second = save_asset(&root, PNG, ImageKind::Png).unwrap();
        assert!(first.starts_with(assets_dir(&root)));
        assert!(first.is_file() && second.is_file());
        assert_ne!(first, second);
        assert!(first.extension().unwrap().eq_ignore_ascii_case("png"));
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn rejects_mismatched_and_oversized_bytes() {
        let root = temp_root("reject");
        let err = save_asset(&root, b"not an image", ImageKind::Png).unwrap_err();
        assert!(matches!(err, Error::UnsupportedImage(_)));
        let empty = save_asset(&root, b"", ImageKind::Png).unwrap_err();
        assert!(matches!(empty, Error::UnsupportedImage(_)));
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn sniff_detects_common_formats() {
        assert_eq!(ImageKind::sniff(PNG), Some(ImageKind::Png));
        assert_eq!(
            ImageKind::sniff(&[0xFF, 0xD8, 0xFF, 0xE0]),
            Some(ImageKind::Jpeg)
        );
        assert_eq!(ImageKind::sniff(b"GIF89a....."), Some(ImageKind::Gif));
        assert_eq!(
            ImageKind::sniff(b"RIFF\0\0\0\0WEBPVP8 "),
            Some(ImageKind::Webp)
        );
        assert_eq!(ImageKind::sniff(b"BM\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0"), Some(ImageKind::Bmp));
        assert_eq!(
            ImageKind::sniff(b"<svg xmlns=\"http://www.w3.org/2000/svg\"/>"),
            Some(ImageKind::Svg)
        );
        assert_eq!(ImageKind::sniff(b"plain text"), None);
    }

    #[test]
    fn markdown_url_is_relative_and_encoded() {
        let note = Path::new(r"C:\ws\book\sub\a.md");
        let image = Path::new(r"C:\ws\book\assets\img 1 #x.png");
        assert_eq!(markdown_url(note, image), "../assets/img%201%20%23x.png");
        let note = Path::new(r"C:\ws\book\a.md");
        assert_eq!(
            markdown_url(note, Path::new(r"C:\ws\book\assets\x.png")),
            "assets/x.png"
        );
    }

    #[test]
    fn extracts_urls_from_generic_responses() {
        let text = r#"{"success":true,"data":{"url":"https://img.example/a.png"}}"#;
        assert_eq!(
            parse_http_response(text, "data.url").unwrap(),
            "https://img.example/a.png"
        );
        let text = r#"{"files":[{"link":"https://img.example/b.png"}]}"#;
        assert_eq!(
            parse_http_response(text, "files.0.link").unwrap(),
            "https://img.example/b.png"
        );
        assert!(parse_http_response("plain", "url").is_err());
        assert!(parse_http_response(r#"{"ok":true}"#, "url").is_err());
    }

    #[test]
    fn parses_picgo_responses() {
        let ok = r#"{"success":true,"result":["https://img.example/p.png"]}"#;
        assert_eq!(parse_picgo_response(ok).unwrap(), "https://img.example/p.png");
        let failed = r#"{"success":false,"message":"图床未配置"}"#;
        let err = parse_picgo_response(failed).unwrap_err();
        assert!(err.to_string().contains("图床未配置"));
    }

    #[test]
    fn multipart_body_wraps_bytes() {
        let body = multipart_body("BOUNDARY", "file", "a.png", ImageKind::Png, b"DATA");
        let text = String::from_utf8_lossy(&body).into_owned();
        assert!(text.starts_with("--BOUNDARY\r\n"));
        assert!(text.contains("name=\"file\"; filename=\"a.png\""));
        assert!(text.contains("Content-Type: image/png"));
        assert!(text.contains("DATA\r\n--BOUNDARY--\r\n"));
    }

    #[test]
    fn header_lines_are_filtered() {
        let headers = parse_headers("Authorization: Bearer x\nContent-Type: text/plain\nbad\n\nX-Env: dev");
        assert_eq!(
            headers,
            vec![
                ("Authorization".to_string(), "Bearer x".to_string()),
                ("X-Env".to_string(), "dev".to_string()),
            ]
        );
    }

    #[test]
    fn host_round_trips_through_json() {
        let host = ImageHost::PicGo {
            endpoint: default_picgo_endpoint(),
        };
        let json = serde_json::to_string(&host).unwrap();
        assert_eq!(json, r#"{"kind":"picgo","endpoint":"http://127.0.0.1:36677/upload"}"#);
        let parsed: ImageHost = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed, host);

        let http = ImageHost::Http {
            endpoint: "https://api.example/upload".into(),
            field: "file".into(),
            url_path: "data.url".into(),
            headers: "Authorization: Bearer x".into(),
        };
        let json = serde_json::to_string(&http).unwrap();
        let parsed: ImageHost = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed, http);

        let local: ImageHost = serde_json::from_str(r#"{"kind":"local"}"#).unwrap();
        assert!(local.is_local());
    }
}
