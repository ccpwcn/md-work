//! Process liveness probe used to discard stale dirty-buffer markers.

use windows::Win32::Foundation::{CloseHandle, STILL_ACTIVE};
use windows::Win32::System::Threading::{
    GetExitCodeProcess, OpenProcess, PROCESS_QUERY_LIMITED_INFORMATION,
};

pub fn alive(pid: u32) -> bool {
    if pid == 0 {
        return false;
    }
    unsafe {
        let Ok(handle) = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, false, pid) else {
            return false;
        };
        let mut code = 0u32;
        let running = GetExitCodeProcess(handle, &mut code).is_ok()
            && code == STILL_ACTIVE.0 as u32;
        let _ = CloseHandle(handle);
        running
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn current_process_is_alive_and_garbage_is_not() {
        assert!(alive(std::process::id()));
        assert!(!alive(0));
        assert!(!alive(0xFFFF_FFF0));
    }
}
