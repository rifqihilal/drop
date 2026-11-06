//! Copied text and pictures, saved as files so they can go on the shelf. Each
//! change is read on a short-lived thread, and what password managers mark
//! as private is left alone.

use std::collections::hash_map::DefaultHasher;
use std::ffi::c_void;
use std::hash::{Hash, Hasher};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU32, Ordering};
use std::time::Duration;

use windows::core::{w, Result, GUID, HSTRING, PCWSTR};
use windows::Win32::Foundation::{GENERIC_WRITE, HWND, LPARAM, WPARAM};
use windows::Win32::Graphics::Imaging::{
    CLSID_WICImagingFactory, GUID_ContainerFormatPng, GUID_WICPixelFormat32bppBGRA,
    IWICImagingFactory, WICBitmapEncoderNoCache,
};
use windows::Win32::System::Com::{
    CoCreateInstance, CoInitializeEx, CLSCTX_INPROC_SERVER, COINIT_MULTITHREADED,
};
use windows::Win32::System::DataExchange::{
    AddClipboardFormatListener, GetClipboardOwner, GetClipboardSequenceNumber,
    IsClipboardFormatAvailable, RegisterClipboardFormatW,
};
use windows::Win32::System::Ole::{CF_DIB, CF_HDROP, CF_UNICODETEXT};
use windows::Win32::UI::WindowsAndMessaging::{PostMessageW, WM_APP};

use crate::{shelf, system};

/// Something copied was saved. LPARAM: a `Box<PathBuf>`.
pub const CLIP_MSG: u32 = WM_APP + 18;
/// Apps announce a copy in several steps, so wait for them to finish.
const SETTLE: Duration = Duration::from_millis(150);
/// Longer texts are left out: they are data, not something to paste back.
const MAX_TEXT: usize = 20_000;
/// Larger pictures are left out to stay light.
const MAX_PIXELS: u64 = 16_000_000;
/// Characters of a text kept in its file's name, which its tile shows.
const NAME_CHARS: usize = 24;

/// Formats that say the clipboard is private.
const PRIVATE: [PCWSTR; 3] = [
    w!("ExcludeClipboardContentFromMonitorProcessing"),
    w!("CanIncludeInClipboardHistory"),
    w!("Clipboard Viewer Ignore"),
];

/// The clipboard change last read.
static LAST_SEEN: AtomicU32 = AtomicU32::new(0);

/// Sends the notch a `WM_CLIPBOARDUPDATE` for every change.
pub fn listen(hwnd: HWND) {
    unsafe {
        let _ = AddClipboardFormatListener(hwnd);
    }
}

/// Saves what was just copied, unless the notch itself copied it.
pub fn capture(hwnd: HWND) {
    let target = hwnd.0 as isize;
    std::thread::spawn(move || {
        std::thread::sleep(SETTLE);
        let window = HWND(target as *mut c_void);
        unsafe {
            let change = GetClipboardSequenceNumber();
            if LAST_SEEN.swap(change, Ordering::Relaxed) == change
                || GetClipboardOwner().is_ok_and(|owner| owner == window)
            {
                return;
            }
        }
        if let Some(path) = save_clipboard() {
            let payload = Box::into_raw(Box::new(path));
            unsafe {
                let _ = PostMessageW(Some(window), CLIP_MSG, WPARAM(0), LPARAM(payload as isize));
            }
        }
    });
}

fn available(format: u32) -> bool {
    unsafe { IsClipboardFormatAvailable(format).is_ok() }
}

fn is_private() -> bool {
    PRIVATE
        .iter()
        .any(|&name| available(unsafe { RegisterClipboardFormatW(name) }))
}

/// Text goes first: apps copying text often add a picture of it too. Files
/// copied in Explorer are not taken, as they are on disk already.
fn save_clipboard() -> Option<PathBuf> {
    if is_private() || available(u32::from(CF_HDROP.0)) {
        return None;
    }
    let dir = shelf::clips_dir()?;
    std::fs::create_dir_all(&dir).ok()?;
    if available(u32::from(CF_UNICODETEXT.0)) {
        let text = system::clipboard_text()?;
        return save_text(&dir, &text);
    }
    if available(u32::from(CF_DIB.0)) {
        let dib = system::clipboard_picture()?;
        return save_picture(&dir, &dib);
    }
    None
}

fn save_text(dir: &Path, text: &str) -> Option<PathBuf> {
    if text.trim().is_empty() || text.chars().count() > MAX_TEXT {
        return None;
    }
    let path = dir.join(text_file_name(text));
    std::fs::write(&path, text).ok()?;
    Some(path)
}

/// The same text gets the same name, so copying it again only moves its tile.
fn text_file_name(text: &str) -> String {
    let safe: String = text
        .chars()
        .filter(|c| c.is_alphanumeric() || c.is_whitespace() || ".,-_()".contains(*c))
        .collect();
    let preview: String = safe
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
        .chars()
        .take(NAME_CHARS)
        .collect();
    let preview = preview.trim_end_matches(['.', ' ']);
    let preview = if preview.is_empty() {
        crate::lang::tr("Teks", "Text")
    } else {
        preview
    };
    format!("{:08x} {preview}.txt", hash(text.as_bytes()))
}

fn hash(bytes: &[u8]) -> u32 {
    let mut hasher = DefaultHasher::new();
    bytes.hash(&mut hasher);
    hasher.finish() as u32
}

fn save_picture(dir: &Path, dib: &[u8]) -> Option<PathBuf> {
    let (width, height, pixels) = dib_pixels(dib)?;
    let path = dir.join(format!(
        "{:08x} {}.png",
        hash(dib),
        crate::lang::tr("Gambar", "Image")
    ));
    unsafe { write_png(&path, width, height, &pixels) }.ok()?;
    Some(path)
}

fn number(bytes: &[u8], at: usize) -> Option<u32> {
    Some(u32::from_le_bytes(bytes.get(at..at + 4)?.try_into().ok()?))
}

/// A device-independent bitmap's size and top-down BGRA pixels. Takes the 24
/// and 32 bit kinds, which is what apps put on the clipboard.
fn dib_pixels(dib: &[u8]) -> Option<(u32, u32, Vec<u8>)> {
    const RGB: u32 = 0;
    const BIT_FIELDS: u32 = 3;
    let header = number(dib, 0)? as usize;
    let width = number(dib, 4)? as i32;
    let height = number(dib, 8)? as i32;
    let bits = u16::from_le_bytes(dib.get(14..16)?.try_into().ok()?);
    let compression = number(dib, 16)?;
    let (width_px, height_px) = (width.unsigned_abs(), height.unsigned_abs());
    if !matches!(bits, 24 | 32)
        || !matches!(compression, RGB | BIT_FIELDS)
        || width <= 0
        || height_px == 0
        || u64::from(width_px) * u64::from(height_px) > MAX_PIXELS
    {
        return None;
    }
    let masks = if compression == BIT_FIELDS && header == 40 {
        12
    } else {
        0
    };
    let stride = (width_px as usize * bits as usize).div_ceil(32) * 4;
    let body = dib.get(header + masks..)?;
    if body.len() < stride * height_px as usize {
        return None;
    }
    let step = bits as usize / 8;
    let mut pixels = Vec::with_capacity(width_px as usize * height_px as usize * 4);
    let mut solid = false;
    for row in 0..height_px as usize {
        let row = if height > 0 {
            height_px as usize - 1 - row
        } else {
            row
        };
        let line = &body[row * stride..][..width_px as usize * step];
        for pixel in line.chunks_exact(step) {
            let alpha = pixel.get(3).copied().unwrap_or(255);
            solid |= alpha != 0;
            pixels.extend_from_slice(&[pixel[0], pixel[1], pixel[2], alpha]);
        }
    }
    // Many apps leave the fourth byte unused, as zero.
    if !solid {
        pixels
            .iter_mut()
            .skip(3)
            .step_by(4)
            .for_each(|alpha| *alpha = 255);
    }
    Some((width_px, height_px, pixels))
}

unsafe fn write_png(path: &Path, width: u32, height: u32, pixels: &[u8]) -> Result<()> {
    let _ = CoInitializeEx(None, COINIT_MULTITHREADED);
    let wic: IWICImagingFactory =
        CoCreateInstance(&CLSID_WICImagingFactory, None, CLSCTX_INPROC_SERVER)?;
    let stream = wic.CreateStream()?;
    stream.InitializeFromFilename(&HSTRING::from(path.as_os_str()), GENERIC_WRITE.0)?;
    let encoder = wic.CreateEncoder(&GUID_ContainerFormatPng, std::ptr::null::<GUID>())?;
    encoder.Initialize(&stream, WICBitmapEncoderNoCache)?;
    let mut frame = None;
    encoder.CreateNewFrame(&mut frame, std::ptr::null_mut())?;
    let frame = frame.ok_or(windows::core::Error::empty())?;
    frame.Initialize(None)?;
    frame.SetSize(width, height)?;
    let mut format = GUID_WICPixelFormat32bppBGRA;
    frame.SetPixelFormat(&mut format)?;
    frame.WritePixels(height, width * 4, pixels)?;
    frame.Commit()?;
    encoder.Commit()
}

/// Takes ownership of the path carried by a [`CLIP_MSG`].
///
/// # Safety
/// `lparam` must come from a [`CLIP_MSG`] and be taken only once.
pub unsafe fn take_path(lparam: LPARAM) -> PathBuf {
    *Box::from_raw(lparam.0 as *mut PathBuf)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A bottom-up 2 by 2 bitmap, one row of each colour.
    fn dib(bits: u16, alpha: u8) -> Vec<u8> {
        let mut bytes = vec![0u8; 40];
        bytes[0] = 40;
        bytes[4] = 2;
        bytes[8] = 2;
        bytes[14..16].copy_from_slice(&bits.to_le_bytes());
        for colour in [[1, 2, 3], [4, 5, 6]] {
            let mut row = Vec::new();
            for _ in 0..2 {
                row.extend_from_slice(&colour);
                if bits == 32 {
                    row.push(alpha);
                }
            }
            row.resize(8, 0);
            bytes.extend_from_slice(&row);
        }
        bytes
    }

    #[test]
    fn reads_bitmaps_top_down() {
        let (width, height, pixels) = dib_pixels(&dib(32, 200)).unwrap();
        assert_eq!((width, height), (2, 2));
        assert_eq!(&pixels[..4], [4, 5, 6, 200]);
        assert_eq!(&pixels[12..], [1, 2, 3, 200]);
    }

    #[test]
    fn unused_alpha_turns_opaque() {
        let (.., pixels) = dib_pixels(&dib(32, 0)).unwrap();
        assert_eq!(pixels[3], 255);
        let (.., pixels) = dib_pixels(&dib(24, 0)).unwrap();
        assert_eq!(pixels[3], 255);
    }

    #[test]
    fn refuses_what_it_cannot_read() {
        assert!(dib_pixels(&dib(8, 0)).is_none());
        assert!(dib_pixels(&dib(32, 0)[..50]).is_none());
        assert!(dib_pixels(&[]).is_none());
    }

    #[test]
    fn names_keep_the_start_of_the_text() {
        let name = text_file_name("Halo  dunia\n: baris kedua yang panjang sekali");
        assert_eq!(&name[8..], " Halo dunia baris kedua y.txt");
        assert!(text_file_name("\\/:*?").ends_with(" Teks.txt"));
        assert_eq!(text_file_name("sama"), text_file_name("sama"));
    }
}
