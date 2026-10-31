//! Drop's settings file, `%APPDATA%\Drop\drop.ini`. The settings window
//! writes it, and it stays readable and editable by hand.

use std::path::PathBuf;
use std::time::SystemTime;

use crate::health;
use crate::prayer::Location;

/// How many calendar links the agenda takes.
pub const FEEDS: usize = 3;

/// How see-through the notch can get at most.
const MIN_OPACITY: f32 = 0.45;

/// How large the closed notch is; open cards keep one size.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub enum NotchSize {
    Small,
    #[default]
    Normal,
    Large,
}

impl NotchSize {
    fn name(self) -> &'static str {
        match self {
            Self::Small => "kecil",
            Self::Normal => "normal",
            Self::Large => "besar",
        }
    }
}

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
    /// Show toasts from other apps in the notch.
    pub notify_apps: bool,
    /// Put new screenshots on the shelf.
    pub shelf_screenshots: bool,
    /// Put copied text and pictures on the shelf.
    pub shelf_clipboard: bool,
    /// Play the notification sound with notices.
    pub sound: bool,
    /// Minutes without input before the eyes appear.
    pub idle_minutes: u32,
    /// Fetch the weather for the prayer location.
    pub weather: bool,
    /// Modules: each adds a page (and, for some, the live strip).
    pub module_music: bool,
    pub module_calendar: bool,
    pub module_shelf: bool,
    pub module_prayer: bool,
    pub module_eyes: bool,
    pub module_todo: bool,
    pub module_usage: bool,
    /// Screen Time shows the last seven days instead of today.
    pub usage_week: bool,
    pub module_lyrics: bool,
    /// Rest as a thin bar, with no clock or live strip.
    pub notch_minimal: bool,
    /// Hovering only widens the notch; a click opens the card.
    pub open_on_click: bool,
    /// The card and the widened notch bounce as they open.
    pub card_bounce: bool,
    /// The open card shows arrows to the pages on either side.
    pub card_arrows: bool,
    pub notch_size: NotchSize,
    /// How solid the notch's black is, 0..100; see [`Settings::notch_opacity`].
    pub notch_fill: u32,
    /// The notch floats below the screen's top edge instead of joining it.
    pub notch_floating: bool,
    /// Links to iCalendar feeds for the agenda; each slot has its colour,
    /// and an empty one is unused.
    pub ical_urls: [String; FEEDS],
    /// Health reminders, each after its minutes of use.
    pub water: bool,
    pub water_minutes: u32,
    pub eyes: bool,
    pub eyes_minutes: u32,
    pub stretch: bool,
    pub stretch_minutes: u32,
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
            notify_apps: true,
            shelf_screenshots: true,
            shelf_clipboard: true,
            sound: true,
            idle_minutes: 2,
            weather: true,
            module_music: true,
            module_calendar: true,
            module_shelf: true,
            module_prayer: true,
            module_eyes: true,
            module_todo: true,
            module_usage: true,
            usage_week: false,
            module_lyrics: true,
            notch_minimal: false,
            open_on_click: false,
            card_bounce: true,
            card_arrows: true,
            notch_size: NotchSize::Normal,
            notch_fill: 100,
            notch_floating: false,
            ical_urls: Default::default(),
            water: true,
            water_minutes: 60,
            eyes: false,
            eyes_minutes: 20,
            stretch: true,
            stretch_minutes: 50,
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

    pub fn health(&self) -> health::Plan {
        health::Plan {
            water: self.water.then_some(self.water_minutes),
            eyes: self.eyes.then_some(self.eyes_minutes),
            stretch: self.stretch.then_some(self.stretch_minutes),
        }
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
             ; Notifikasi aplikasi lain, seperti WhatsApp.\n\
             aplikasi = {}\n\
             bunyi = {}\n\
             \n\
             [umum]\n\
             ; Menit tanpa sentuhan keyboard atau mouse sebelum mata muncul.\n\
             mata_setelah_menit = {}\n\
             \n\
             [modul]\n\
             ; Cuaca diambil dari Open-Meteo untuk lokasi di atas.\n\
             cuaca = {}\n\
             musik = {}\n\
             kalender = {}\n\
             shelf = {}\n\
             ; Screenshot baru langsung masuk shelf.\n\
             screenshot = {}\n\
             ; Teks dan gambar yang dicopy masuk shelf sampai Drop ditutup.\n\
             riwayat_copy = {}\n\
             halaman_sholat = {}\n\
             mata = {}\n\
             tugas = {}\n\
             ; Waktu pakai per app hari ini, cuma disimpan di PC ini.\n\
             screen_time = {}\n\
             ; Screen Time menampilkan 7 hari terakhir, bukan hari ini.\n\
             screen_time_7_hari = {}\n\
             ; Lirik dicari di lrclib.net dari judul dan penyanyi lagu.\n\
             lirik = {}\n\
             ; Saat tertutup notch hanya bar hitam tipis.\n\
             notch_kecil = {}\n\
             ; Hover hanya melebarkan notch, kartu terbuka saat diklik.\n\
             buka_klik = {}\n\
             ; Notch dan kartu memantul saat melebar.\n\
             pantulan = {}\n\
             ; Panah kiri dan kanan untuk ganti halaman kartu.\n\
             panah_halaman = {}\n\
             ; Ukuran notch: kecil, normal atau besar.\n\
             ukuran_notch = {}\n\
             ; Transparansi notch: 100 hitam penuh, 0 paling tembus tapi tetap terlihat.\n\
             transparansi = {}\n\
             ; Notch mengambang di bawah tepi atas layar, tidak menempel.\n\
             notch_mengambang = {}\n\
             \n\
             [agenda]\n\
             ; Tautan kalender .ics (Google Calendar: Setelan > Alamat rahasia\n\
             ; dalam format iCal). Kosongkan untuk mematikan agenda.\n\
             ical = {}\n\
             ical2 = {}\n\
             ical3 = {}\n\
             \n\
             [kesehatan]\n\
             ; Pengingat setelah sekian menit memakai komputer; menjauh dari\n\
             ; komputer {} menit atau lebih memulai hitungan dari awal.\n\
             minum_air = {}\n\
             minum_air_menit = {}\n\
             istirahat_mata = {}\n\
             istirahat_mata_menit = {}\n\
             peregangan = {}\n\
             peregangan_menit = {}\n",
            self.city,
            number(self.latitude),
            number(self.longitude),
            self.elevation,
            self.remind_minutes,
            flag(self.notify_wifi),
            flag(self.notify_bluetooth),
            flag(self.notify_battery),
            flag(self.notify_prayer),
            flag(self.notify_apps),
            flag(self.sound),
            self.idle_minutes,
            flag(self.weather),
            flag(self.module_music),
            flag(self.module_calendar),
            flag(self.module_shelf),
            flag(self.shelf_screenshots),
            flag(self.shelf_clipboard),
            flag(self.module_prayer),
            flag(self.module_eyes),
            flag(self.module_todo),
            flag(self.module_usage),
            flag(self.usage_week),
            flag(self.module_lyrics),
            flag(self.notch_minimal),
            flag(self.open_on_click),
            flag(self.card_bounce),
            flag(self.card_arrows),
            self.notch_size.name(),
            self.notch_fill,
            flag(self.notch_floating),
            self.ical_urls[0],
            self.ical_urls[1],
            self.ical_urls[2],
            health::BREAK_MINUTES,
            flag(self.water),
            self.water_minutes,
            flag(self.eyes),
            self.eyes_minutes,
            flag(self.stretch),
            self.stretch_minutes,
        )
    }
}

impl Settings {
    /// The notch's opacity: never below [`MIN_OPACITY`], so it stays visible.
    pub fn notch_opacity(&self) -> f32 {
        MIN_OPACITY + (1.0 - MIN_OPACITY) * self.notch_fill.min(100) as f32 / 100.0
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
        let minutes = |default: f64| number().unwrap_or(default).clamp(5.0, 240.0) as u32;
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
            "aplikasi" => s.notify_apps = flag(true),
            "bunyi" => s.sound = flag(true),
            "mata_setelah_menit" => {
                s.idle_minutes = number().map_or(2, |v| v.clamp(1.0, 120.0) as u32)
            }
            "cuaca" => s.weather = flag(true),
            "musik" => s.module_music = flag(true),
            "kalender" => s.module_calendar = flag(true),
            "shelf" => s.module_shelf = flag(true),
            "screenshot" => s.shelf_screenshots = flag(true),
            "riwayat_copy" => s.shelf_clipboard = flag(true),
            "halaman_sholat" => s.module_prayer = flag(true),
            "mata" => s.module_eyes = flag(true),
            "tugas" => s.module_todo = flag(true),
            "screen_time" => s.module_usage = flag(true),
            "screen_time_7_hari" => s.usage_week = flag(false),
            "lirik" => s.module_lyrics = flag(true),
            "notch_kecil" => s.notch_minimal = flag(false),
            "buka_klik" => s.open_on_click = flag(false),
            "pantulan" => s.card_bounce = flag(true),
            "panah_halaman" => s.card_arrows = flag(true),
            "ukuran_notch" => {
                s.notch_size = match value.to_lowercase().as_str() {
                    "kecil" => NotchSize::Small,
                    "besar" => NotchSize::Large,
                    _ => NotchSize::Normal,
                }
            }
            "transparansi" => s.notch_fill = number().map_or(100, |v| v.clamp(0.0, 100.0) as u32),
            "notch_mengambang" => s.notch_floating = flag(false),
            "ical" => s.ical_urls[0] = value.to_string(),
            "ical2" => s.ical_urls[1] = value.to_string(),
            "ical3" => s.ical_urls[2] = value.to_string(),
            "minum_air" => s.water = flag(true),
            "minum_air_menit" => s.water_minutes = minutes(60.0),
            "istirahat_mata" => s.eyes = flag(false),
            "istirahat_mata_menit" => s.eyes_minutes = minutes(20.0),
            "peregangan" => s.stretch = flag(true),
            "peregangan_menit" => s.stretch_minutes = minutes(50.0),
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
            notify_apps: false,
            shelf_screenshots: false,
            shelf_clipboard: false,
            sound: false,
            idle_minutes: 5,
            weather: false,
            module_music: false,
            module_calendar: true,
            module_shelf: false,
            module_prayer: true,
            module_eyes: false,
            module_todo: false,
            module_usage: false,
            usage_week: true,
            module_lyrics: false,
            notch_minimal: true,
            open_on_click: true,
            card_bounce: false,
            card_arrows: false,
            notch_size: NotchSize::Large,
            notch_fill: 40,
            notch_floating: true,
            ical_urls: [
                "https://calendar.google.com/calendar/ical/x%40gmail.com/private-ab=cd/basic.ics"
                    .to_string(),
                String::new(),
                "https://example.com/kerja.ics".to_string(),
            ],
            water: false,
            water_minutes: 45,
            eyes: true,
            eyes_minutes: 25,
            stretch: true,
            stretch_minutes: 90,
        };
        assert_eq!(parse(&settings.to_ini()), settings);
    }

    #[test]
    fn the_notch_never_turns_invisible() {
        let at = |notch_fill| {
            Settings {
                notch_fill,
                ..Settings::default()
            }
            .notch_opacity()
        };
        assert_eq!(at(100), 1.0);
        assert_eq!(at(0), MIN_OPACITY);
        assert_eq!(parse("transparansi = 500\n").notch_fill, 100);
        assert_eq!(Settings::default().notch_fill, 100);
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
