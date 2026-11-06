use windows::Win32::Foundation::ERROR_SUCCESS;
use windows::Win32::System::Registry::{HKEY_CURRENT_USER, RRF_RT_REG_SZ, RegGetValueW};
use windows::core::HSTRING;

/// Workspace folder picked on the installer's first-run page. The installer
/// stores it in the registry so non-ASCII paths survive; the app only reads it
/// while `config/runtime.toml` (or the legacy `config.json`) has no workspace
/// root yet.
pub fn first_run_workspace() -> Option<String> {
    // SAFETY: 键名与值名都是本地构造的 HSTRING，缓冲区容量按字节数传入，
    // 且只从 API 实际写回的长度里解码字符串。
    unsafe {
        let subkey = HSTRING::from("Software\\md-work");
        let name = HSTRING::from("WorkspaceRoot");
        let mut buf = vec![0u16; 1024];
        let mut len = (buf.len() * 2) as u32;
        let status = RegGetValueW(
            HKEY_CURRENT_USER,
            &subkey,
            &name,
            RRF_RT_REG_SZ,
            None,
            Some(buf.as_mut_ptr() as *mut _),
            Some(&mut len),
        );
        if status != ERROR_SUCCESS {
            return None;
        }
        let chars = (len as usize / 2).saturating_sub(1).min(buf.len());
        let path = String::from_utf16_lossy(&buf[..chars]).trim().to_string();
        (!path.is_empty()).then_some(path)
    }
}
