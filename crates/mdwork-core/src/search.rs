//! Content search across a notebook, with cancellation and result caps.
//!
//! The scan is deliberately on demand: notebooks stay plain folders and no
//! index has to be maintained or invalidated.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Mutex;

use regex::{Regex, RegexBuilder};

use crate::doc::decode;
use crate::notebook::{Entry, EntryKind, Notebook};
use crate::Result;

pub const DEFAULT_MAX_HITS: usize = 500;
const MAX_LINE_CHARS: usize = 240;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SearchQuery {
    pub text: String,
    pub case_sensitive: bool,
    pub regex: bool,
}

impl SearchQuery {
    pub fn new(text: impl Into<String>) -> Self {
        Self {
            text: text.into(),
            case_sensitive: false,
            regex: false,
        }
    }

    pub fn is_blank(&self) -> bool {
        self.text.trim().is_empty()
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SearchHit {
    pub notebook_id: uuid::Uuid,
    pub notebook_name: String,
    pub rel_path: String,
    /// One-based line number.
    pub line: usize,
    /// One-based column of the first match.
    pub column: usize,
    pub text: String,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct SearchOutcome {
    pub hits: Vec<SearchHit>,
    pub files: usize,
    pub cancelled: bool,
    pub truncated: bool,
}

pub fn search_notebook(
    notebook: &Notebook,
    query: &SearchQuery,
    cancel: &AtomicBool,
    max_hits: usize,
) -> Result<SearchOutcome> {
    search_notebook_filtered(notebook, query, cancel, max_hits, |_| true)
}

/// Like [`search_notebook`], but only scans notes accepted by `allow`, so
/// callers enforcing an ACL never read disallowed files.
pub fn search_notebook_filtered<F>(
    notebook: &Notebook,
    query: &SearchQuery,
    cancel: &AtomicBool,
    max_hits: usize,
    allow: F,
) -> Result<SearchOutcome>
where
    F: Fn(&str) -> bool + Sync,
{
    let mut outcome = SearchOutcome::default();
    if query.is_blank() {
        return Ok(outcome);
    }

    let entries: Vec<Entry> = notebook
        .list_entries()?
        .into_iter()
        .filter(|entry| entry.kind == EntryKind::Note && allow(&entry.rel_path))
        .collect();

    let matcher = Matcher::new(query)?;
    let hits = Mutex::new(Vec::new());
    let files = Mutex::new(0usize);
    let stop = AtomicBool::new(false);

    let workers = std::thread::available_parallelism()
        .map(|value| value.get())
        .unwrap_or(2)
        .clamp(1, 8);
    let chunk = entries.len().div_ceil(workers).max(1);

    std::thread::scope(|scope| {
        let matcher = &matcher;
        for slice in entries.chunks(chunk) {
            let hits = &hits;
            let files = &files;
            let stop = &stop;
            scope.spawn(move || {
                for entry in slice {
                    if cancel.load(Ordering::Relaxed) || stop.load(Ordering::Relaxed) {
                        return;
                    }
                    let Ok(path) = notebook.note_path(&entry.rel_path) else {
                        continue;
                    };
                    let Ok(bytes) = std::fs::read(&path) else {
                        continue;
                    };
                    *files.lock().unwrap() += 1;
                    let text = decode(&bytes).text;
                    let mut local = Vec::new();
                    for (index, line) in text.lines().enumerate() {
                        if cancel.load(Ordering::Relaxed) || stop.load(Ordering::Relaxed) {
                            return;
                        }
                        if let Some(column) = matcher.position(line) {
                            local.push(SearchHit {
                                notebook_id: notebook.marker.id,
                                notebook_name: notebook.display_name().to_string(),
                                rel_path: entry.rel_path.clone(),
                                line: index + 1,
                                column,
                                text: truncate_line(line),
                            });
                            if local.len() >= max_hits {
                                break;
                            }
                        }
                    }
                    if !local.is_empty() {
                        let mut shared = hits.lock().unwrap();
                        shared.extend(local);
                        if shared.len() >= max_hits {
                            stop.store(true, Ordering::Relaxed);
                        }
                    }
                }
            });
        }
    });

    let mut hits = hits.into_inner().unwrap();
    hits.truncate(max_hits);
    outcome.truncated = stop.load(Ordering::Relaxed);
    outcome.cancelled = cancel.load(Ordering::Relaxed) && !outcome.truncated;
    outcome.hits = hits;
    outcome.files = files.into_inner().unwrap();
    Ok(outcome)
}

enum Matcher {
    Plain { needle: String, case_sensitive: bool },
    Regex(Regex),
}

impl Matcher {
    fn new(query: &SearchQuery) -> Result<Self> {
        if query.regex {
            let regex = RegexBuilder::new(&query.text)
                .case_insensitive(!query.case_sensitive)
                .build()
                .map_err(|err| crate::Error::InvalidPath(format!("正则表达式无效：{err}")))?;
            return Ok(Self::Regex(regex));
        }
        let needle = if query.case_sensitive {
            query.text.clone()
        } else {
            query.text.to_lowercase()
        };
        Ok(Self::Plain {
            needle,
            case_sensitive: query.case_sensitive,
        })
    }

    /// Returns the one-based column of the first match in `line`.
    fn position(&self, line: &str) -> Option<usize> {
        match self {
            Self::Regex(regex) => regex.find(line).map(|found| char_column(line, found.start())),
            Self::Plain {
                needle,
                case_sensitive,
            } => {
                if *case_sensitive {
                    line.find(needle.as_str())
                        .map(|byte| char_column(line, byte))
                } else {
                    line.to_lowercase()
                        .find(needle.as_str())
                        .map(|byte| char_column(line, byte))
                }
            }
        }
    }
}

fn char_column(line: &str, byte: usize) -> usize {
    line.get(..byte).map(|head| head.chars().count() + 1).unwrap_or(1)
}

fn truncate_line(line: &str) -> String {
    let mut text = line.trim().to_string();
    if text.chars().count() > MAX_LINE_CHARS {
        text = text.chars().take(MAX_LINE_CHARS).collect::<String>() + "…";
    }
    text
}

#[cfg(test)]
mod tests {
    use super::*;
    use uuid::Uuid;

    fn temp_notebook() -> Notebook {
        let root = std::env::temp_dir().join(format!("mdwork-search-{}", Uuid::new_v4()));
        Notebook::create(&root, "搜索").unwrap()
    }

    #[test]
    fn finds_lines_and_columns() {
        let notebook = temp_notebook();
        notebook
            .create_note("一.md", "第一行\nTODO 第二行\n最后\n")
            .unwrap();
        notebook.create_note("二.md", "没有关键词\n").unwrap();

        let cancel = AtomicBool::new(false);
        let outcome =
            search_notebook(&notebook, &SearchQuery::new("todo"), &cancel, DEFAULT_MAX_HITS)
                .unwrap();
        assert_eq!(outcome.files, 2);
        assert_eq!(outcome.hits.len(), 1);
        let hit = &outcome.hits[0];
        assert_eq!(hit.rel_path, "一.md");
        assert_eq!(hit.line, 2);
        assert_eq!(hit.column, 1);
        std::fs::remove_dir_all(&notebook.root).unwrap();
    }

    #[test]
    fn regex_and_case_sensitivity() {
        let notebook = temp_notebook();
        notebook.create_note("a.md", "Alpha\nbeta\n").unwrap();
        let cancel = AtomicBool::new(false);

        let mut query = SearchQuery::new("a.*a");
        query.regex = true;
        let outcome = search_notebook(&notebook, &query, &cancel, DEFAULT_MAX_HITS).unwrap();
        assert_eq!(outcome.hits.len(), 1);

        query.regex = false;
        query.case_sensitive = true;
        let outcome = search_notebook(&notebook, &query, &cancel, DEFAULT_MAX_HITS).unwrap();
        assert!(outcome.hits.is_empty());
        std::fs::remove_dir_all(&notebook.root).unwrap();
    }

    #[test]
    fn cancellation_stops_scan() {
        let notebook = temp_notebook();
        for index in 0..20 {
            notebook
                .create_note(&format!("{index}.md"), "命中\n")
                .unwrap();
        }
        let cancel = AtomicBool::new(true);
        let outcome = search_notebook(&notebook, &SearchQuery::new("命中"), &cancel, 5).unwrap();
        assert!(outcome.hits.len() <= 5);
        assert!(outcome.cancelled || outcome.truncated);
        std::fs::remove_dir_all(&notebook.root).unwrap();
    }
}
