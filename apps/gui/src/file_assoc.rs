use windows::Win32::Foundation::ERROR_SUCCESS;
use windows::Win32::System::Registry::{
    HKEY, HKEY_CURRENT_USER, KEY_SET_VALUE, REG_OPTION_NON_VOLATILE, REG_SZ, RRF_RT_REG_SZ,
    RegCloseKey, RegCreateKeyExW, RegGetValueW, RegSetValueExW,
};
use windows::core::{HSTRING, PCWSTR};

const PROGID: &str = "MoJian.Markdown";
const VERB: &str = "OpenWithMoJian";

fn set_string(
    root: HKEY,
    subkey: &str,
    name: Option<&str>,
    value: &str,
) -> windows::core::Result<()> {
    // SAFETY: 键名、值名都是本地构造的 HSTRING，输出句柄仅在本函数内使用并关闭。
    unsafe {
        let subkey = HSTRING::from(subkey);
        let mut key = HKEY::default();
        let status = RegCreateKeyExW(
            root,
            &subkey,
            None,
            PCWSTR::null(),
            REG_OPTION_NON_VOLATILE,
            KEY_SET_VALUE,
            None,
            &mut key,
            None,
        );
        if status != ERROR_SUCCESS {
            return Err(windows::core::Error::from_win32());
        }
        let name = name.map(HSTRING::from);
        let data: Vec<u16> = value.encode_utf16().chain(std::iter::once(0)).collect();
        let bytes = std::slice::from_raw_parts(data.as_ptr() as *const u8, data.len() * 2);
        let result = RegSetValueExW(
            key,
            name.as_ref()
                .map(|name| PCWSTR(name.as_ptr()))
                .unwrap_or(PCWSTR::null()),
            None,
            REG_SZ,
            Some(bytes),
        );
        let _ = RegCloseKey(key);
        if result != ERROR_SUCCESS {
            return Err(windows::core::Error::from_win32());
        }
        Ok(())
    }
}

/// Registers the per-user "用墨笺打开" context menu entry and the ProgID for
/// `.md` files. Only HKCU is touched, so no elevation is required and the
/// Windows default-app choice (`UserChoice`) is left untouched.
pub fn register(exe: &str, icon: &str) -> windows::core::Result<()> {
    let classes = "Software\\Classes";

    set_string(
        HKEY_CURRENT_USER,
        &format!("{classes}\\{PROGID}"),
        None,
        "Markdown 笔记",
    )?;
    set_string(
        HKEY_CURRENT_USER,
        &format!("{classes}\\{PROGID}\\DefaultIcon"),
        None,
        icon,
    )?;
    set_string(
        HKEY_CURRENT_USER,
        &format!("{classes}\\{PROGID}\\shell\\open\\command"),
        None,
        &format!("\"{exe}\" \"%1\""),
    )?;

    let verb_key = format!("{classes}\\SystemFileAssociations\\.md\\shell\\{VERB}");
    set_string(HKEY_CURRENT_USER, &verb_key, None, "用墨笺打开")?;
    set_string(HKEY_CURRENT_USER, &verb_key, Some("Icon"), icon)?;
    set_string(
        HKEY_CURRENT_USER,
        &format!("{verb_key}\\command"),
        None,
        &format!("\"{exe}\" \"%1\""),
    )?;

    set_string(
        HKEY_CURRENT_USER,
        &format!("{classes}\\.md\\OpenWithProgids"),
        Some(PROGID),
        "",
    )?;

    Ok(())
}

pub fn ensure_registered() -> windows::core::Result<()> {
    if is_registered() {
        return Ok(());
    }
    let exe = std::env::current_exe()?;
    let exe = exe.to_string_lossy().into_owned();
    let icon = format!("{exe},0");
    register(&exe, &icon)
}

/// Whether the HKCU context-menu entry already points at this executable.
pub fn is_registered() -> bool {
    let Ok(exe) = std::env::current_exe() else {
        return false;
    };
    let expected = format!("\"{}\" \"%1\"", exe.to_string_lossy());
    current_command().as_deref() == Some(expected.as_str())
}

fn current_command() -> Option<String> {
    // SAFETY: 键名是本地构造的 HSTRING，缓冲区长度按字节数传入，解码范围限于写回长度。
    unsafe {
        let subkey = HSTRING::from("Software\\Classes\\MoJian.Markdown\\shell\\open\\command");
        let mut buf = vec![0u16; 1024];
        let mut len = (buf.len() * 2) as u32;
        let status = RegGetValueW(
            HKEY_CURRENT_USER,
            &subkey,
            PCWSTR::null(),
            RRF_RT_REG_SZ,
            None,
            Some(buf.as_mut_ptr() as *mut _),
            Some(&mut len),
        );
        if status != ERROR_SUCCESS {
            return None;
        }
        let chars = (len as usize / 2).saturating_sub(1).min(buf.len());
        Some(String::from_utf16_lossy(&buf[..chars]))
    }
}
