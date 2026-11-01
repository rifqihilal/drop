//! Whether an app is using the camera or the microphone.
//!
//! Windows keeps for each app when it last started and stopped using either
//! one, under the capability consent store. A worker thread sleeps on a
//! registry notification for that store, so nothing polls.

use std::ffi::c_void;

use windows::core::{w, PCWSTR};
use windows::Win32::Foundation::{CloseHandle, HANDLE, HWND, LPARAM, WAIT_OBJECT_0, WPARAM};
use windows::Win32::System::Registry::{
    RegCloseKey, RegEnumKeyExW, RegGetValueW, RegNotifyChangeKeyValue, RegOpenKeyExW, HKEY,
    HKEY_CURRENT_USER, KEY_NOTIFY, KEY_READ, REG_NOTIFY_CHANGE_LAST_SET, RRF_RT_REG_QWORD,
};
use windows::Win32::System::Threading::{CreateEventW, WaitForMultipleObjects, INFINITE};
use windows::Win32::UI::WindowsAndMessaging::{PostMessageW, WM_APP};

/// The camera or microphone started or stopped being used. WPARAM: the
/// [`CAMERA`] and [`MIC`] bits of what is in use now.
pub const PRIVACY_MSG: u32 = WM_APP + 20;
pub const CAMERA: usize = 1;
pub const MIC: usize = 2;

const STORE: &str =
    r"Software\Microsoft\Windows\CurrentVersion\CapabilityAccessManager\ConsentStore";
/// Apps that are not from the Store sit one level further down.
const DESKTOP_APPS: &str = "NonPackaged";
const NAME_CHARS: usize = 512;

/// An app is using the device between a start and the stop that follows it;
/// while it does, Windows leaves the stop at zero.
fn in_use(start: u64, stop: u64) -> bool {
    start != 0 && stop == 0
}

fn wide(text: &str) -> Vec<u16> {
    text.encode_utf16().chain([0]).collect()
}

/// A 64-bit value of the key at `path` under `root`, or 0 when it is not there.
unsafe fn qword(root: HKEY, path: &str, name: PCWSTR) -> u64 {
    let mut value = 0u64;
    let mut size = size_of::<u64>() as u32;
    let path = wide(path);
    let read = RegGetValueW(
        root,
        PCWSTR(path.as_ptr()),
        name,
        RRF_RT_REG_QWORD,
        None,
        Some((&raw mut value).cast::<c_void>()),
        Some(&mut size),
    );
    if read.is_ok() {
        value
    } else {
        0
    }
}

/// The names of the keys under `path`.
unsafe fn subkeys(root: HKEY, path: &str) -> Vec<String> {
    let mut key = HKEY::default();
    let path = wide(path);
    if RegOpenKeyExW(root, PCWSTR(path.as_ptr()), None, KEY_READ, &mut key).is_err() {
        return Vec::new();
    }
    let mut names = Vec::new();
    for index in 0.. {
        let mut name = [0u16; NAME_CHARS];
        let mut length = NAME_CHARS as u32;
        let found = RegEnumKeyExW(
            key,
            index,
            Some(windows::core::PWSTR(name.as_mut_ptr())),
            &mut length,
            None,
            None,
            None,
            None,
        );
        if found.is_err() {
            break;
        }
        names.push(String::from_utf16_lossy(&name[..length as usize]));
    }
    let _ = RegCloseKey(key);
    names
}

/// Whether any app is using the device whose consent store key is `device`.
unsafe fn device_in_use(root: HKEY, device: &str) -> bool {
    let used = |path: &str| {
        in_use(
            qword(root, path, w!("LastUsedTimeStart")),
            qword(root, path, w!("LastUsedTimeStop")),
        )
    };
    let base = format!(r"{STORE}\{device}");
    subkeys(root, &base).into_iter().any(|name| {
        if name == DESKTOP_APPS {
            subkeys(root, &format!(r"{base}\{name}"))
                .into_iter()
                .any(|app| used(&format!(r"{base}\{name}\{app}")))
        } else {
            used(&format!(r"{base}\{name}"))
        }
    })
}

fn state() -> usize {
    unsafe {
        let mut state = 0;
        if device_in_use(HKEY_CURRENT_USER, "webcam") {
            state |= CAMERA;
        }
        if device_in_use(HKEY_CURRENT_USER, "microphone") {
            state |= MIC;
        }
        state
    }
}

/// Posts [`PRIVACY_MSG`] at the start and whenever the state changes.
pub fn watch(hwnd: HWND) {
    let target = hwnd.0 as isize;
    std::thread::spawn(move || unsafe {
        let mut keys = Vec::new();
        for device in ["webcam", "microphone"] {
            let path = wide(&format!(r"{STORE}\{device}"));
            let mut key = HKEY::default();
            if RegOpenKeyExW(
                HKEY_CURRENT_USER,
                PCWSTR(path.as_ptr()),
                None,
                KEY_NOTIFY,
                &mut key,
            )
            .is_ok()
            {
                keys.push(key);
            }
        }
        let Ok(event) = CreateEventW(None, false, false, None) else {
            return;
        };
        if keys.is_empty() {
            let _ = CloseHandle(event);
            return;
        }
        let mut last = usize::MAX;
        loop {
            // Armed before reading, so a change in between is not lost.
            for &key in &keys {
                let armed = RegNotifyChangeKeyValue(
                    key,
                    true,
                    REG_NOTIFY_CHANGE_LAST_SET,
                    Some(event),
                    true,
                );
                if armed.is_err() {
                    return;
                }
            }
            let now = state();
            if now != last {
                last = now;
                let _ = PostMessageW(
                    Some(HWND(target as *mut c_void)),
                    PRIVACY_MSG,
                    WPARAM(now),
                    LPARAM(0),
                );
            }
            let handles: [HANDLE; 1] = [event];
            if WaitForMultipleObjects(&handles, false, INFINITE) != WAIT_OBJECT_0 {
                return;
            }
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn in_use_between_a_start_and_its_stop() {
        assert!(!in_use(0, 0), "never used");
        assert!(in_use(133_000_000_000_000_000, 0), "started, not stopped");
        assert!(
            !in_use(133_000_000_000_000_000, 133_000_000_100_000_000),
            "stopped"
        );
    }
}
