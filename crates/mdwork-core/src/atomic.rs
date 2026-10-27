use std::fs::{self, File};
use std::io::{self, Write};
use std::path::Path;

use uuid::Uuid;

/// Writes to a sibling temporary file, flushes it to disk, then replaces the
/// target. Never truncates the target in place.
pub fn write_atomic(path: &Path, data: &[u8]) -> io::Result<()> {
    let parent = path.parent().unwrap_or_else(|| Path::new("."));
    fs::create_dir_all(parent)?;

    let file_name = path
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or("document");
    let tmp = parent.join(format!(".{file_name}.{}.tmp", Uuid::new_v4()));

    let mut file = File::create(&tmp)?;
    let result = file.write_all(data).and_then(|_| file.sync_all());
    drop(file);

    if let Err(err) = result {
        let _ = fs::remove_file(&tmp);
        return Err(err);
    }

    if let Err(err) = replace(&tmp, path) {
        let _ = fs::remove_file(&tmp);
        return Err(err);
    }
    Ok(())
}

#[cfg(windows)]
fn replace(tmp: &Path, target: &Path) -> io::Result<()> {
    use windows::core::{HSTRING, PCWSTR};
    use windows::Win32::Storage::FileSystem::{ReplaceFileW, REPLACE_FILE_FLAGS};

    if !target.exists() {
        return fs::rename(tmp, target);
    }

    let target_w = HSTRING::from(target.to_string_lossy().as_ref());
    let tmp_w = HSTRING::from(tmp.to_string_lossy().as_ref());
    let result = unsafe {
        ReplaceFileW(
            &target_w,
            &tmp_w,
            PCWSTR::null(),
            REPLACE_FILE_FLAGS(0),
            None,
            None,
        )
    };

    match result {
        Ok(()) => Ok(()),
        Err(_) => fs::rename(tmp, target),
    }
}

#[cfg(not(windows))]
fn replace(tmp: &Path, target: &Path) -> io::Result<()> {
    fs::rename(tmp, target)
}
