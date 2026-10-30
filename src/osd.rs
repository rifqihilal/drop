//! Hides Windows' own volume/brightness flyout while Drop shows them instead.
//!
//! Brightness keys go straight from the firmware to the display driver, so the
//! flyout cannot be prevented; its host window is made fully transparent and
//! click-through instead, and restored when Drop exits.

use windows::core::w;
use windows::Win32::Foundation::{COLORREF, HWND};
use windows::Win32::UI::WindowsAndMessaging::{
    FindWindowExW, GetLayeredWindowAttributes, GetWindowLongW, SetLayeredWindowAttributes,
    SetWindowLongW, GWL_EXSTYLE, LWA_ALPHA, WS_EX_LAYERED, WS_EX_TRANSPARENT,
};

pub fn set_suppressed(suppress: bool) {
    for hwnd in flyout_hosts() {
        unsafe {
            let ex_style = GetWindowLongW(hwnd, GWL_EXSTYLE) as u32;
            let mut alpha = 255u8;
            // Explorer never makes this window layered itself, so a layered host
            // at alpha 0 was hidden by Drop, possibly by a run that crashed.
            let suppressed = (ex_style & WS_EX_LAYERED.0) != 0
                && GetLayeredWindowAttributes(hwnd, None, Some(&mut alpha), None).is_ok()
                && alpha == 0;
            let flags = WS_EX_LAYERED.0 | WS_EX_TRANSPARENT.0;
            if suppress && !suppressed {
                SetWindowLongW(hwnd, GWL_EXSTYLE, (ex_style | flags) as i32);
                let _ = SetLayeredWindowAttributes(hwnd, COLORREF(0), 0, LWA_ALPHA);
            } else if !suppress && suppressed {
                let _ = SetLayeredWindowAttributes(hwnd, COLORREF(0), 255, LWA_ALPHA);
                SetWindowLongW(hwnd, GWL_EXSTYLE, (ex_style & !flags) as i32);
            }
        }
    }
}

/// Every top-level window of the flyout's class.
fn flyout_hosts() -> Vec<HWND> {
    let mut hosts = Vec::new();
    let mut after = None;
    while let Ok(hwnd) = unsafe { FindWindowExW(None, after, w!("NativeHWNDHost"), None) } {
        if hwnd.is_invalid() {
            break;
        }
        hosts.push(hwnd);
        after = Some(hwnd);
    }
    hosts
}
