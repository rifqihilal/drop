//! New pictures in the Windows Screenshots folder, where Snipping Tool saves
//! what PrtSc and Win+Shift+S capture, so they can go on the shelf.

use std::ffi::c_void;
use std::fs::OpenOptions;
use std::os::windows::fs::OpenOptionsExt;
use std::path::{Path, PathBuf};
use std::time::Duration;

use windows::Win32::Foundation::{HWND, LPARAM, WPARAM};
use windows::Win32::Storage::FileSystem::FILE_NOTIFY_CHANGE_FILE_NAME;
use windows::Win32::System::Com::CoTaskMemFree;
use windows::Win32::UI::Shell::{FOLDERID_Screenshots, SHGetKnownFolderPath, KF_FLAG_CREATE};
use windows::Win32::UI::WindowsAndMessaging::{PostMessageW, WM_APP};

use crate::watch::Folder;

/// A screenshot was saved. LPARAM: a `Box<PathBuf>`.
pub const SCREENSHOT_MSG: u32 = WM_APP + 15;
/// How often, and how many times, to look for a file to be fully written.
const RETRY: Duration = Duration::from_millis(100);
const RETRIES: u32 = 30;

/// Watches the folder on its own thread, asleep until a file appears.
pub fn watch(hwnd: HWND) {
    let target = hwnd.0 as isize;
    std::thread::spawn(move || {
        let Some(path) = folder() else {
            return;
        };
        let Ok(watched) = Folder::open(&path) else {
            return;
        };
        while let Ok(changes) = watched.wait(FILE_NOTIFY_CHANGE_FILE_NAME) {
            let mut files: Vec<PathBuf> = changes
                .iter()
                .filter(|change| change.added && is_picture(&change.name))
                .map(|change| path.join(&change.name))
                .collect();
            files.dedup();
            for file in files {
                if written(&file) {
                    let payload = Box::into_raw(Box::new(file));
                    unsafe {
                        let _ = PostMessageW(
                            Some(HWND(target as *mut c_void)),
                            SCREENSHOT_MSG,
                            WPARAM(0),
                            LPARAM(payload as isize),
                        );
                    }
                }
            }
        }
    });
}

fn folder() -> Option<PathBuf> {
    unsafe {
        let wide = SHGetKnownFolderPath(&FOLDERID_Screenshots, KF_FLAG_CREATE, None).ok()?;
        let path = wide.to_string().ok();
        CoTaskMemFree(Some(wide.as_ptr() as *const c_void));
        path.map(PathBuf::from)
    }
}

fn is_picture(name: &str) -> bool {
    Path::new(name)
        .extension()
        .and_then(|extension| extension.to_str())
        .is_some_and(|extension| {
            ["png", "jpg", "jpeg"]
                .iter()
                .any(|known| extension.eq_ignore_ascii_case(known))
        })
}

/// Waits until the writer is done: it holds the file open while saving, so
/// opening it with no sharing fails until then.
fn written(path: &Path) -> bool {
    for _ in 0..RETRIES {
        let closed = OpenOptions::new().read(true).share_mode(0).open(path);
        if closed.is_ok_and(|file| file.metadata().is_ok_and(|meta| meta.len() > 0)) {
            return true;
        }
        std::thread::sleep(RETRY);
    }
    false
}

/// Takes ownership of the path carried by a [`SCREENSHOT_MSG`].
///
/// # Safety
/// `lparam` must come from a [`SCREENSHOT_MSG`] and be taken only once.
pub unsafe fn take_path(lparam: LPARAM) -> PathBuf {
    *Box::from_raw(lparam.0 as *mut PathBuf)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_pictures_count() {
        assert!(is_picture("Screenshot 2026-10-30 225411.png"));
        assert!(is_picture("SHOT.JPG"));
        assert!(!is_picture("Screenshot.mp4"));
        assert!(!is_picture("desktop.ini"));
        assert!(!is_picture("png"));
    }
}
