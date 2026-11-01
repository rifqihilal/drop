//! Files arriving in the Windows Downloads folder. Browsers write a temporary
//! file while receiving and rename it when done, so the notch can show the
//! name and the bytes so far, then say when it lands, went away or stopped
//! growing. Only the file is seen, so a pause cannot be told from a lost
//! connection, nor a cancelled download from a failed one. A folder
//! notification wakes the worker, which reads sizes only while one is running.

use std::collections::HashMap;
use std::ffi::c_void;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use windows::Win32::Foundation::{HWND, LPARAM, WPARAM};
use windows::Win32::Storage::FileSystem::{FILE_NOTIFY_CHANGE_FILE_NAME, FILE_NOTIFY_CHANGE_SIZE};
use windows::Win32::System::Com::CoTaskMemFree;
use windows::Win32::UI::Shell::{FOLDERID_Downloads, SHGetKnownFolderPath, KF_FLAG_DEFAULT};
use windows::Win32::UI::WindowsAndMessaging::{PostMessageW, WM_APP};

use crate::watch::Folder;

/// The Downloads folder changed. LPARAM: a `Box<Update>`.
pub const DOWNLOAD_MSG: u32 = WM_APP + 22;
/// How often the bytes of a running download are read.
const STEP: Duration = Duration::from_millis(700);
/// A download whose file does not grow for this long counts as stopped.
const STALL: Duration = Duration::from_secs(30);

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
    /// Names whose temporary file went without leaving a finished one.
    pub cancelled: Vec<String>,
    /// Names that just stopped growing.
    pub stalled: Vec<String>,
}

struct Tracked {
    bytes: u64,
    changed: Instant,
    /// Not growing, so left out of what is showing.
    stalled: bool,
}

/// The temporary files in the folder, and whether each is still growing.
#[derive(Default)]
struct Tracker {
    files: HashMap<String, Tracked>,
    /// One look was had: files found by the first are leftovers until they
    /// grow, and say nothing when they stop.
    started: bool,
}

/// What one look at the folder found.
#[derive(Default, PartialEq, Debug)]
struct Look {
    active: Vec<(String, u64)>,
    gone: Vec<String>,
    stalled: Vec<String>,
}

impl Tracker {
    fn look(&mut self, scanned: Vec<(String, u64)>, now: Instant) -> Look {
        let mut look = Look {
            gone: self
                .files
                .keys()
                .filter(|name| !scanned.iter().any(|(still, _)| still == *name))
                .cloned()
                .collect(),
            ..Look::default()
        };
        look.gone.sort();
        for name in &look.gone {
            self.files.remove(name);
        }
        for (name, bytes) in scanned {
            let leftover = !self.started;
            let file = self.files.entry(name.clone()).or_insert(Tracked {
                bytes,
                changed: now,
                stalled: leftover,
            });
            if file.bytes != bytes {
                file.bytes = bytes;
                file.changed = now;
                file.stalled = false;
            }
            if !file.stalled && now.duration_since(file.changed) >= STALL {
                file.stalled = true;
                look.stalled.push(name.clone());
            }
            if !file.stalled {
                look.active.push((name, bytes));
            }
        }
        self.started = true;
        look
    }

    /// Whether any file is still growing, which is worth reading again soon.
    fn has_active(&self) -> bool {
        self.files.values().any(|file| !file.stalled)
    }
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
        let mut tracker = Tracker::default();
        let mut posted: Vec<(String, u64)> = Vec::new();
        loop {
            let look = tracker.look(scan(&folder), Instant::now());
            // A download that went either landed or left nothing behind.
            let (done, cancelled): (Vec<String>, Vec<String>) = look
                .gone
                .into_iter()
                .partition(|name| folder.join(name).is_file());
            let quiet = done.is_empty() && cancelled.is_empty() && look.stalled.is_empty();
            if look.active != posted || !quiet {
                posted = look.active.clone();
                let update = Box::into_raw(Box::new(Update {
                    active: look.active,
                    done,
                    cancelled,
                    stalled: look.stalled,
                }));
                unsafe {
                    let _ = PostMessageW(
                        Some(HWND(target as *mut c_void)),
                        DOWNLOAD_MSG,
                        WPARAM(0),
                        LPARAM(update as isize),
                    );
                }
            }
            if tracker.has_active() {
                std::thread::sleep(STEP);
            } else if watched
                .wait(FILE_NOTIFY_CHANGE_FILE_NAME | FILE_NOTIFY_CHANGE_SIZE)
                .is_err()
            {
                return;
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

    fn file(name: &str, bytes: u64) -> (String, u64) {
        (name.to_string(), bytes)
    }

    #[test]
    fn a_download_that_left_is_gone() {
        let now = Instant::now();
        let mut tracker = Tracker::default();
        tracker.look(Vec::new(), now);
        tracker.look(vec![file("a.zip", 10), file("b.mp4", 20)], now);
        let look = tracker.look(vec![file("b.mp4", 30)], now);
        assert_eq!(look.gone, ["a.zip"]);
        assert_eq!(look.active, [file("b.mp4", 30)]);
    }

    #[test]
    fn a_download_that_stops_growing_is_told_once() {
        let now = Instant::now();
        let mut tracker = Tracker::default();
        tracker.look(Vec::new(), now);
        tracker.look(vec![file("a.zip", 10)], now);
        let later = now + STALL;
        let look = tracker.look(vec![file("a.zip", 10)], later);
        assert_eq!(look.stalled, ["a.zip"]);
        assert!(look.active.is_empty());
        assert!(!tracker.has_active());
        let look = tracker.look(vec![file("a.zip", 10)], later + STALL);
        assert!(look.stalled.is_empty());
    }

    #[test]
    fn a_stopped_download_comes_back_when_it_grows() {
        let now = Instant::now();
        let mut tracker = Tracker::default();
        tracker.look(Vec::new(), now);
        tracker.look(vec![file("a.zip", 10)], now);
        tracker.look(vec![file("a.zip", 10)], now + STALL);
        let look = tracker.look(vec![file("a.zip", 11)], now + 2 * STALL);
        assert_eq!(look.active, [file("a.zip", 11)]);
        assert!(tracker.has_active());
    }

    #[test]
    fn leftovers_stay_quiet() {
        let now = Instant::now();
        let mut tracker = Tracker::default();
        let look = tracker.look(vec![file("lama.zip", 10)], now);
        assert_eq!(look, Look::default());
        let look = tracker.look(vec![file("lama.zip", 10)], now + 2 * STALL);
        assert_eq!(look, Look::default());
    }

    #[test]
    fn sizes_read_in_short_units() {
        assert_eq!(size_label(900), "900 B");
        assert_eq!(size_label(24 * 1024), "24 KB");
        assert_eq!(size_label(3 * 1024 * 1024 / 2), "1.5 MB");
        assert_eq!(size_label(2 * 1024 * 1024 * 1024), "2.0 GB");
    }
}
