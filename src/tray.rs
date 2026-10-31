//! Drop's icons, drawn at runtime. In the notification area, a pair of eyes
//! recolored to stay visible on both light and dark taskbars; on windows, the
//! same eyes on a dark tile.

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
    SM_CXICON, SM_CXSMICON, WM_APP,
};

/// Message the shell sends when the icon is clicked.
pub const CALLBACK_MSG: u32 = WM_APP + 1;
const ICON_ID: u32 = 1;

thread_local! {
    static ICON: Cell<Option<HICON>> = const { Cell::new(None) };
    /// The window icons, made once: small and large.
    static WINDOW_ICONS: Cell<Option<(HICON, HICON)>> = const { Cell::new(None) };
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

/// The icons for a window's title bar and taskbar button: small and large.
pub fn window_icons() -> Option<(HICON, HICON)> {
    if let Some(icons) = WINDOW_ICONS.with(Cell::get) {
        return Some(icons);
    }
    let small = tile_icon(unsafe { GetSystemMetrics(SM_CXSMICON) }.max(16))?;
    let large = tile_icon(unsafe { GetSystemMetrics(SM_CXICON) }.max(32))?;
    WINDOW_ICONS.with(|cell| cell.set(Some((small, large))));
    Some((small, large))
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

/// How much two eye capsules cover each pixel, 0..1, antialiased with a signed
/// distance.
fn eye_coverage(size: i32) -> Vec<f32> {
    let s = size as f32;
    let (eye_w, eye_h) = (0.24 * s, 0.56 * s);
    let radius = eye_w / 2.0;
    let half_segment = (eye_h - eye_w) / 2.0;
    let centers = [(0.31 * s, 0.5 * s), (0.69 * s, 0.5 * s)];

    let mut coverage = vec![0.0f32; (size * size) as usize];
    for y in 0..size {
        for x in 0..size {
            let (px, py) = (x as f32 + 0.5, y as f32 + 0.5);
            coverage[(y * size + x) as usize] = centers
                .iter()
                .map(|&(cx, cy)| {
                    let dy = ((py - cy).abs() - half_segment).max(0.0);
                    let distance = ((px - cx).powi(2) + dy * dy).sqrt() - radius;
                    (0.5 - distance).clamp(0.0, 1.0)
                })
                .fold(0.0f32, f32::max);
        }
    }
    coverage
}

/// Premultiplied BGRA, the way icons take it.
fn premultiplied(shade: f32, alpha: f32) -> u32 {
    let a = (alpha * 255.0).round() as u32;
    let c = (shade * alpha).round() as u32;
    (a << 24) | (c << 16) | (c << 8) | c
}

/// The eyes alone, in a shade that shows on the taskbar.
fn eyes_icon(light_taskbar: bool) -> Option<HICON> {
    let size = unsafe { GetSystemMetrics(SM_CXSMICON) }.max(16);
    let shade = if light_taskbar { 28.0 } else { 255.0 };
    let pixels: Vec<u32> = eye_coverage(size)
        .into_iter()
        .map(|eye| premultiplied(shade, eye))
        .collect();
    make_icon(size, &pixels)
}

/// The eyes in white on a dark rounded tile, which shows on any background.
fn tile_icon(size: i32) -> Option<HICON> {
    const TILE_SHADE: f32 = 20.0;
    let s = size as f32;
    let radius = 0.24 * s;
    let eyes = eye_coverage(size);
    let mut pixels = vec![0u32; (size * size) as usize];
    for y in 0..size {
        for x in 0..size {
            let index = (y * size + x) as usize;
            let dx = ((x as f32 + 0.5 - s / 2.0).abs() - (s / 2.0 - radius)).max(0.0);
            let dy = ((y as f32 + 0.5 - s / 2.0).abs() - (s / 2.0 - radius)).max(0.0);
            let tile = (0.5 - ((dx * dx + dy * dy).sqrt() - radius)).clamp(0.0, 1.0);
            let shade = TILE_SHADE + (255.0 - TILE_SHADE) * eyes[index];
            pixels[index] = premultiplied(shade, tile);
        }
    }
    make_icon(size, &pixels)
}

fn make_icon(size: i32, pixels: &[u32]) -> Option<HICON> {
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
