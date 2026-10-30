use std::fs::OpenOptions;
use std::io::Write as _;
use std::sync::mpsc::{self, Receiver};

use windows::Win32::Foundation::{CloseHandle, INVALID_HANDLE_VALUE};
use windows::Win32::Storage::FileSystem::{PIPE_ACCESS_DUPLEX, ReadFile};
use windows::Win32::System::Pipes::{
    ConnectNamedPipe, CreateNamedPipeW, DisconnectNamedPipe, PIPE_READMODE_MESSAGE,
    PIPE_TYPE_MESSAGE, PIPE_UNLIMITED_INSTANCES, PIPE_WAIT,
};
use windows::core::HSTRING;

const MAX_PAYLOAD: usize = 32 * 1024;

fn pipe_name() -> String {
    let user = std::env::var("USERNAME").unwrap_or_else(|_| "unknown".to_string());
    format!(r"\\.\pipe\md-work-{user}-single")
}

/// Delivers argv to the running instance. Returns false when no primary exists.
pub fn try_forward(payload: &str) -> bool {
    match OpenOptions::new().write(true).open(pipe_name()) {
        Ok(mut pipe) => writeln!(pipe, "{payload}").is_ok(),
        Err(_) => false,
    }
}

/// Starts the primary listener. The channel yields one payload per connection.
pub fn spawn_server() -> Receiver<String> {
    let (tx, rx) = mpsc::channel();
    std::thread::spawn(move || while serve_one(&tx) {});
    rx
}

fn serve_one(tx: &mpsc::Sender<String>) -> bool {
    let name = HSTRING::from(pipe_name());
    let handle = unsafe {
        CreateNamedPipeW(
            &name,
            PIPE_ACCESS_DUPLEX,
            PIPE_TYPE_MESSAGE | PIPE_READMODE_MESSAGE | PIPE_WAIT,
            PIPE_UNLIMITED_INSTANCES,
            MAX_PAYLOAD as u32,
            MAX_PAYLOAD as u32,
            0,
            None,
        )
    };
    if handle == INVALID_HANDLE_VALUE {
        return false;
    }

    let _ = unsafe { ConnectNamedPipe(handle, None) };

    let mut buf = vec![0u8; MAX_PAYLOAD];
    let mut read = 0u32;
    let connected = unsafe { ReadFile(handle, Some(&mut buf), Some(&mut read), None) }.is_ok();

    if connected && read > 0 {
        let text = String::from_utf8_lossy(&buf[..read as usize]);
        let line = text.lines().next().unwrap_or_default();
        if !line.is_empty() {
            let _ = tx.send(line.to_string());
        }
    }

    unsafe {
        let _ = DisconnectNamedPipe(handle);
        let _ = CloseHandle(handle);
    }
    true
}
