//! Drop's settings file, `%APPDATA%\Drop\drop.ini`. The settings window
//! writes it, and it stays readable and editable by hand.

use std::path::PathBuf;
use std::time::SystemTime;

use crate::prayer::Location;

#[derive(Clone, PartialEq, Debug)]
pub struct Settings {
    /// Place name shown with prayer times.
    pub city: String,
    pub latitude: Option<f64>,
    pub longitude: Option<f64>,
    /// Metres above sea level.
    pub elevation: f64,
    /// Minutes before each prayer to remind.
    pub remind_minutes: u32,
    pub notify_wifi: bool,
    pub notify_bluetooth: bool,
    pub notify_battery: bool,
    pub notify_prayer: bool,
    /// Play the notification sound with notices.
    pub sound: bool,
    /// Minutes without input before the eyes appear.
    pub idle_minutes: u32,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            city: String::new(),
            latitude: None,
            longitude: None,
            elevation: 0.0,
            remind_minutes: 5,
            notify_wifi: true,
            notify_bluetooth: true,
            notify_battery: true,
            notify_prayer: true,
            sound: true,
            idle_minutes: 2,
        }
    }
}

/// Where prayer times are computed for.
#[derive(Clone, PartialEq, Debug)]
pub struct PrayerSettings {
    pub city: String,
    pub location: Location,
    pub remind_minutes: u32,
}

impl Settings {
    /// None until both coordinates are filled in.
    pub fn prayer(&self) -> Option<PrayerSettings> {
        Some(PrayerSettings {
            city: self.city.clone(),
            location: Location {
                latitude: self.latitude?,
                longitude: self.longitude?,
                elevation: self.elevation,
            },
            remind_minutes: self.remind_minutes,
        })
    }

    pub fn load() -> Self {
        path()
            .and_then(|path| std::fs::read_to_string(path).ok())
            .map(|text| parse(&text))
            .unwrap_or_default()
    }

    pub fn save(&self) -> std::io::Result<()> {
        let path = path().ok_or(std::io::ErrorKind::NotFound)?;
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir)?;
        }
        std::fs::write(path, self.to_ini())
    }

    fn to_ini(&self) -> String {
        let number = |value: Option<f64>| value.map(|v| v.to_string()).unwrap_or_default();
        let flag = |on: bool| if on { "ya" } else { "tidak" };
        format!(
            "; Pengaturan Drop. Ubah lewat menu tray > Pengaturan, atau langsung di sini;\n\
             ; Drop membaca ulang file ini dalam satu menit.\n\
             \n\
             [sholat]\n\
             ; Koordinat dalam derajat desimal; lintang selatan bernilai negatif.\n\
             kota = {}\n\
             lintang = {}\n\
             bujur = {}\n\
             ; Ketinggian dalam meter di atas permukaan laut.\n\
             ketinggian = {}\n\
             ; Berapa menit sebelum waktu sholat peringatan muncul.\n\
             pengingat = {}\n\
             \n\
             [notifikasi]\n\
             wifi = {}\n\
             bluetooth = {}\n\
             baterai = {}\n\
             sholat = {}\n\
             bunyi = {}\n\
             \n\
             [umum]\n\
             ; Menit tanpa sentuhan keyboard atau mouse sebelum mata muncul.\n\
             mata_setelah_menit = {}\n",
            self.city,
            number(self.latitude),
            number(self.longitude),
            self.elevation,
            self.remind_minutes,
            flag(self.notify_wifi),
            flag(self.notify_bluetooth),
            flag(self.notify_battery),
            flag(self.notify_prayer),
            flag(self.sound),
            self.idle_minutes,
        )
    }
}

pub fn path() -> Option<PathBuf> {
    let appdata = std::env::var_os("APPDATA")?;
    Some(PathBuf::from(appdata).join("Drop").join("drop.ini"))
}

/// Writes the default settings on first run, so the file is there to edit.
pub fn ensure_exists() {
    if path().is_some_and(|path| !path.exists()) {
        let _ = Settings::default().save();
    }
}

pub fn modified() -> Option<SystemTime> {
    std::fs::metadata(path()?).ok()?.modified().ok()
}

/// Reads a coordinate or other number typed by hand, accepting a decimal comma.
pub fn parse_number(text: &str) -> Option<f64> {
    text.trim().replace(',', ".").parse().ok()
}

pub fn valid_latitude(value: f64) -> bool {
    value.abs() <= 90.0
}

pub fn valid_longitude(value: f64) -> bool {
    value.abs() <= 180.0
}

fn parse(text: &str) -> Settings {
    let mut settings = Settings::default();
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
        let number = || parse_number(value);
        let flag = |default: bool| match value.to_lowercase().as_str() {
            "ya" | "true" | "1" | "on" => true,
            "tidak" | "false" | "0" | "off" => false,
            _ => default,
        };
        let s = &mut settings;
        match key.trim().to_lowercase().as_str() {
            "kota" => s.city = value.to_string(),
            "lintang" => s.latitude = number().filter(|&v| valid_latitude(v)),
            "bujur" => s.longitude = number().filter(|&v| valid_longitude(v)),
            "ketinggian" => s.elevation = number().unwrap_or(0.0).max(0.0),
            "pengingat" => s.remind_minutes = number().map_or(5, |v| v.clamp(1.0, 60.0) as u32),
            "wifi" => s.notify_wifi = flag(true),
            "bluetooth" => s.notify_bluetooth = flag(true),
            "baterai" => s.notify_battery = flag(true),
            "sholat" => s.notify_prayer = flag(true),
            "bunyi" => s.sound = flag(true),
            "mata_setelah_menit" => {
                s.idle_minutes = number().map_or(2, |v| v.clamp(1.0, 120.0) as u32)
            }
            _ => {}
        }
    }
    settings
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn defaults_have_no_location() {
        let settings = parse(&Settings::default().to_ini());
        assert_eq!(settings, Settings::default());
        assert_eq!(settings.prayer(), None);
    }

    #[test]
    fn saved_settings_read_back() {
        let settings = Settings {
            city: "Aimas".to_string(),
            latitude: Some(-0.95698),
            longitude: Some(131.33534),
            elevation: 9.0,
            remind_minutes: 10,
            notify_wifi: false,
            notify_bluetooth: true,
            notify_battery: false,
            notify_prayer: true,
            sound: false,
            idle_minutes: 5,
        };
        assert_eq!(parse(&settings.to_ini()), settings);
    }

    #[test]
    fn accepts_hand_edits() {
        let text = "\u{feff}[sholat]\nkota = Aimas\nlintang = -0,95698\nbujur = 131.33534\nbunyi = tidak\n";
        let settings = parse(text);
        assert_eq!(settings.latitude, Some(-0.95698));
        assert!(!settings.sound);
        assert_eq!(settings.prayer().unwrap().location.longitude, 131.33534);
    }
}
