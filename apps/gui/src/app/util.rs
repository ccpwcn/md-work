use super::*;
use gpui_kit::base::{Scrollbar, ScrollbarMode};

/// `uniform_list` 自带滚轮滚动，但不会绘制滚动条。侧边栏面板统一通过
/// 这个扩展方法包一层滚动区域，让列表与滚动条共享同一个滚动句柄。
pub(super) trait PanelScrollbar: Sized {
    fn panel_scrollbar(self, handle: &UniformListScrollHandle) -> AnyElement;
}

impl PanelScrollbar for UniformList {
    fn panel_scrollbar(self, handle: &UniformListScrollHandle) -> AnyElement {
        div()
            .flex_1()
            .min_h_0()
            .v_flex()
            .relative()
            .child(self.flex_1().min_h_0().track_scroll(handle))
            .child(
                Scrollbar::vertical(handle)
                    .viewport_from_layout()
                    .mode(ScrollbarMode::Always),
            )
            .into_any_element()
    }
}

/// Subsequence match with a bonus for word starts and consecutive runs.
pub(super) fn fuzzy_score(label: &str, query: &str) -> Option<i64> {
    if query.trim().is_empty() {
        return Some(0);
    }
    let mut score = 0i64;
    let mut pos = 0usize;
    let mut previous: Option<usize> = None;
    for ch in query.chars().filter(|ch| !ch.is_whitespace()) {
        let found = label[pos..].find(ch)?;
        let index = pos + found;
        let boundary = index == 0
            || matches!(
                label.as_bytes().get(index.wrapping_sub(1)),
                Some(b' ' | b'.' | b'-' | b'/' | b':' | b'_')
            );
        score += if boundary { 8 } else { 1 };
        if previous.is_some_and(|last| index == last + 1) {
            score += 4;
        }
        previous = Some(index);
        pos = index + ch.len_utf8();
    }
    Some(score)
}

pub(super) fn parse_font_size(raw: &str, fallback: f32) -> f32 {
    raw.trim()
        .parse::<f32>()
        .ok()
        .filter(|size| size.is_finite())
        .map(|size| size.clamp(10.0, 32.0))
        .unwrap_or(fallback.clamp(10.0, 32.0))
}

pub(super) fn format_pt(value: f32) -> String {
    if (value - value.round()).abs() < f32::EPSILON {
        format!("{}", value.round() as i32)
    } else {
        format!("{value:.1}")
    }
}

pub(super) fn format_age(seconds: u64) -> String {
    if seconds < 60 {
        t!("recovery.age_now").to_string()
    } else if seconds < 3600 {
        t!("recovery.age_minutes", count = seconds / 60).to_string()
    } else if seconds < 86_400 {
        t!("recovery.age_hours", count = seconds / 3600).to_string()
    } else {
        t!("recovery.age_days", count = seconds / 86_400).to_string()
    }
}

pub(super) fn mcp_field(label: String, control: AnyElement, muted: gpui_kit::Hsla) -> AnyElement {
    div()
        .v_flex()
        .gap_1()
        .child(div().text_xs().text_color(muted).child(label))
        .child(control)
        .into_any_element()
}

pub(super) fn empty_hint(text: String, color: gpui_kit::Hsla) -> AnyElement {
    div()
        .flex_1()
        .p_3()
        .text_sm()
        .text_color(color)
        .child(text)
        .into_any_element()
}

pub(super) fn notice_icon(kind: NoticeKind) -> IconName {
    match kind {
        NoticeKind::Info => IconName::Info,
        NoticeKind::Success => IconName::CircleCheck,
        NoticeKind::Warning => IconName::TriangleAlert,
        NoticeKind::Error => IconName::CircleX,
    }
}

pub(super) fn notice_color(kind: NoticeKind, cx: &App) -> gpui_kit::Hsla {
    match kind {
        NoticeKind::Info => cx.theme().info,
        NoticeKind::Success => cx.theme().success,
        NoticeKind::Warning => cx.theme().warning,
        NoticeKind::Error => cx.theme().danger,
    }
}

pub(super) fn format_notice_age(created: Instant) -> String {
    let seconds = created.elapsed().as_secs();
    if seconds < 60 {
        t!("notice.just_now").to_string()
    } else if seconds < 3_600 {
        t!("notice.minutes_ago", count = seconds / 60).to_string()
    } else if seconds < 86_400 {
        t!("notice.hours_ago", count = seconds / 3_600).to_string()
    } else {
        t!("notice.days_ago", count = seconds / 86_400).to_string()
    }
}

pub(super) fn title_icon(id: &'static str, icon: IconName, tooltip: String) -> Button {
    Button::new(id)
        .ghost()
        .compact()
        .icon(icon)
        .tooltip(tooltip)
}

pub(super) fn write_math_cache(key: u64, svg: &str) -> Option<PathBuf> {
    let dir = mdwork_core::paths::cache_dir().join("math");
    if std::fs::create_dir_all(&dir).is_err() {
        return None;
    }
    let path = dir.join(format!("{key:016x}.svg"));
    mdwork_core::atomic::write_atomic(&path, svg.as_bytes())
        .ok()
        .map(|_| path)
}

pub(super) fn seed_workspace(workspace: &Workspace) -> mdwork_core::Result<()> {
    let notebook = workspace.create_notebook("我的笔记本")?;
    notebook.create_note("欢迎使用墨笺.md", WELCOME_NOTE)?;
    Ok(())
}

/// Loads the session belonging to `root`. Before workspaces became first-class
/// there was a single global `session.json`; it is migrated once into the
/// current workspace and then renamed so other workspaces do not inherit it.
pub(super) fn load_workspace_session(root: &Path, allow_legacy: bool) -> Session {
    let path = mdwork_core::paths::workspace_session_path(root);
    if path.is_file() {
        return match Session::load_from(&path) {
            Ok(session) => session,
            Err(err) => {
                tracing::warn!(path = %path.display(), error = %err, "读取会话失败，使用默认会话");
                Session::default()
            }
        };
    }
    if allow_legacy {
        let legacy = mdwork_core::paths::session_path();
        if legacy.is_file() {
            match Session::load_from(&legacy) {
                Ok(session) => {
                    if let Err(err) = session.save_to(&path) {
                        tracing::warn!(error = %err, "迁移会话失败");
                    }
                    let archived = legacy.with_file_name("session.migrated.json");
                    if let Err(err) = std::fs::rename(&legacy, &archived) {
                        tracing::warn!(error = %err, "归档旧会话失败");
                    } else {
                        tracing::info!(path = %archived.display(), "已迁移旧版会话");
                    }
                    return session;
                }
                Err(err) => tracing::warn!(error = %err, "读取旧会话失败"),
            }
        }
    }
    Session::default()
}

/// Titlebar controls sit inside the OS drag region, so a click that the app
/// does not consume reaches `DefWindowProc` and a double click maximizes the
/// window. Swallow mouse-down for every interactive titlebar control.
pub(super) fn titlebar_guard(element: impl IntoElement) -> AnyElement {
    div()
        .on_mouse_down(MouseButton::Left, |_, window, cx| {
            window.prevent_default();
            cx.stop_propagation();
        })
        .child(element)
        .into_any_element()
}

pub(super) fn open_path_with_default_app_sync(path: &Path) -> std::io::Result<()> {
    if !path.exists() {
        return Err(std::io::Error::new(
            std::io::ErrorKind::NotFound,
            path.display().to_string(),
        ));
    }
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        const CREATE_NO_WINDOW: u32 = 0x0800_0000;
        // `cmd /C start "" <path>` opens with the shell association; the empty
        // title argument is required when the path may contain spaces.
        std::process::Command::new("cmd")
            .args(["/C", "start", "", &path.display().to_string()])
            .creation_flags(CREATE_NO_WINDOW)
            .spawn()
            .map(|_| ())
    }
    #[cfg(target_os = "macos")]
    {
        std::process::Command::new("open")
            .arg(path)
            .spawn()
            .map(|_| ())
    }
    #[cfg(all(unix, not(target_os = "macos")))]
    {
        std::process::Command::new("xdg-open")
            .arg(path)
            .spawn()
            .map(|_| ())
    }
}

pub(super) fn ensure_export_extension(path: PathBuf, format: ExportFormat) -> PathBuf {
    let extension = format.extension();
    if path
        .extension()
        .and_then(|extension| extension.to_str())
        .is_some_and(|current| current.eq_ignore_ascii_case(extension))
    {
        path
    } else {
        path.with_extension(extension)
    }
}

pub(super) fn ensure_zip_extension(path: PathBuf) -> PathBuf {
    if path
        .extension()
        .and_then(|extension| extension.to_str())
        .is_some_and(|current| current.eq_ignore_ascii_case("zip"))
    {
        path
    } else {
        path.with_extension("zip")
    }
}

pub(super) fn unique_zip_path(dir: &Path, stem: &str) -> PathBuf {
    let mut candidate = dir.join(format!("{stem}.zip"));
    let mut counter = 2;
    while candidate.exists() {
        candidate = dir.join(format!("{stem} ({counter}).zip"));
        counter += 1;
    }
    candidate
}

pub(super) fn subscribe_zip_path(cx: &mut Context<AppShell>, input: &Entity<InputState>) {
    cx.subscribe(input, |this, _, event: &InputEvent, cx| {
        if matches!(event, InputEvent::Change) {
            this.schedule_zip_plan(cx);
        }
    })
    .detach();
}

pub(super) fn format_bytes(bytes: u64) -> String {
    const KB: f64 = 1024.0;
    const MB: f64 = KB * 1024.0;
    const GB: f64 = MB * 1024.0;
    let value = bytes as f64;
    if value >= GB {
        format!("{:.2} GB", value / GB)
    } else if value >= MB {
        format!("{:.1} MB", value / MB)
    } else if value >= KB {
        format!("{:.1} KB", value / KB)
    } else {
        format!("{bytes} B")
    }
}

/// Picks the image payload out of a paste event: bitmap data wins over files,
/// and non-image files fall through to the default text paste.
pub(super) fn clipboard_images(item: &ClipboardItem) -> Vec<PastedImage> {
    let mut bitmaps = Vec::new();
    let mut files = Vec::new();
    for entry in &item.entries {
        match entry {
            ClipboardEntry::Image(image) => {
                let kind = match image.format {
                    ImageFormat::Png => Some(ImageKind::Png),
                    ImageFormat::Jpeg => Some(ImageKind::Jpeg),
                    ImageFormat::Gif => Some(ImageKind::Gif),
                    ImageFormat::Webp => Some(ImageKind::Webp),
                    ImageFormat::Bmp => Some(ImageKind::Bmp),
                    ImageFormat::Svg => Some(ImageKind::Svg),
                    _ => ImageKind::sniff(&image.bytes),
                };
                if let Some(kind) = kind
                    && image.bytes.len() as u64 <= image::IMAGE_MAX_BYTES
                {
                    bitmaps.push(PastedImage::Bytes {
                        bytes: image.bytes.clone(),
                        kind,
                    });
                }
            }
            ClipboardEntry::ExternalPaths(paths) => {
                for path in paths.paths() {
                    if ImageKind::from_extension(path).is_some() {
                        files.push(PastedImage::File(path.clone()));
                    }
                }
            }
            ClipboardEntry::String(_) => {}
        }
    }
    if bitmaps.is_empty() { files } else { bitmaps }
}

pub(super) fn store_images(
    notebook_root: &Path,
    note_path: &Path,
    host: &ImageHost,
    images: Vec<PastedImage>,
) -> StoredImages {
    let mut links = Vec::new();
    let mut failed_uploads = Vec::new();
    let mut error = None;
    for image in images {
        let (bytes, kind) = match image {
            PastedImage::Bytes { bytes, kind } => (bytes, kind),
            PastedImage::File(path) => {
                let Some(kind) = ImageKind::from_extension(&path) else {
                    continue;
                };
                match std::fs::read(&path) {
                    Ok(bytes) => (bytes, kind),
                    Err(err) => {
                        error = Some(format!("{}: {err}", path.display()));
                        continue;
                    }
                }
            }
        };
        let asset = match image::save_asset(notebook_root, &bytes, kind) {
            Ok(path) => path,
            Err(err) => {
                error = Some(err.to_string());
                continue;
            }
        };
        let mut url = image::markdown_url(note_path, &asset);
        if !host.is_local() {
            match image::upload(host, &asset, &bytes, kind) {
                Ok(remote) => {
                    let _ = std::fs::remove_file(&asset);
                    url = remote;
                }
                Err(err) => failed_uploads.push(err.to_string()),
            }
        }
        links.push(format!("![]({url})"));
    }
    // Blank lines keep adjacent figures as separate CommonMark paragraphs so
    // preview promotes them to block images instead of one soft-broken paragraph.
    StoredImages {
        markdown: links.join("\n\n"),
        failed_uploads,
        error,
    }
}

#[expect(
    clippy::too_many_arguments,
    reason = "图片插入任务需要一次性捕获文档、路径、图床与窗口上下文"
)]
pub(super) fn spawn_image_insert(
    shell: WeakEntity<AppShell>,
    doc: Entity<Doc>,
    notebook_root: PathBuf,
    note_path: PathBuf,
    host: ImageHost,
    images: Vec<PastedImage>,
    window: &mut Window,
    cx: &mut App,
) {
    let doc = doc.downgrade();
    let task = window.spawn(cx, async move |cx| {
        let stored = cx
            .background_executor()
            .spawn(async move { store_images(&notebook_root, &note_path, &host, images) })
            .await;
        let (notice, is_error) = if let Some(error) = &stored.error {
            (
                Some(t!("image.failed", message = error.clone()).to_string()),
                true,
            )
        } else {
            (
                stored
                    .failed_uploads
                    .first()
                    .map(|error| t!("image.upload_failed", message = error.clone()).to_string()),
                false,
            )
        };
        let markdown = stored.markdown;
        if !markdown.is_empty() {
            let _ = doc.update_in(cx, |doc, window, cx| doc.insert_text(&markdown, window, cx));
        }
        let _ = shell.update(cx, |this, cx| {
            if let Some(notice) = notice {
                if is_error {
                    this.notify_error(notice, cx);
                } else {
                    this.notify_warning(notice, cx);
                }
            }
            cx.notify();
        });
    });
    task.detach();
}

pub(super) fn stage_label(stage: &str) -> String {
    match stage {
        "read" => t!("export.stage.read").to_string(),
        "compile" => t!("export.stage.compile").to_string(),
        "write" => t!("export.stage.write").to_string(),
        "render" => t!("export.stage.render").to_string(),
        other => other.to_string(),
    }
}

pub(super) fn is_atomic_temp(path: &Path) -> bool {
    path.file_name()
        .and_then(|name| name.to_str())
        .is_some_and(|name| name.starts_with('.') && name.ends_with(".tmp"))
}

pub(super) fn paths_equal(a: &Path, b: &Path) -> bool {
    #[cfg(windows)]
    {
        a.as_os_str().to_string_lossy().to_lowercase()
            == b.as_os_str().to_string_lossy().to_lowercase()
    }
    #[cfg(not(windows))]
    {
        a == b
    }
}

pub(super) fn path_under(rel: &str, prefix: &str) -> bool {
    if rel.eq_ignore_ascii_case(prefix) {
        return true;
    }
    rel.len() > prefix.len()
        && rel.as_bytes()[prefix.len()] == b'/'
        && rel[..prefix.len()].eq_ignore_ascii_case(prefix)
}

pub(super) fn token_store() -> TokenStore {
    TokenStore::new(TokenStore::default_path())
}

pub(super) fn split_globs(raw: &str) -> Vec<String> {
    raw.split([',', ';', '\n', '\r'])
        .map(str::trim)
        .filter(|part| !part.is_empty())
        .map(str::to_string)
        .collect()
}

pub(super) fn load_audit_entries() -> (Vec<AuditEntry>, Option<String>) {
    match AuditLog::new(AuditLog::default_dir()).recent(100) {
        Ok(entries) => (entries, None),
        Err(err) => (
            Vec::new(),
            Some(t!("mcp.audit_error", message = err.to_string()).to_string()),
        ),
    }
}

pub(super) fn scope_label(scope: Scope) -> String {
    let key = match scope {
        Scope::Read => "mcp.scope.read",
        Scope::Write => "mcp.scope.write",
        Scope::Search => "mcp.scope.search",
        Scope::Delete => "mcp.scope.delete",
        Scope::DeletePermanent => "mcp.scope.delete_permanent",
        Scope::Export => "mcp.scope.export",
        Scope::Admin => "mcp.scope.admin",
    };
    t!(key).to_string()
}

pub(super) fn age_label(deleted_at: u64) -> String {
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|duration| duration.as_secs())
        .unwrap_or(deleted_at);
    let elapsed = now.saturating_sub(deleted_at);
    if elapsed < 60 {
        t!("trash.just_now").to_string()
    } else if elapsed < 3600 {
        t!("trash.minutes_ago", n = elapsed / 60).to_string()
    } else if elapsed < 86_400 {
        t!("trash.hours_ago", n = elapsed / 3600).to_string()
    } else {
        t!("trash.days_ago", n = elapsed / 86_400).to_string()
    }
}

/// Formats a unix timestamp as a local `YYYY-MM-DD HH:MM` label for tooltips.
pub(super) fn local_datetime_label(unix_secs: u64) -> String {
    use windows::Win32::Foundation::{FILETIME, SYSTEMTIME};
    use windows::Win32::System::Time::{FileTimeToSystemTime, SystemTimeToTzSpecificLocalTime};

    // Seconds between the Windows FILETIME epoch (1601-01-01) and the Unix epoch.
    let ticks = unix_secs
        .saturating_add(11_644_473_600)
        .saturating_mul(10_000_000);
    let filetime = FILETIME {
        dwLowDateTime: (ticks & 0xffff_ffff) as u32,
        dwHighDateTime: (ticks >> 32) as u32,
    };
    let mut utc = SYSTEMTIME::default();
    let mut local = SYSTEMTIME::default();
    // SAFETY: 两个 SYSTEMTIME 输出参数都是本函数栈上的有效可变引用。
    let converted = unsafe {
        FileTimeToSystemTime(&filetime, &mut utc)
            .and_then(|()| SystemTimeToTzSpecificLocalTime(None, &utc, &mut local))
            .is_ok()
    };
    if converted {
        format!(
            "{:04}-{:02}-{:02} {:02}:{:02}",
            local.wYear, local.wMonth, local.wDay, local.wHour, local.wMinute
        )
    } else {
        age_label(unix_secs)
    }
}

/// Label shown in the tree and in the in-place rename field.
///
/// Notes drop a trailing `.md`, matching `Notebook::list_tree`.
pub(super) fn entry_display_name(rel_path: &str, kind: EntryKind) -> String {
    let name = file_name(rel_path);
    match kind {
        EntryKind::Note => name.trim_end_matches(".md").to_string(),
        EntryKind::Folder => name.to_string(),
    }
}

/// File names of the direct children of `parent`, in tree order.
pub(super) fn sibling_file_names(tree: &[Entry], parent: &str, order: &TreeOrder) -> Vec<String> {
    let mut children: Vec<(String, bool)> = tree
        .iter()
        .filter(|item| parent_rel(&item.rel_path) == parent)
        .map(|item| {
            (
                file_name(&item.rel_path).to_string(),
                item.kind == EntryKind::Folder,
            )
        })
        .collect();
    let saved = order.names(parent).map(|names| names.to_vec());
    children.sort_by(|left, right| {
        sibling_rank(saved.as_deref(), &left.0)
            .cmp(&sibling_rank(saved.as_deref(), &right.0))
            .then_with(|| right.1.cmp(&left.1))
            .then_with(|| left.0.to_lowercase().cmp(&right.0.to_lowercase()))
    });
    children.into_iter().map(|(name, _)| name).collect()
}

/// Puts `child` immediately after `after_name`. `None` appends.
///
/// An existing entry with the same name (any ASCII case) is moved, not duplicated.
pub(super) fn insert_sibling_name(
    names: &mut Vec<String>,
    child: String,
    after_name: Option<&str>,
) {
    names.retain(|name| !name.eq_ignore_ascii_case(&child));
    let insert_at = match after_name {
        Some(anchor) => names
            .iter()
            .position(|name| name.eq_ignore_ascii_case(anchor))
            .map(|index| index + 1)
            .unwrap_or(names.len()),
        None => names.len(),
    };
    names.insert(insert_at, child);
}

/// Records where a just-created note or folder sits. `anchor` is the sibling
/// relative path to insert after; `None` appends.
pub(super) fn place_created(notebook: &Notebook, rel: &str, anchor: Option<&str>) {
    let after = anchor.map(file_name);
    if let Err(err) = notebook.place_child(parent_rel(rel), file_name(rel), after) {
        tracing::warn!(error = %err, path = %rel, "记录新建项的顺序失败");
    }
}

pub(super) fn load_notebook_entry(
    notebook: Notebook,
    expanded: HashSet<String>,
    root_expanded: bool,
) -> NotebookEntry {
    let order = notebook.load_tree_order();
    let tree = notebook.list_tree().unwrap_or_default();
    let rows = build_rows(&tree, &expanded, &order);
    NotebookEntry {
        notebook,
        tree,
        expanded,
        rows,
        order,
        root_expanded,
    }
}

pub(super) fn build_rows(
    tree: &[Entry],
    expanded: &HashSet<String>,
    order: &TreeOrder,
) -> Vec<TreeRow> {
    let mut by_parent: BTreeMap<&str, Vec<&Entry>> = BTreeMap::new();
    for item in tree {
        let parent = parent_rel(&item.rel_path);
        by_parent.entry(parent).or_default().push(item);
    }
    for (parent, items) in &mut by_parent {
        let saved = order.names(parent).map(|names| names.to_vec());
        items.sort_by(|left, right| {
            sibling_rank(saved.as_deref(), file_name(&left.rel_path))
                .cmp(&sibling_rank(saved.as_deref(), file_name(&right.rel_path)))
                .then_with(|| {
                    let left_folder = left.kind == EntryKind::Folder;
                    let right_folder = right.kind == EntryKind::Folder;
                    right_folder
                        .cmp(&left_folder)
                        .then_with(|| left.name.to_lowercase().cmp(&right.name.to_lowercase()))
                })
        });
    }

    let mut note_counts: HashMap<&str, usize> = HashMap::new();
    for item in tree.iter().filter(|item| item.kind == EntryKind::Note) {
        let mut ancestor = item.rel_path.as_str();
        while let Some(index) = ancestor.rfind('/') {
            ancestor = &ancestor[..index];
            *note_counts.entry(ancestor).or_insert(0) += 1;
        }
    }

    let mut rows = Vec::with_capacity(tree.len());
    push_rows("", 0, &by_parent, expanded, &note_counts, &mut rows);
    rows
}

pub(super) fn push_rows(
    parent: &str,
    depth: usize,
    by_parent: &BTreeMap<&str, Vec<&Entry>>,
    expanded: &HashSet<String>,
    note_counts: &HashMap<&str, usize>,
    rows: &mut Vec<TreeRow>,
) {
    let Some(items) = by_parent.get(parent) else {
        return;
    };
    for item in items {
        let is_folder = item.kind == EntryKind::Folder;
        let has_children = is_folder && by_parent.contains_key(item.rel_path.as_str());
        let is_open = has_children && expanded.contains(&item.rel_path);
        rows.push(TreeRow {
            rel_path: item.rel_path.clone(),
            name: item.name.clone(),
            kind: item.kind,
            depth,
            expanded: is_open,
            created: item.created,
            modified: item.modified,
            note_count: note_counts
                .get(item.rel_path.as_str())
                .copied()
                .unwrap_or(0),
        });
        if is_open {
            push_rows(
                &item.rel_path,
                depth + 1,
                by_parent,
                expanded,
                note_counts,
                rows,
            );
        }
    }
}

#[cfg(test)]
mod tests {
    use std::collections::HashSet;

    use super::{
        build_rows, entry_display_name, insert_sibling_name, local_datetime_label,
        sibling_file_names,
    };
    use crate::app::{Entry, EntryKind, TreeOrder};

    fn sample(rel_path: &str, name: &str, kind: EntryKind) -> Entry {
        Entry {
            rel_path: rel_path.to_string(),
            name: name.to_string(),
            kind,
            size: 0,
            created: 0,
            modified: 0,
        }
    }

    #[test]
    fn formats_unix_time_in_the_current_century() {
        // 1_700_000_000 is 2023-11-14 22:13 UTC; any local offset stays in 2023-11.
        let label = local_datetime_label(1_700_000_000);
        assert!(label.starts_with("2023-11-"), "unexpected label: {label}");
        assert_eq!(label.len(), 16, "unexpected label: {label}");
    }

    #[test]
    fn new_note_is_inserted_after_its_anchor() {
        let tree = vec![
            sample("甲.md", "甲", EntryKind::Note),
            sample("丙.md", "丙", EntryKind::Note),
        ];
        let mut order = TreeOrder::default();
        order.set_names("", vec!["甲.md".to_string(), "丙.md".to_string()]);
        let mut names = sibling_file_names(&tree, "", &order);
        insert_sibling_name(&mut names, "新笔记.md".to_string(), Some("甲.md"));
        assert_eq!(names, vec!["甲.md", "新笔记.md", "丙.md"]);
        order.set_names("", names);
        let mut tree = tree;
        tree.push(sample(
            "新笔记.md",
            &entry_display_name("新笔记.md", EntryKind::Note),
            EntryKind::Note,
        ));
        let rows = build_rows(&tree, &HashSet::new(), &order);
        let labels: Vec<_> = rows.iter().map(|row| row.name.as_str()).collect();
        assert_eq!(labels, vec!["甲", "新笔记", "丙"]);
    }

    #[test]
    fn child_created_under_a_folder_stays_one_level_down() {
        let tree = vec![
            sample("资料", "资料", EntryKind::Folder),
            sample("随笔.md", "随笔", EntryKind::Note),
            sample("资料/新笔记.md", "新笔记", EntryKind::Note),
        ];
        let mut order = TreeOrder::default();
        order.set_names("", vec!["资料".to_string(), "随笔.md".to_string()]);
        order.set_names("资料", vec!["新笔记.md".to_string()]);
        let mut expanded = HashSet::new();
        expanded.insert("资料".to_string());
        let rows = build_rows(&tree, &expanded, &order);
        let labels: Vec<_> = rows
            .iter()
            .map(|row| (row.name.as_str(), row.depth))
            .collect();
        assert_eq!(labels, vec![("资料", 0), ("新笔记", 1), ("随笔", 0)]);
    }

    #[test]
    fn new_folder_appends_when_there_is_no_anchor() {
        let tree = vec![sample("笔记.md", "笔记", EntryKind::Note)];
        let order = TreeOrder::default();
        let mut names = sibling_file_names(&tree, "", &order);
        insert_sibling_name(&mut names, "新文件夹".to_string(), None);
        assert_eq!(names, vec!["笔记.md", "新文件夹"]);
    }
}
