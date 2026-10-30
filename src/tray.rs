//! Notification-area icon: a pair of eyes drawn at runtime, recolored to stay
//! visible on both light and dark taskbars.

use std::cell::Cell;
use std::ffi::c_void;
use std::sync::OnceLock;

use windows::core::w;
use windows::Win32::Foundation::HWND;
use windows::Win32::Graphics::Gdi::{
    CreateBitmap, CreateDIBSection, DeleteObject, BITMAPINFO, BITMAPINFOHEADER, BI_RGB,
    DIB_RGB_COLORS,
};
use windows::Win32::System::Registry::{RegGetValueW, HKEY_CURRENT_USER, RRF_RT_REG_DWORD};
use windows::Win32::UI::Shell::{
    Shell_NotifyIconW, NIF_ICON, NIF_MESSAGE, NIF_TIP, NIM_ADD, NIM_DELETE, NIM_MODIFY,
    NOTIFYICONDATAW,
};
use windows::Win32::UI::WindowsAndMessaging::{
    CreateIconIndirect, DestroyIcon, GetSystemMetrics, RegisterWindowMessageW, HICON, ICONINFO,
    SM_CXSMICON, WM_APP,
};

/// Message the shell sends when the icon is clicked.
pub const CALLBACK_MSG: u32 = WM_APP + 1;
const ICON_ID: u32 = 1;

thread_local! {
    static ICON: Cell<Option<HICON>> = const { Cell::new(None) };
}

/// Broadcast when Explorer restarts; the icon has to be added again.
pub fn taskbar_created_msg() -> u32 {
    static MSG: OnceLock<u32> = OnceLock::new();
    *MSG.get_or_init(|| unsafe { RegisterWindowMessageW(w!("TaskbarCreated")) })
}

pub fn add(hwnd: HWND) {
    update(hwnd, true);
}

/// Redraws the icon for the current taskbar theme.
pub fn refresh(hwnd: HWND) {
    update(hwnd, false);
}

pub fn remove(hwnd: HWND) {
    let data = NOTIFYICONDATAW {
        cbSize: size_of::<NOTIFYICONDATAW>() as u32,
        hWnd: hwnd,
        uID: ICON_ID,
        ..Default::default()
    };
    unsafe {
        let _ = Shell_NotifyIconW(NIM_DELETE, &data);
    }
    replace_icon(None);
}

fn update(hwnd: HWND, add: bool) {
    let Some(icon) = eyes_icon(taskbar_is_light()) else {
        return;
    };
    let mut data = NOTIFYICONDATAW {
        cbSize: size_of::<NOTIFYICONDATAW>() as u32,
        hWnd: hwnd,
        uID: ICON_ID,
        uFlags: NIF_ICON | NIF_MESSAGE | NIF_TIP,
        uCallbackMessage: CALLBACK_MSG,
        hIcon: icon,
        ..Default::default()
    };
    for (slot, unit) in data.szTip.iter_mut().zip("Drop".encode_utf16()) {
        *slot = unit;
    }
    unsafe {
        let _ = Shell_NotifyIconW(if add { NIM_ADD } else { NIM_MODIFY }, &data);
    }
    replace_icon(Some(icon));
}

fn replace_icon(icon: Option<HICON>) {
    if let Some(old) = ICON.with(|cell| cell.replace(icon)) {
        unsafe {
            let _ = DestroyIcon(old);
        }
    }
}

/// True when Windows uses the light theme for the taskbar.
fn taskbar_is_light() -> bool {
    let mut value: u32 = 0;
    let mut size = size_of::<u32>() as u32;
    let result = unsafe {
        RegGetValueW(
            HKEY_CURRENT_USER,
            w!("Software\\Microsoft\\Windows\\CurrentVersion\\Themes\\Personalize"),
            w!("SystemUsesLightTheme"),
            RRF_RT_REG_DWORD,
            None,
            Some(&mut value as *mut u32 as *mut c_void),
            Some(&mut size),
        )
    };
    result.is_ok() && value != 0
}

/// Two eye capsules, antialiased with a signed distance per pixel.
fn eyes_icon(light_taskbar: bool) -> Option<HICON> {
    let size = unsafe { GetSystemMetrics(SM_CXSMICON) }.max(16);
    let s = size as f32;
    let (eye_w, eye_h) = (0.24 * s, 0.56 * s);
    let radius = eye_w / 2.0;
    let half_segment = (eye_h - eye_w) / 2.0;
    let centers = [(0.31 * s, 0.5 * s), (0.69 * s, 0.5 * s)];
    let shade: u32 = if light_taskbar { 0x1c } else { 0xff };

    let mut pixels = vec![0u32; (size * size) as usize];
    for y in 0..size {
        for x in 0..size {
            let (px, py) = (x as f32 + 0.5, y as f32 + 0.5);
            let coverage = centers
                .iter()
                .map(|&(cx, cy)| {
                    let dy = ((py - cy).abs() - half_segment).max(0.0);
                    let distance = ((px - cx).powi(2) + dy * dy).sqrt() - radius;
                    (0.5 - distance).clamp(0.0, 1.0)
                })
                .fold(0.0f32, f32::max);
            let alpha = (coverage * 255.0).round() as u32;
            // Icons take premultiplied BGRA.
            let c = shade * alpha / 255;
            pixels[(y * size + x) as usize] = (alpha << 24) | (c << 16) | (c << 8) | c;
        }
    }

    unsafe {
        let info = BITMAPINFO {
            bmiHeader: BITMAPINFOHEADER {
                biSize: size_of::<BITMAPINFOHEADER>() as u32,
                biWidth: size,
                biHeight: -size,
                biPlanes: 1,
                biBitCount: 32,
                biCompression: BI_RGB.0,
                ..Default::default()
            },
            ..Default::default()
        };
        let mut bits: *mut c_void = std::ptr::null_mut();
        let color = CreateDIBSection(None, &info, DIB_RGB_COLORS, &mut bits, None, 0).ok()?;
        std::ptr::copy_nonoverlapping(pixels.as_ptr(), bits as *mut u32, pixels.len());
        // The alpha channel does the masking; the mask bitmap only has to exist.
        let mask = CreateBitmap(size, size, 1, 1, None);
        let icon = CreateIconIndirect(&ICONINFO {
            fIcon: true.into(),
            xHotspot: 0,
            yHotspot: 0,
            hbmMask: mask,
            hbmColor: color,
        });
        let _ = DeleteObject(color.into());
        let _ = DeleteObject(mask.into());
        icon.ok()
    }
}
