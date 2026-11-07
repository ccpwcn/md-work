//! Windows print dialog + GDI text output (VNote-style: dialog then print).

use std::mem::size_of;

use windows::Win32::Foundation::{GlobalFree, HWND};
use windows::Win32::Graphics::Gdi::{
    DeleteDC, ETO_OPTIONS, ExtTextOutW, GET_DEVICE_CAPS_INDEX, GetDeviceCaps, GetTextMetricsW, HDC,
    HORZRES, TEXTMETRICW, VERTRES,
};
use windows::Win32::Storage::Xps::{DOCINFOW, EndDoc, EndPage, StartDocW, StartPage};
use windows::Win32::UI::Controls::Dialogs::{
    PD_RETURNDC, PD_SELECTION, PD_USEDEVMODECOPIESANDCOLLATE, PRINTDLGW, PrintDlgW,
};
use windows::core::PCWSTR;

/// Outcome of a print attempt.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PrintResult {
    Cancelled,
    Printed,
    Failed(String),
}

/// Shows the system print dialog and prints `text` to the selected printer.
///
/// When `allow_selection` is true, the Print Selection option is enabled in
/// the dialog (caller should already pass the selection body when appropriate).
pub fn print_text(title: &str, text: &str, allow_selection: bool) -> PrintResult {
    let hwnd = HWND::default();
    let mut flags = PD_RETURNDC.0 | PD_USEDEVMODECOPIESANDCOLLATE.0;
    if allow_selection {
        flags |= PD_SELECTION.0;
    }

    let mut pd = PRINTDLGW {
        lStructSize: size_of::<PRINTDLGW>() as u32,
        hwndOwner: hwnd,
        Flags: windows::Win32::UI::Controls::Dialogs::PRINTDLGEX_FLAGS(flags),
        nFromPage: 1,
        nToPage: 1,
        nMinPage: 1,
        nMaxPage: 1,
        nCopies: 1,
        ..Default::default()
    };

    // SAFETY: pd 已完整初始化，且在整个调用期间由本函数独占。
    let accepted = unsafe { PrintDlgW(&mut pd) };
    if !accepted.as_bool() {
        cleanup_print_dlg(&pd);
        return PrintResult::Cancelled;
    }

    let hdc = pd.hDC;
    if hdc.is_invalid() {
        cleanup_print_dlg(&pd);
        return PrintResult::Failed("打印机设备上下文无效".into());
    }

    // SAFETY: hdc 来自 PrintDlgW 且刚校验过有效，调用期间不会被释放。
    let result = unsafe { print_with_dc(hdc, title, text) };
    // SAFETY: hdc 由对话框创建，仅在此处释放一次。
    unsafe {
        let _ = DeleteDC(hdc);
    }
    cleanup_print_dlg(&pd);
    result
}

fn cleanup_print_dlg(pd: &PRINTDLGW) {
    // SAFETY: 句柄来自 PRINTDLGW，已判空且每个只释放一次。
    unsafe {
        if !pd.hDevMode.is_invalid() {
            let _ = GlobalFree(Some(pd.hDevMode));
        }
        if !pd.hDevNames.is_invalid() {
            let _ = GlobalFree(Some(pd.hDevNames));
        }
    }
}

/// # Safety
/// `hdc` 必须是有效、且在本次调用期间保持存活的打印机设备上下文。
unsafe fn print_with_dc(hdc: HDC, title: &str, text: &str) -> PrintResult {
    // SAFETY: 宽字符串缓冲区在同步 GDI 调用期间保持存活，指针不会被保存。
    unsafe {
        let title_wide: Vec<u16> = title.encode_utf16().chain(std::iter::once(0)).collect();
        let doc = DOCINFOW {
            cbSize: size_of::<DOCINFOW>() as i32,
            lpszDocName: PCWSTR(title_wide.as_ptr()),
            lpszOutput: PCWSTR::null(),
            lpszDatatype: PCWSTR::null(),
            fwType: 0,
        };

        if StartDocW(hdc, &doc) <= 0 {
            return PrintResult::Failed("无法开始打印任务".into());
        }

        let mut metrics = TEXTMETRICW::default();
        if !GetTextMetricsW(hdc, &mut metrics).as_bool() {
            let _ = EndDoc(hdc);
            return PrintResult::Failed("无法读取打印机字体度量".into());
        }

        let page_width = GetDeviceCaps(Some(hdc), GET_DEVICE_CAPS_INDEX(HORZRES.0));
        let page_height = GetDeviceCaps(Some(hdc), GET_DEVICE_CAPS_INDEX(VERTRES.0));
        let margin = (page_width.max(1) / 20).max(40);
        let usable_width = (page_width - margin * 2).max(1) as usize;
        let line_height = (metrics.tmHeight + metrics.tmExternalLeading).max(1);
        let lines_per_page = ((page_height - margin * 2) / line_height).max(1) as usize;

        let lines = wrap_lines(text, usable_width, |s| text_width_approx(s, &metrics));
        let mut line_idx = 0usize;
        let mut ok = true;

        while line_idx < lines.len() {
            if StartPage(hdc) <= 0 {
                ok = false;
                break;
            }
            let page_end = (line_idx + lines_per_page).min(lines.len());
            for (row, line) in lines[line_idx..page_end].iter().enumerate() {
                let y = margin + row as i32 * line_height;
                let wide: Vec<u16> = line.encode_utf16().collect();
                if wide.is_empty() {
                    continue;
                }
                let drawn = ExtTextOutW(
                    hdc,
                    margin,
                    y,
                    ETO_OPTIONS(0),
                    None,
                    PCWSTR(wide.as_ptr()),
                    wide.len() as u32,
                    None,
                );
                if !drawn.as_bool() {
                    ok = false;
                    break;
                }
            }
            if EndPage(hdc) <= 0 {
                ok = false;
                break;
            }
            line_idx = page_end;
            if !ok {
                break;
            }
        }

        let _ = EndDoc(hdc);
        if ok {
            PrintResult::Printed
        } else {
            PrintResult::Failed("打印过程中出错".into())
        }
    }
}

fn text_width_approx(s: &str, metrics: &TEXTMETRICW) -> usize {
    let avg = metrics.tmAveCharWidth.max(1) as usize;
    s.chars().count() * avg
}

fn wrap_lines(text: &str, max_width: usize, width_of: impl Fn(&str) -> usize) -> Vec<String> {
    let mut out = Vec::new();
    for paragraph in text.split('\n') {
        if paragraph.is_empty() {
            out.push(String::new());
            continue;
        }
        let mut current = String::new();
        for ch in paragraph.chars() {
            let mut candidate = current.clone();
            candidate.push(ch);
            if !current.is_empty() && width_of(&candidate) > max_width {
                out.push(std::mem::take(&mut current));
                current.push(ch);
            } else {
                current = candidate;
            }
        }
        out.push(current);
    }
    if out.is_empty() {
        out.push(String::new());
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn wraps_long_lines() {
        let lines = wrap_lines("abcdefghij", 30, |s| s.len() * 10);
        assert!(lines.len() >= 2);
        assert_eq!(lines.concat(), "abcdefghij");
    }

    #[test]
    fn keeps_empty_paragraphs() {
        let lines = wrap_lines("a\n\nb", 1000, |s| s.len());
        assert_eq!(lines, vec!["a".to_string(), String::new(), "b".to_string()]);
    }
}
