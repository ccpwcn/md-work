use std::path::{Path, PathBuf};
use std::sync::mpsc::{Receiver, channel};
use std::time::{Duration, Instant};

use notify::{RecursiveMode, Watcher};

const DEBOUNCE: Duration = Duration::from_millis(300);

/// Keeps a recursive watch alive; dropping it stops the watch.
pub struct WorkspaceWatcher {
    _inner: notify::RecommendedWatcher,
}

impl WorkspaceWatcher {
    /// Starts watching `root`; the receiver yields debounced batches of changed paths.
    pub fn start(root: &Path) -> crate::Result<(Self, Receiver<Vec<PathBuf>>)> {
        let (raw_tx, raw_rx) = channel::<Vec<PathBuf>>();
        let mut watcher =
            notify::recommended_watcher(move |event: notify::Result<notify::Event>| {
                let Ok(event) = event else {
                    return;
                };
                if event.kind.is_access() {
                    return;
                }
                let _ = raw_tx.send(event.paths);
            })?;
        watcher.watch(root, RecursiveMode::Recursive)?;

        let (tx, rx) = channel::<Vec<PathBuf>>();
        std::thread::spawn(move || {
            while let Ok(mut batch) = raw_rx.recv() {
                let deadline = Instant::now() + DEBOUNCE;
                loop {
                    let now = Instant::now();
                    if now >= deadline {
                        break;
                    }
                    match raw_rx.recv_timeout(deadline - now) {
                        Ok(more) => batch.extend(more),
                        Err(_) => break,
                    }
                }
                if tx.send(batch).is_err() {
                    break;
                }
            }
        });

        Ok((Self { _inner: watcher }, rx))
    }
}
