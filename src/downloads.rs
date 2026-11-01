//! Files arriving in the Windows Downloads folder. Browsers write a temporary
//! file while receiving and rename it when done, so the notch can show the
//! name and the bytes so far, then say when it lands. A folder notification
//! wakes the worker, which reads sizes only while a download is running.

use std::ffi::c_void;
use std::path::{Path, PathBuf};
use std::time::Duration;

use windows::Win32::Foundation::{HWND, LPARAM, WPARAM};
use windows::Win32::Storage::FileSystem::FILE_NOTIFY_CHANGE_FILE_NAME;
use windows::Win32::System::Com::CoTaskMemFree;
use windows::Win32::UI::Shell::{FOLDERID_Downloads, SHGetKnownFolderPath, KF_FLAG_DEFAULT};
use windows::Win32::UI::WindowsAndMessaging::{PostMessageW, WM_APP};

use crate::watch::Folder;

/// The Downloads folder changed. LPARAM: a `Box<Update>`.
pub const DOWNLOAD_MSG: u32 = WM_APP + 22;
/// How often the bytes of a running download are read.
const STEP: Duration = Duration::from_millis(700);

/// Browsers' extensions for a file still being written.
const TEMP: [&str; 6] = [
    "crdownload",
    "part",
    "partial",
    "download",
    "opdownload",
    "aria2",
];

/// What the Downloads folder holds now, and what just finished.
#[derive(Default)]
pub struct Update {
    /// Files still arriving: their final name and bytes written so far.
    pub active: Vec<(String, u64)>,
    /// Names that were being written and are now in place.
    pub done: Vec<String>,
}

/// The name a temporary `name` will have once its download finishes.
fn landed_name(name: &str) -> Option<String> {
    let path = Path::new(name);
    let extension = path.extension()?.to_str()?;
    if !TEMP
        .iter()
        .any(|known| extension.eq_ignore_ascii_case(known))
    {
        return None;
    }
    Some(path.file_stem()?.to_string_lossy().into_owned())
}

/// The downloads in `folder`, by final name.
fn scan(folder: &Path) -> Vec<(String, u64)> {
    let Ok(entries) = std::fs::read_dir(folder) else {
        return Vec::new();
    };
    let mut active = Vec::new();
    for entry in entries.flatten() {
        let name = entry.file_name().to_string_lossy().into_owned();
        if let Some(landed) = landed_name(&name) {
            let bytes = entry.metadata().map(|meta| meta.len()).unwrap_or(0);
            active.push((landed, bytes));
        }
    }
    active.sort();
    active
}

/// Names in `before` that are gone from `active`.
fn gone(before: &[(String, u64)], active: &[(String, u64)]) -> Vec<String> {
    before
        .iter()
        .filter(|(name, _)| !active.iter().any(|(still, _)| still == name))
        .map(|(name, _)| name.clone())
        .collect()
}

/// Bytes as a short label, such as "1.4 MB".
pub fn size_label(bytes: u64) -> String {
    const K: f64 = 1024.0;
    let bytes = bytes as f64;
    if bytes < K {
        format!("{bytes:.0} B")
    } else if bytes < K * K {
        format!("{:.0} KB", bytes / K)
    } else if bytes < K * K * K {
        format!("{:.1} MB", bytes / (K * K))
    } else {
        format!("{:.1} GB", bytes / (K * K * K))
    }
}

/// Takes the update a message carries, freeing the box.
pub fn take_update(lparam: LPARAM) -> Update {
    unsafe { *Box::from_raw(lparam.0 as *mut Update) }
}

/// Watches the folder on its own thread, asleep until something appears.
pub fn watch(hwnd: HWND) {
    let target = hwnd.0 as isize;
    std::thread::spawn(move || {
        let Some(folder) = folder() else {
            return;
        };
        let Ok(watched) = Folder::open(&folder) else {
            return;
        };
        let mut posted: Vec<(String, u64)> = Vec::new();
        loop {
            let active = scan(&folder);
            let mut done = Vec::new();
            for name in gone(&posted, &active) {
                // A cancelled download leaves no file behind.
                if folder.join(&name).is_file() {
                    done.push(name);
                }
            }
            if active != posted || !done.is_empty() {
                posted = active.clone();
                let update = Box::into_raw(Box::new(Update { active, done }));
                unsafe {
                    let _ = PostMessageW(
                        Some(HWND(target as *mut c_void)),
                        DOWNLOAD_MSG,
                        WPARAM(0),
                        LPARAM(update as isize),
                    );
                }
            }
            if posted.is_empty() {
                if watched.wait(FILE_NOTIFY_CHANGE_FILE_NAME).is_err() {
                    return;
                }
            } else {
                std::thread::sleep(STEP);
            }
        }
    });
}

fn folder() -> Option<PathBuf> {
    unsafe {
        let wide = SHGetKnownFolderPath(&FOLDERID_Downloads, KF_FLAG_DEFAULT, None).ok()?;
        let path = wide.to_string().ok();
        CoTaskMemFree(Some(wide.as_ptr() as *const c_void));
        path.map(PathBuf::from)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn browser_temp_files_have_a_landed_name() {
        assert_eq!(
            landed_name("Rapat 2026.pdf.crdownload").as_deref(),
            Some("Rapat 2026.pdf")
        );
        assert_eq!(landed_name("film.mkv.PART").as_deref(), Some("film.mkv"));
        assert_eq!(landed_name("Rapat.pdf"), None);
        assert_eq!(landed_name("tanpa ekstensi"), None);
    }

    #[test]
    fn a_finished_download_is_the_one_that_left() {
        let before = vec![("a.zip".to_string(), 10), ("b.mp4".to_string(), 20)];
        let active = vec![("b.mp4".to_string(), 30)];
        assert_eq!(gone(&before, &active), ["a.zip"]);
    }

    #[test]
    fn sizes_read_in_short_units() {
        assert_eq!(size_label(900), "900 B");
        assert_eq!(size_label(24 * 1024), "24 KB");
        assert_eq!(size_label(3 * 1024 * 1024 / 2), "1.5 MB");
        assert_eq!(size_label(2 * 1024 * 1024 * 1024), "2.0 GB");
    }
}
