//! Single serialised background writer for GUI-originated disk writes.
//!
//! Every write triggered by the UI is queued here and executed on one worker
//! thread in submission order, so the event loop never blocks on the disk and
//! operations that depend on each other (save a note, then delete it) still
//! happen in the right order.

use std::sync::OnceLock;
use std::sync::mpsc::{self, Receiver, Sender};
use std::time::Duration;

type Job = Box<dyn FnOnce() + Send + 'static>;

static QUEUE: OnceLock<Sender<Job>> = OnceLock::new();

fn queue() -> &'static Sender<Job> {
    QUEUE.get_or_init(|| {
        let (tx, rx): (Sender<Job>, Receiver<Job>) = mpsc::channel();
        std::thread::Builder::new()
            .name("mdwork-disk".to_string())
            .spawn(move || run(rx))
            .expect("failed to spawn the disk writer thread");
        tx
    })
}

fn run(rx: Receiver<Job>) {
    while let Ok(job) = rx.recv() {
        let outcome = std::panic::catch_unwind(std::panic::AssertUnwindSafe(job));
        if outcome.is_err() {
            tracing::error!("磁盘写入任务异常结束");
        }
    }
}

pub fn enqueue(job: impl FnOnce() + Send + 'static) {
    if let Err(err) = queue().send(Box::new(job)) {
        tracing::error!(error = %err, "磁盘写入线程不可用");
    }
}

/// Runs `job` on the writer thread and delivers its result asynchronously.
pub fn submit<R: Send + 'static>(
    job: impl FnOnce() -> R + Send + 'static,
) -> futures::channel::oneshot::Receiver<R> {
    let (tx, rx) = futures::channel::oneshot::channel();
    enqueue(move || {
        if !tx.is_canceled() {
            let _ = tx.send(job());
        }
    });
    rx
}

/// Blocks until everything queued so far has been written. Only for shutdown,
/// where a few milliseconds do not affect responsiveness.
pub fn flush(timeout: Duration) {
    let (tx, rx) = mpsc::channel();
    let sent = queue()
        .send(Box::new(move || {
            let _ = tx.send(());
        }))
        .is_ok();
    if sent && rx.recv_timeout(timeout).is_err() {
        tracing::warn!("等待磁盘写入超时");
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;
    use std::sync::atomic::{AtomicUsize, Ordering};

    #[test]
    fn jobs_run_in_submission_order() {
        let order = Arc::new(std::sync::Mutex::new(Vec::new()));
        for index in 0..16 {
            let order = order.clone();
            enqueue(move || order.lock().unwrap().push(index));
        }
        flush(Duration::from_secs(5));
        assert_eq!(*order.lock().unwrap(), (0..16).collect::<Vec<_>>());
    }

    #[test]
    fn panicking_job_does_not_break_the_queue() {
        enqueue(|| panic!("expected test panic"));
        let counter = Arc::new(AtomicUsize::new(0));
        let job_counter = counter.clone();
        enqueue(move || {
            job_counter.fetch_add(1, Ordering::SeqCst);
        });
        flush(Duration::from_secs(5));
        assert_eq!(counter.load(Ordering::SeqCst), 1);
    }
}
