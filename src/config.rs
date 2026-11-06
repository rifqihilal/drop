//! Drop's settings file, `%APPDATA%\Drop\drop.ini`. The settings window
//! writes it, and it stays readable and editable by hand.

use std::path::PathBuf;
use std::time::SystemTime;

use crate::health;
use crate::lang::{self, Lang};
use crate::prayer::Location;
use crate::render::Page;

/// How many calendar links the agenda takes.
pub const FEEDS: usize = 3;

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

/// When the notch steps aside for the app in front.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub enum AutoHide {
    Never,
    /// A game, a full-screen video or a presentation.
    #[default]
    Fullscreen,
    /// Those, and any maximized window.
    Maximized,
}

impl AutoHide {
    fn name(self) -> &'static str {
        match self {
            Self::Never => "tidak",
            Self::Fullscreen => "fullscreen",
            Self::Maximized => "maximize",
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
    /// Say when charging reaches 80%.
    pub charge_limit: bool,
    /// Say when the microphone is muted or unmuted.
    pub notify_mic: bool,
    pub notify_prayer: bool,
    /// Show toasts from other apps in the notch.
    pub notify_apps: bool,
    /// Put new screenshots on the shelf.
    pub shelf_screenshots: bool,
    /// Put copied text and pictures on the shelf.
    pub shelf_clipboard: bool,
    /// Stay quiet while Windows' Do Not Disturb is on.
    pub follow_dnd: bool,
    /// Play the notification sound with notices.
    pub sound: bool,
    /// A small notch beside the main one while the camera or microphone is in use.
    pub privacy_dots: bool,
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
    /// The calculator's page on the card.
    pub module_calc: bool,
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
    /// The notch floats below the screen's top edge instead of joining it.
    pub notch_floating: bool,
    pub auto_hide: AutoHide,
    /// Hours from UTC of a second clock in the card's header, such as 7 for
    /// WIB; None for none.
    pub second_zone: Option<f64>,
    /// Links to iCalendar feeds for the agenda; each slot has its colour,
    /// and an empty one is unused.
    pub ical_urls: [String; FEEDS],
    /// The interface language.
    pub language: Lang,
    /// Every card page, in the order the card shows them.
    pub page_order: Vec<Page>,
    /// Pages kept off the card. Home is never among them.
    pub pages_hidden: Vec<Page>,
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
            charge_limit: false,
            notify_mic: true,
            notify_prayer: true,
            notify_apps: true,
            shelf_screenshots: true,
            shelf_clipboard: true,
            follow_dnd: true,
            sound: true,
            privacy_dots: true,
            idle_minutes: 2,
            weather: true,
            module_music: true,
            module_calendar: true,
            module_shelf: true,
            module_prayer: true,
            module_eyes: true,
            module_todo: true,
            module_calc: true,
            module_usage: true,
            usage_week: false,
            module_lyrics: true,
            notch_minimal: false,
            open_on_click: false,
            card_bounce: true,
            card_arrows: true,
            notch_size: NotchSize::Normal,
            notch_floating: false,
            auto_hide: AutoHide::Fullscreen,
            second_zone: None,
            ical_urls: Default::default(),
            language: Lang::system(),
            page_order: Page::ALL.to_vec(),
            pages_hidden: Vec::new(),
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

    /// Reads the settings file, and puts its language into effect.
    pub fn load() -> Self {
        let settings: Self = path()
            .and_then(|path| std::fs::read_to_string(path).ok())
            .map(|text| parse(&text))
            .unwrap_or_default();
        lang::set(settings.language);
        settings
    }

    pub fn save(&self) -> std::io::Result<()> {
        let path = path().ok_or(std::io::ErrorKind::NotFound)?;
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir)?;
        }
        let text = match self.language {
            Lang::English => english_comments(&self.to_ini()),
            Lang::Indonesian => self.to_ini(),
        };
        std::fs::write(path, text)
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
             ; Ingatkan cabut charger saat baterai sampai 80%.\n\
             baterai_80 = {}\n\
             ; Notifikasi saat mikrofon di-mute atau dinyalakan.\n\
             mic = {}\n\
             sholat = {}\n\
             ; Notifikasi aplikasi lain, seperti WhatsApp.\n\
             aplikasi = {}\n\
             ; Notifikasi aplikasi dan bunyi diam saat Do Not Disturb Windows aktif.\n\
             ikuti_dnd = {}\n\
             bunyi = {}\n\
             \n\
             [umum]\n\
             ; Bahasa tampilan: id (Indonesia) atau en (English).\n\
             bahasa = {}\n\
             ; Notch kecil di kanan saat kamera atau mikrofon dipakai.\n\
             titik_privasi = {}\n\
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
             kalkulator = {}\n\
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
             ; Notch mengambang di bawah tepi atas layar, tidak menempel.\n\
             notch_mengambang = {}\n\
             ; Sembunyikan notch saat app di depan fullscreen, juga saat di-maximize, atau tidak.\n\
             sembunyi_otomatis = {}\n\
             ; Jam daerah lain di Home: selisih jam dari UTC (7 WIB, 8 WITA, 9 WIT); kosong = mati.\n\
             zona_kedua = {}\n\
             \n\
             [agenda]\n\
             ; Tautan kalender .ics (Google Calendar: Setelan > Alamat rahasia\n\
             ; dalam format iCal). Kosongkan untuk mematikan agenda.\n\
             ical = {}\n\
             ical2 = {}\n\
             ical3 = {}\n\
             \n\
             [halaman]\n\
             ; Urutan halaman kartu, dipisah koma: musik, home, notifikasi, sholat, kalender,\n\
             ; pomodoro, tugas, kalkulator, screen_time, shelf. Yang tidak disebut ditaruh di akhir.\n\
             urutan_halaman = {}\n\
             ; Halaman yang tidak tampil di kartu, dipisah koma. Home selalu tampil.\n\
             halaman_sembunyi = {}\n\
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
            flag(self.charge_limit),
            flag(self.notify_mic),
            flag(self.notify_prayer),
            flag(self.notify_apps),
            flag(self.follow_dnd),
            flag(self.sound),
            self.language.key(),
            flag(self.privacy_dots),
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
            flag(self.module_calc),
            flag(self.module_usage),
            flag(self.usage_week),
            flag(self.module_lyrics),
            flag(self.notch_minimal),
            flag(self.open_on_click),
            flag(self.card_bounce),
            flag(self.card_arrows),
            self.notch_size.name(),
            flag(self.notch_floating),
            self.auto_hide.name(),
            number(self.second_zone),
            self.ical_urls[0],
            self.ical_urls[1],
            self.ical_urls[2],
            page_keys(&self.page_order),
            page_keys(&self.pages_hidden),
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

fn page_keys(pages: &[Page]) -> String {
    pages
        .iter()
        .map(|page| page.key())
        .collect::<Vec<_>>()
        .join(", ")
}

/// Pages named in a comma-separated list, each once, unknown names skipped.
fn pages_named(text: &str) -> Vec<Page> {
    let mut pages = Vec::new();
    for page in text.split(',').filter_map(Page::from_key) {
        if !pages.contains(&page) {
            pages.push(page);
        }
    }
    pages
}

/// A full page order: the pages named, then the rest in their default order.
pub fn page_order(text: &str) -> Vec<Page> {
    let mut pages = pages_named(text);
    for page in Page::ALL {
        if !pages.contains(&page) {
            pages.push(page);
        }
    }
    pages
}

/// The comments of `drop.ini` in English, line for line; the keys and
/// values stay as they are, so the file reads the same either way.
const COMMENTS_EN: [(&str, &str); 32] = [
    (
        "Pengaturan Drop. Ubah lewat menu tray > Pengaturan, atau langsung di sini;",
        "Drop's settings. Change them from the tray menu > Settings, or right here;",
    ),
    (
        "Drop membaca ulang file ini dalam satu menit.",
        "Drop reads this file again within a minute. Keys stay in Indonesian.",
    ),
    (
        "Koordinat dalam derajat desimal; lintang selatan bernilai negatif.",
        "Coordinates in decimal degrees; southern latitudes are negative.",
    ),
    (
        "Ketinggian dalam meter di atas permukaan laut.",
        "Elevation in metres above sea level.",
    ),
    (
        "Berapa menit sebelum waktu sholat peringatan muncul.",
        "How many minutes before each prayer time the reminder shows.",
    ),
    (
        "Ingatkan cabut charger saat baterai sampai 80%.",
        "Suggest unplugging the charger when the battery reaches 80%.",
    ),
    (
        "Notifikasi saat mikrofon di-mute atau dinyalakan.",
        "A notice when the microphone is muted or turned on.",
    ),
    (
        "Notifikasi aplikasi lain, seperti WhatsApp.",
        "Notifications from other apps, such as WhatsApp.",
    ),
    (
        "Notifikasi aplikasi dan bunyi diam saat Do Not Disturb Windows aktif.",
        "App notifications and sounds stay quiet while Windows' Do Not Disturb is on.",
    ),
    (
        "Bahasa tampilan: id (Indonesia) atau en (English).",
        "Display language: id (Indonesian) or en (English).",
    ),
    (
        "Notch kecil di kanan saat kamera atau mikrofon dipakai.",
        "A small notch on the right while the camera or microphone is in use.",
    ),
    (
        "Menit tanpa sentuhan keyboard atau mouse sebelum mata muncul.",
        "Minutes without keyboard or mouse before the eyes appear.",
    ),
    (
        "Cuaca diambil dari Open-Meteo untuk lokasi di atas.",
        "The weather comes from Open-Meteo for the location above.",
    ),
    (
        "Screenshot baru langsung masuk shelf.",
        "New screenshots go straight to the shelf.",
    ),
    (
        "Teks dan gambar yang dicopy masuk shelf sampai Drop ditutup.",
        "Copied text and images go to the shelf until Drop closes.",
    ),
    (
        "Waktu pakai per app hari ini, cuma disimpan di PC ini.",
        "Time spent per app, kept on this PC only.",
    ),
    (
        "Screen Time menampilkan 7 hari terakhir, bukan hari ini.",
        "Screen Time shows the last 7 days instead of today.",
    ),
    (
        "Lirik dicari di lrclib.net dari judul dan penyanyi lagu.",
        "Lyrics are looked up on lrclib.net by the song's title and artist.",
    ),
    (
        "Saat tertutup notch hanya bar hitam tipis.",
        "When closed the notch is only a thin black bar.",
    ),
    (
        "Hover hanya melebarkan notch, kartu terbuka saat diklik.",
        "Hovering only widens the notch; the card opens on a click.",
    ),
    (
        "Notch dan kartu memantul saat melebar.",
        "The notch and card bounce as they grow.",
    ),
    (
        "Panah kiri dan kanan untuk ganti halaman kartu.",
        "Left and right arrows to change the card's page.",
    ),
    (
        "Ukuran notch: kecil, normal atau besar.",
        "Notch size: kecil (small), normal or besar (large).",
    ),
    (
        "Notch mengambang di bawah tepi atas layar, tidak menempel.",
        "The notch floats below the screen's top edge instead of joining it.",
    ),
    (
        "Sembunyikan notch saat app di depan fullscreen, juga saat di-maximize, atau tidak.",
        "Tuck the notch away for a full-screen app (fullscreen), also a maximized one (maximize), or not (tidak).",
    ),
    (
        "Jam daerah lain di Home: selisih jam dari UTC (7 WIB, 8 WITA, 9 WIT); kosong = mati.",
        "A second clock on Home: hours from UTC (7 WIB, 8 WITA, 9 WIT); empty = off.",
    ),
    (
        "Tautan kalender .ics (Google Calendar: Setelan > Alamat rahasia",
        "Calendar .ics links (Google Calendar: Settings > Secret address",
    ),
    (
        "dalam format iCal). Kosongkan untuk mematikan agenda.",
        "in iCal format). Leave them empty to turn the agenda off.",
    ),
    (
        "Urutan halaman kartu, dipisah koma: musik, home, notifikasi, sholat, kalender,",
        "The card's pages in order, separated by commas: musik, home, notifikasi, sholat, kalender,",
    ),
    (
        "pomodoro, tugas, kalkulator, screen_time, shelf. Yang tidak disebut ditaruh di akhir.",
        "pomodoro, tugas, kalkulator, screen_time, shelf. Pages left out go at the end.",
    ),
    (
        "Halaman yang tidak tampil di kartu, dipisah koma. Home selalu tampil.",
        "Pages kept off the card, separated by commas. Home always shows.",
    ),
    (
        "Pengingat setelah sekian menit memakai komputer; menjauh dari",
        "Reminders after so many minutes at the computer; being away",
    ),
];

/// `text` with its comments in English.
fn english_comments(text: &str) -> String {
    text.lines()
        .map(|line| {
            let Some(comment) = line.strip_prefix("; ") else {
                return line.to_string();
            };
            if let Some((_, english)) = COMMENTS_EN.iter().find(|(id, _)| *id == comment) {
                return format!("; {english}");
            }
            // "komputer 5 menit atau lebih memulai hitungan dari awal."
            comment
                .strip_prefix("komputer ")
                .and_then(|rest| rest.strip_suffix(" menit atau lebih memulai hitungan dari awal."))
                .map_or(line.to_string(), |minutes| {
                    format!("; for {minutes} minutes or more starts the count over.")
                })
        })
        .map(|line| line + "\n")
        .collect()
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

/// UTC offsets run from -12 to +14 hours.
pub fn valid_zone(value: f64) -> bool {
    (-12.0..=14.0).contains(&value)
}

pub fn valid_longitude(value: f64) -> bool {
    value.abs() <= 180.0
}

fn parse(text: &str) -> Settings {
    let mut settings = Settings {
        // A file from before the language setting was written by someone
        // using Drop in Indonesian; only a new file follows Windows.
        language: Lang::Indonesian,
        ..Settings::default()
    };
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
            "baterai_80" => s.charge_limit = flag(false),
            "mic" => s.notify_mic = flag(true),
            "titik_privasi" => s.privacy_dots = flag(true),
            "bahasa" => s.language = Lang::from_key(value).unwrap_or_else(Lang::system),
            "sholat" => s.notify_prayer = flag(true),
            "aplikasi" => s.notify_apps = flag(true),
            "ikuti_dnd" => s.follow_dnd = flag(true),
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
            "kalkulator" => s.module_calc = flag(true),
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
            "notch_mengambang" => s.notch_floating = flag(false),
            "sembunyi_otomatis" => {
                s.auto_hide = match value.to_lowercase().as_str() {
                    "tidak" => AutoHide::Never,
                    "maximize" => AutoHide::Maximized,
                    _ => AutoHide::Fullscreen,
                }
            }
            "zona_kedua" => s.second_zone = number().filter(|&v| valid_zone(v)),
            "ical" => s.ical_urls[0] = value.to_string(),
            "ical2" => s.ical_urls[1] = value.to_string(),
            "ical3" => s.ical_urls[2] = value.to_string(),
            "urutan_halaman" => s.page_order = page_order(value),
            "halaman_sembunyi" => {
                s.pages_hidden = pages_named(value)
                    .into_iter()
                    .filter(|&page| page != Page::Status)
                    .collect()
            }
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
            charge_limit: true,
            notify_mic: false,
            notify_prayer: true,
            notify_apps: false,
            shelf_screenshots: false,
            shelf_clipboard: false,
            follow_dnd: false,
            sound: false,
            privacy_dots: false,
            idle_minutes: 5,
            weather: false,
            module_music: false,
            module_calendar: true,
            module_shelf: false,
            module_prayer: true,
            module_eyes: false,
            module_todo: false,
            module_calc: false,
            module_usage: false,
            usage_week: true,
            module_lyrics: false,
            notch_minimal: true,
            open_on_click: true,
            card_bounce: false,
            card_arrows: false,
            notch_size: NotchSize::Large,
            notch_floating: true,
            auto_hide: AutoHide::Maximized,
            second_zone: Some(5.5),
            ical_urls: [
                "https://calendar.google.com/calendar/ical/x%40gmail.com/private-ab=cd/basic.ics"
                    .to_string(),
                String::new(),
                "https://example.com/kerja.ics".to_string(),
            ],
            language: Lang::English,
            page_order: page_order("shelf, home, tugas"),
            pages_hidden: vec![Page::Usage, Page::Music],
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
    fn older_files_stay_in_indonesian() {
        assert_eq!(parse("musik = ya\n").language, Lang::Indonesian);
        assert_eq!(parse("bahasa = en\n").language, Lang::English);
    }

    #[test]
    fn english_comments_read_the_same() {
        let settings = Settings {
            language: Lang::English,
            ..Settings::default()
        };
        let text = english_comments(&settings.to_ini());
        assert_eq!(parse(&text), settings);
        // Every comment line found its English one.
        let indonesian = settings.to_ini();
        for (id, en) in indonesian.lines().zip(text.lines()) {
            if id.starts_with("; ") {
                assert_ne!(id, en, "untranslated: {id}");
            }
        }
    }

    #[test]
    fn page_order_is_always_complete() {
        let order = page_order("tugas, apa_ini, tugas, HOME");
        assert_eq!(order.len(), Page::ALL.len());
        assert_eq!(&order[..3], &[Page::Todo, Page::Status, Page::Music]);
        assert_eq!(page_order(""), Page::ALL.to_vec());
        let hidden = parse(
            "halaman_sembunyi = home, shelf
",
        )
        .pages_hidden;
        assert_eq!(hidden, vec![Page::Shelf]);
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
