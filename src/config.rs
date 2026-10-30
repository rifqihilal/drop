//! Drop's settings file, `%APPDATA%\Drop\drop.ini`, edited by hand.

use std::path::PathBuf;
use std::time::SystemTime;

use windows::core::{w, HSTRING};
use windows::Win32::UI::Shell::ShellExecuteW;
use windows::Win32::UI::WindowsAndMessaging::SW_SHOWNORMAL;

use crate::prayer::Location;

const TEMPLATE: &str = "\
; Pengaturan Drop
; Simpan file ini, lalu Drop membacanya ulang dalam satu menit.

[sholat]
; Nama kota yang ditampilkan.
kota =
; Koordinat lokasi Anda dalam derajat desimal; lintang selatan bernilai negatif.
; Di Google Maps: klik kanan lokasi, angka pertama lintang, angka kedua bujur.
; Contoh Jakarta: lintang = -6.1754 dan bujur = 106.8272
lintang =
bujur =
; Ketinggian dalam meter di atas permukaan laut (boleh dikosongkan).
ketinggian =
; Berapa menit sebelum waktu sholat peringatan muncul.
pengingat = 5
";

#[derive(Clone, PartialEq, Debug)]
pub struct PrayerSettings {
    pub city: String,
    pub location: Location,
    /// Minutes before each prayer to remind.
    pub remind_minutes: u32,
}

pub fn path() -> Option<PathBuf> {
    let appdata = std::env::var_os("APPDATA")?;
    Some(PathBuf::from(appdata).join("Drop").join("drop.ini"))
}

/// Writes the commented template on first run.
pub fn ensure_exists() {
    let Some(path) = path() else {
        return;
    };
    if path.exists() {
        return;
    }
    if let Some(dir) = path.parent() {
        let _ = std::fs::create_dir_all(dir);
    }
    let _ = std::fs::write(&path, TEMPLATE);
}

pub fn modified() -> Option<SystemTime> {
    std::fs::metadata(path()?).ok()?.modified().ok()
}

/// Prayer settings, or None until a location has been filled in.
pub fn load_prayer() -> Option<PrayerSettings> {
    parse_prayer(&std::fs::read_to_string(path()?).ok()?)
}

fn parse_prayer(text: &str) -> Option<PrayerSettings> {
    let mut city = String::new();
    let (mut latitude, mut longitude, mut elevation) = (None, None, 0.0);
    let mut remind_minutes = 5;
    // Notepad may save with a byte-order mark.
    for line in text.trim_start_matches('\u{feff}').lines() {
        let line = line.trim();
        if line.starts_with([';', '#', '[']) {
            continue;
        }
        let Some((key, value)) = line.split_once('=') else {
            continue;
        };
        let value = value.trim();
        // Accept a decimal comma, as typed with Indonesian number formats.
        let number = || value.replace(',', ".").parse::<f64>().ok();
        match key.trim().to_lowercase().as_str() {
            "kota" => city = value.to_string(),
            "lintang" => latitude = number().filter(|v| v.abs() <= 90.0),
            "bujur" => longitude = number().filter(|v| v.abs() <= 180.0),
            "ketinggian" => elevation = number().unwrap_or(0.0),
            "pengingat" => {
                remind_minutes = number().map_or(5, |v| v.clamp(1.0, 60.0) as u32);
            }
            _ => {}
        }
    }
    Some(PrayerSettings {
        city,
        location: Location {
            latitude: latitude?,
            longitude: longitude?,
            elevation,
        },
        remind_minutes,
    })
}

/// Opens the settings file in the default editor (Notepad).
pub fn open_in_editor() {
    ensure_exists();
    let Some(path) = path() else {
        return;
    };
    unsafe {
        ShellExecuteW(
            None,
            w!("open"),
            &HSTRING::from(path.as_os_str()),
            None,
            None,
            SW_SHOWNORMAL,
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn template_has_no_location_yet() {
        assert_eq!(parse_prayer(TEMPLATE), None);
    }

    #[test]
    fn reads_a_filled_in_file() {
        let text = TEMPLATE
            .replace("kota =\n", "kota = Jakarta\n")
            .replace("lintang =\n", "lintang = -6,1754\n")
            .replace("bujur =\n", "bujur = 106.8272\n")
            .replace("pengingat = 5", "pengingat = 10");
        let settings = parse_prayer(&text).expect("location set");
        assert_eq!(settings.city, "Jakarta");
        assert_eq!(settings.location.latitude, -6.1754);
        assert_eq!(settings.location.longitude, 106.8272);
        assert_eq!(settings.location.elevation, 0.0);
        assert_eq!(settings.remind_minutes, 10);
    }
}
