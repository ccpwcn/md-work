//! The key note operations must leave a trace in the log file so support can
//! reconstruct what a user did.

use mdwork_core::doc::{Eol, TextEncoding};
use mdwork_core::notebook::Notebook;

#[test]
fn key_note_operations_are_logged() {
    let base = std::env::temp_dir().join(format!("mdwork-core-log-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&base);
    let logs = base.join("logs");
    mdwork_log::init(mdwork_log::Options {
        dir: logs.clone(),
        console: false,
        retention: 2,
    })
    .expect("logging starts");

    let notebook = Notebook::create(&base.join("nb"), "日志测试").expect("notebook created");
    notebook
        .save_note("a.md", "hello", TextEncoding::Utf8, false, Eol::Lf)
        .expect("note saved");
    let item = notebook.delete_entry("a.md").expect("note deleted");
    notebook
        .restore_trash_item(&item.id)
        .expect("note restored");
    mdwork_core::AppConfig::default()
        .save_to(&base.join("config.json"))
        .expect("settings saved");

    let text = std::fs::read_dir(&logs)
        .expect("log dir exists")
        .flatten()
        .map(|entry| entry.path())
        .max()
        .map(|path| std::fs::read_to_string(path).expect("log file is readable"))
        .expect("a log file was written");
    for needle in [
        "创建笔记本",
        "保存笔记",
        "删除条目到回收站",
        "从回收站恢复条目",
        "保存设置",
    ] {
        assert!(text.contains(needle), "missing {needle} in log:\n{text}");
    }

    let _ = std::fs::remove_dir_all(&base);
}
