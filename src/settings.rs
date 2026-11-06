//! The settings window: a sidebar of pages, one per feature. `PAGES` lists what
//! each page holds; the Sholat page adds place search and a live preview of
//! today's times above its rows.
//!
//! Text fields are native EDIT controls (for typing, selection and IME);
//! everything else is drawn with Direct2D in Windows 11's dark style. Every
//! change is written as soon as it is made, so there is no Save button. The
//! window only exists while open, so it costs nothing the rest of the time.

use std::cell::RefCell;
use std::ffi::c_void;

use windows::core::{w, Result, HSTRING};
use windows::Win32::Foundation::{COLORREF, HWND, LPARAM, LRESULT, POINT, RECT, WPARAM};
use windows::Win32::Graphics::Direct2D::Common::{
    D2D1_ALPHA_MODE_IGNORE, D2D1_COLOR_F, D2D1_PIXEL_FORMAT, D2D_RECT_F,
};
use windows::Win32::Graphics::Direct2D::{
    D2D1CreateFactory, ID2D1DCRenderTarget, ID2D1Factory, ID2D1SolidColorBrush,
    D2D1_ANTIALIAS_MODE_PER_PRIMITIVE, D2D1_DRAW_TEXT_OPTIONS_NONE, D2D1_ELLIPSE,
    D2D1_FACTORY_TYPE_SINGLE_THREADED, D2D1_FEATURE_LEVEL_DEFAULT, D2D1_RENDER_TARGET_PROPERTIES,
    D2D1_RENDER_TARGET_TYPE_SOFTWARE, D2D1_RENDER_TARGET_USAGE_NONE, D2D1_ROUNDED_RECT,
};
use windows::Win32::Graphics::DirectWrite::{
    DWriteCreateFactory, IDWriteFactory, IDWriteTextFormat, DWRITE_FACTORY_TYPE_SHARED,
    DWRITE_FONT_STRETCH_NORMAL, DWRITE_FONT_STYLE_NORMAL, DWRITE_FONT_WEIGHT,
    DWRITE_FONT_WEIGHT_NORMAL, DWRITE_FONT_WEIGHT_SEMI_BOLD, DWRITE_MEASURING_MODE_NATURAL,
    DWRITE_PARAGRAPH_ALIGNMENT_CENTER, DWRITE_TEXT_ALIGNMENT, DWRITE_TEXT_ALIGNMENT_CENTER,
    DWRITE_TEXT_ALIGNMENT_LEADING, DWRITE_WORD_WRAPPING_NO_WRAP,
};
use windows::Win32::Graphics::Dwm::{
    DwmSetWindowAttribute, DWMWA_CAPTION_COLOR, DWMWA_USE_IMMERSIVE_DARK_MODE,
    DWMWA_WINDOW_CORNER_PREFERENCE, DWMWCP_ROUND,
};
use windows::Win32::Graphics::Dxgi::Common::DXGI_FORMAT_B8G8R8A8_UNORM;
use windows::Win32::Graphics::Gdi::{
    BeginPaint, CreateFontW, CreateSolidBrush, DeleteObject, EndPaint, InvalidateRect,
    ScreenToClient, SetBkColor, SetTextColor, CLEARTYPE_QUALITY, CLIP_DEFAULT_PRECIS,
    DEFAULT_CHARSET, HBRUSH, HDC, HFONT, OUT_DEFAULT_PRECIS, PAINTSTRUCT,
};
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::UI::Controls::EM_SETSEL;
use windows::Win32::UI::HiDpi::GetDpiForSystem;
use windows::Win32::UI::Input::KeyboardAndMouse::{GetKeyState, SetFocus, VK_CONTROL};
use windows::Win32::UI::Shell::ShellExecuteW;
use windows::Win32::UI::WindowsAndMessaging::{
    CreateWindowExW, DefWindowProcW, DestroyWindow, GetClientRect, GetCursorPos,
    GetWindowTextLengthW, GetWindowTextW, IsChild, IsDialogMessageW, LoadCursorW, PostMessageW,
    RegisterClassExW, SendMessageW, SetCursor, SetForegroundWindow, SetWindowPos, SetWindowTextW,
    ShowWindow, SystemParametersInfoW, EN_CHANGE, EN_KILLFOCUS, EN_SETFOCUS, ES_AUTOHSCROLL, HMENU,
    HTCAPTION, HTCLIENT, ICON_BIG, ICON_SMALL, IDCANCEL, IDC_ARROW, IDC_HAND, IDOK, MSG,
    SPI_GETWORKAREA, SWP_FRAMECHANGED, SWP_NOACTIVATE, SWP_NOMOVE, SWP_NOSIZE, SWP_NOZORDER,
    SW_HIDE, SW_SHOW, SW_SHOWNORMAL, SYSTEM_PARAMETERS_INFO_UPDATE_FLAGS, WINDOW_EX_STYLE,
    WINDOW_STYLE, WM_APP, WM_CLOSE, WM_COMMAND, WM_CTLCOLOREDIT, WM_DESTROY, WM_ERASEBKGND,
    WM_KEYDOWN, WM_LBUTTONUP, WM_MOUSEMOVE, WM_MOUSEWHEEL, WM_NCCALCSIZE, WM_NCHITTEST,
    WM_NCMOUSEMOVE, WM_PAINT, WM_SETCURSOR, WM_SETFONT, WM_SETICON, WNDCLASSEXW, WS_CAPTION,
    WS_CHILD, WS_CLIPCHILDREN, WS_OVERLAPPED, WS_SYSMENU, WS_TABSTOP,
};

use windows_numerics::Matrix3x2;

use crate::config::{self, AutoHide, NotchSize, Settings};
use crate::geo::{self, Place};
use crate::render::{self, Page as CardPage};
use crate::{autostart, prayer, tray, update};

/// Sent to the notch window after the settings were saved.
pub const SAVED_MSG: u32 = WM_APP + 6;
/// Place search finished. LPARAM: `Box<Result<Vec<Place>, String>>`.
const SEARCH_DONE_MSG: u32 = WM_APP + 7;
/// The update check finished. LPARAM: `Box<Option<String>>`, the latest version.
const UPDATE_DONE_MSG: u32 = WM_APP + 23;

const CLIENT: (f32, f32) = (680.0, 560.0);
/// The title bar above the page, whose coordinates start under it.
const TITLE_BAR: f32 = 34.0;
/// Center and radius of the close button, in the title bar.
const CLOSE: (f32, f32, f32) = (CLIENT.0 - 21.0, -TITLE_BAR / 2.0, 10.0);
const CLOSE_RED: u32 = 0xe5484d;

// Child control IDs.
const ID_SEARCH: usize = 101;
const ID_CITY: usize = 102;
const ID_LATITUDE: usize = 103;
const ID_LONGITUDE: usize = 104;
const ID_ELEVATION: usize = 105;
const ID_REMIND: usize = 106;
const ID_IDLE: usize = 107;
const ID_ICAL: usize = 108;
const ID_ICAL2: usize = 109;
const ID_ICAL3: usize = 110;

// Layout, in logical pixels.
const SIDEBAR: f32 = 168.0;
const LEFT: f32 = 184.0;
const RIGHT: f32 = 664.0;
const TITLE: Rect = Rect::new(LEFT, 10.0, RIGHT, 46.0);
const NAV_TOP: f32 = 12.0;
const NAV_STEP: f32 = 36.0;
const HEADING: f32 = 24.0;
const ROW: f32 = 48.0;
const GROUP_GAP: f32 = 10.0;
const CONTENT_TOP: f32 = 50.0;
const FOOTER_TOP: f32 = 516.0;
const FOOTER_STATUS: Rect = Rect::new(LEFT, 524.0, RIGHT, 552.0);

// The Sholat page's location block.
const LOCATION_HEADING: Rect = Rect::new(LEFT + 2.0, 50.0, RIGHT, 72.0);
const SEARCH_FIELD: Rect = Rect::new(LEFT, 74.0, 584.0, 104.0);
const SEARCH_BUTTON: Rect = Rect::new(592.0, 74.0, RIGHT, 104.0);
const STATUS: Rect = Rect::new(LEFT, 106.0, RIGHT, 122.0);
/// Labels above the prayer fields: text and top-left corner.
const FIELD_LABELS: [(&str, f32, f32); 4] = [
    ("Nama lokasi", LEFT, 124.0),
    ("Ketinggian (meter)", 430.0, 124.0),
    ("Lintang", LEFT, 176.0),
    ("Bujur", 430.0, 176.0),
];
/// Search results drop down from under the search box, over these fields.
const RESULTS_TOP: f32 = 106.0;
const RESULT_ROW: f32 = 38.0;
const COVERED_FIELDS: [usize; 5] = [ID_CITY, ID_LATITUDE, ID_LONGITUDE, ID_ELEVATION, ID_REMIND];
const CITY_FIELD: Rect = Rect::new(LEFT, 142.0, 418.0, 172.0);
const ELEVATION_FIELD: Rect = Rect::new(430.0, 142.0, RIGHT, 172.0);
const LATITUDE_FIELD: Rect = Rect::new(LEFT, 194.0, 418.0, 224.0);
const LONGITUDE_FIELD: Rect = Rect::new(430.0, 194.0, RIGHT, 224.0);
const PREVIEW: Rect = Rect::new(LEFT, 232.0, RIGHT, 310.0);
/// Where the Sholat page's rows start, under the location block.
const PRAYER_TOP: f32 = 318.0;

const SEARCH_HINT: &str = "Cari kota, kabupaten, atau desa";

/// Switch states, in this order: five notifications, start with Windows,
/// eight modules, three health reminders, the minimal notch, opening the card
/// by click, and what comes after those as added.
const TOGGLE_COUNT: usize = 30;
/// First health reminder switch.
const FIRST_HEALTH: usize = 14;

const PAGE_GENERAL: usize = 0;
const PAGE_ORDER: usize = 2;
const PAGE_PRAYER: usize = 9;
const PAGE_ABOUT: usize = 12;

enum Row {
    Switch {
        toggle: usize,
        title: &'static str,
        note: &'static str,
    },
    Field {
        id: usize,
        width: f32,
        title: &'static str,
        note: &'static str,
    },
    /// When the notch hides for the app in front, picked from [`HIDES`].
    Hide {
        title: &'static str,
        note: &'static str,
    },
    /// The second clock's zone, picked from [`ZONES`].
    Zone {
        title: &'static str,
        note: &'static str,
    },
    /// The notch size, picked from [`SIZES`].
    Size {
        title: &'static str,
        note: &'static str,
    },
    /// The notch's style, picked from [`STYLES`].
    Style {
        title: &'static str,
        note: &'static str,
    },
}

const SIZES: [(&str, NotchSize); 3] = [
    ("Kecil", NotchSize::Small),
    ("Normal", NotchSize::Normal),
    ("Besar", NotchSize::Large),
];
const SIZE_WIDTH: f32 = 60.0;
/// Indonesia's three zones, as hours from UTC, or none.
const ZONES: [(&str, Option<f64>); 4] = [
    ("Mati", None),
    ("WIB", Some(7.0)),
    ("WITA", Some(8.0)),
    ("WIT", Some(9.0)),
];
const ZONE_WIDTH: f32 = 58.0;
const HIDES: [(&str, AutoHide); 3] = [
    ("Mati", AutoHide::Never),
    ("Fullscreen", AutoHide::Fullscreen),
    ("Maximize", AutoHide::Maximized),
];
const HIDE_WIDTH: f32 = 80.0;
const STYLES: [(&str, bool); 2] = [("Classic", false), ("Floating", true)];
const STYLE_WIDTH: f32 = 90.0;

struct Group {
    title: &'static str,
    rows: &'static [Row],
}

struct Page {
    title: &'static str,
    glyph: &'static str,
    color: u32,
    /// Top of the first group.
    top: f32,
    groups: &'static [Group],
}

static PAGES: [Page; 13] = [
    Page {
        title: "General",
        glyph: "\u{E713}",
        color: 0x8e8e93,
        top: CONTENT_TOP,
        groups: &[Group {
            title: "Notch",
            rows: &[
                Row::Style {
                    title: "Style notch",
                    note: "Menempel di tepi atas, atau mengambang",
                },
                Row::Size {
                    title: "Ukuran notch",
                    note: "Cuma notch tertutup, kartu tetap",
                },
                Row::Switch {
                    toggle: 17,
                    title: "Notch minimal",
                    note: "Bar hitam tipis yang menempel di atas, tab browser tetap kelihatan",
                },
                Row::Hide {
                    title: "Sembunyi otomatis",
                    note: "Jadi bar tipis, tidak hilang",
                },
                Row::Switch {
                    toggle: 11,
                    title: "Mata saat idle",
                    note: "Notch kasih lihat mata saat PC didiamkan",
                },
                Row::Field {
                    id: ID_IDLE,
                    width: 64.0,
                    title: "Mata muncul setelah",
                    note: "Menit tanpa aktivitas, 1 sampai 120",
                },
                Row::Switch {
                    toggle: 5,
                    title: "Start bareng Windows",
                    note: "Drop kebuka sendiri setelah masuk Windows",
                },
            ],
        }],
    },
    Page {
        title: "Kartu",
        glyph: "\u{E8A1}",
        color: 0xbf5af2,
        top: CONTENT_TOP,
        groups: &[Group {
            title: "Kartu",
            rows: &[
                Row::Switch {
                    toggle: 18,
                    title: "Klik buat buka kartu",
                    note: "Hover cuma melebarkan notch dan kasih lihat tanggal",
                },
                Row::Switch {
                    toggle: 21,
                    title: "Bounce",
                    note: "Notch dan kartu mantul saat melebar",
                },
                Row::Switch {
                    toggle: 24,
                    title: "Panah halaman",
                    note: "Panah kiri dan kanan di kartu buat ganti halaman",
                },
                Row::Zone {
                    title: "Jam daerah lain",
                    note: "Tampil di Home, misal jam Jakarta",
                },
            ],
        }],
    },
    // Drawn by hand: the card's pages, to show or hide and put in order.
    Page {
        title: "Halaman",
        glyph: "\u{E8FD}",
        color: 0xac8e68,
        top: CONTENT_TOP,
        groups: &[],
    },
    Page {
        title: "Notifikasi",
        glyph: "\u{EA8F}",
        color: 0x32ade6,
        top: CONTENT_TOP,
        groups: &[Group {
            title: "Sistem",
            rows: &[
                Row::Switch {
                    toggle: 0,
                    title: "Wi-Fi connect dan disconnect",
                    note: "Kabarin saat koneksi berubah",
                },
                Row::Switch {
                    toggle: 1,
                    title: "Device Bluetooth",
                    note: "Kabarin saat device connect atau disconnect",
                },
                Row::Switch {
                    toggle: 2,
                    title: "Battery",
                    note: "Kabarin saat dicas, dilepas, penuh, dan hampir habis",
                },
                Row::Switch {
                    toggle: 28,
                    title: "Ingatkan di 80%",
                    note: "Saran cabut charger biar baterai awet",
                },
                Row::Switch {
                    toggle: 26,
                    title: "Mic mute dan unmute",
                    note: "Kabarin saat mikrofon dimatikan atau dinyalakan",
                },
                Row::Switch {
                    toggle: 27,
                    title: "Indikator kamera dan mic",
                    note: "Notch kecil di kanan saat kamera atau mic dipakai, klik mic buat mute",
                },
                Row::Switch {
                    toggle: 19,
                    title: "Notifikasi app",
                    note: "Pesan dari app lain, kayak WhatsApp",
                },
                Row::Switch {
                    toggle: 25,
                    title: "Ikuti Do Not Disturb",
                    note: "Notifikasi app dan suara diam saat Do Not Disturb Windows nyala",
                },
                Row::Switch {
                    toggle: 4,
                    title: "Suara notifikasi",
                    note: "Suara singkat saat notifikasi muncul",
                },
            ],
        }],
    },
    Page {
        title: "Music & lirik",
        glyph: "\u{E8D6}",
        color: 0xff375f,
        top: CONTENT_TOP,
        groups: &[Group {
            title: "Modul",
            rows: &[
                Row::Switch {
                    toggle: 7,
                    title: "Music",
                    note: "Kartu player dan info lagu di notch",
                },
                Row::Switch {
                    toggle: 13,
                    title: "Lirik lagu",
                    note: "Lirik sync dari LRCLIB",
                },
            ],
        }],
    },
    Page {
        title: "Cuaca",
        glyph: "\u{E706}",
        color: 0xff9f0a,
        top: CONTENT_TOP,
        groups: &[Group {
            title: "Modul",
            rows: &[Row::Switch {
                toggle: 6,
                title: "Cuaca",
                note: "Suhu di notch dan Home, pakai lokasi Sholat",
            }],
        }],
    },
    Page {
        title: "To-do",
        glyph: "\u{E73E}",
        color: 0x30d158,
        top: CONTENT_TOP,
        groups: &[Group {
            title: "Modul",
            rows: &[
                Row::Switch {
                    toggle: 12,
                    title: "To-do",
                    note: "To-do list di notch",
                },
                Row::Switch {
                    toggle: 29,
                    title: "Kalkulator",
                    note: "Halaman kalkulator sederhana di kartu",
                },
            ],
        }],
    },
    Page {
        title: "Screen Time",
        glyph: "\u{E7F4}",
        color: 0x5e5ce6,
        top: CONTENT_TOP,
        groups: &[Group {
            title: "Modul",
            rows: &[Row::Switch {
                toggle: 23,
                title: "Screen Time",
                note: "Waktu pakai per app sampai 7 hari, cuma disimpan di PC ini",
            }],
        }],
    },
    Page {
        title: "Kalender",
        glyph: "\u{E787}",
        color: 0xff453a,
        top: CONTENT_TOP,
        groups: &[
            Group {
                title: "Modul",
                rows: &[Row::Switch {
                    toggle: 8,
                    title: "Kalender & Pomodoro",
                    note: "Kartu kalender, agenda, dan timer fokus",
                }],
            },
            Group {
                title: "Agenda",
                rows: &[
                    Row::Field {
                        id: ID_ICAL,
                        width: 240.0,
                        title: "Link kalender 1",
                        note: "Link .ics, acaranya warna biru",
                    },
                    Row::Field {
                        id: ID_ICAL2,
                        width: 240.0,
                        title: "Link kalender 2",
                        note: "Link .ics, acaranya warna hijau",
                    },
                    Row::Field {
                        id: ID_ICAL3,
                        width: 240.0,
                        title: "Link kalender 3",
                        note: "Link .ics, acaranya warna merah",
                    },
                ],
            },
        ],
    },
    Page {
        title: "Sholat",
        glyph: "\u{E823}",
        color: 0x00c7be,
        top: PRAYER_TOP,
        groups: &[Group {
            title: "Modul & pengingat",
            rows: &[
                Row::Switch {
                    toggle: 10,
                    title: "Halaman sholat",
                    note: "Kartu jadwal sholat dan posisi matahari",
                },
                Row::Switch {
                    toggle: 3,
                    title: "Pengingat sholat",
                    note: "Notifikasi menjelang waktu sholat",
                },
                Row::Field {
                    id: ID_REMIND,
                    width: 64.0,
                    title: "Ingatkan sebelum",
                    note: "Menit sebelum waktu sholat, 1 sampai 60",
                },
            ],
        }],
    },
    Page {
        title: "Health",
        glyph: "\u{EB42}",
        color: 0x0a84ff,
        top: CONTENT_TOP,
        groups: &[Group {
            title: "Pengingat",
            rows: &[
                Row::Switch {
                    toggle: 14,
                    title: "Minum air",
                    note: "Pengingat minum",
                },
                Row::Switch {
                    toggle: 15,
                    title: "Istirahat mata",
                    note: "Pengingat istirahat mata",
                },
                Row::Switch {
                    toggle: 16,
                    title: "Stretching",
                    note: "Pengingat berdiri dan meregang",
                },
            ],
        }],
    },
    Page {
        title: "Shelf",
        glyph: "\u{E8B7}",
        color: 0xffb340,
        top: CONTENT_TOP,
        groups: &[Group {
            title: "Modul",
            rows: &[
                Row::Switch {
                    toggle: 9,
                    title: "Shelf",
                    note: "Tampung file yang di-drag ke notch",
                },
                Row::Switch {
                    toggle: 20,
                    title: "Screenshot otomatis",
                    note: "Screenshot baru langsung masuk shelf",
                },
                Row::Switch {
                    toggle: 22,
                    title: "Clipboard",
                    note: "Teks dan gambar yang dicopy masuk shelf sampai Drop ditutup",
                },
            ],
        }],
    },
    // Drawn by hand: the version and the update check.
    Page {
        title: "About",
        glyph: "\u{E946}",
        color: 0x64d2ff,
        top: CONTENT_TOP,
        groups: &[],
    },
];

/// The Halaman page's list: a heading, then one row per card page.
const ORDER_ROW: f32 = 42.0;

fn order_row(index: usize) -> Rect {
    let top = CONTENT_TOP + HEADING + index as f32 * ORDER_ROW;
    Rect::new(LEFT, top, RIGHT, top + ORDER_ROW)
}

/// The button that moves a row's page up, or down, left of its switch.
fn order_button(row: Rect, down: bool) -> Rect {
    let cy = (row.top + row.bottom) / 2.0;
    let right = row.right - 72.0 - if down { 0.0 } else { 32.0 };
    Rect::new(right - 28.0, cy - 14.0, right, cy + 14.0)
}

/// What a click on the Halaman page's list does, by row.
#[derive(Clone, Copy, PartialEq, Debug)]
enum OrderClick {
    Up(usize),
    Down(usize),
    Show(usize),
}

/// Each card page's colour in the Halaman list.
fn card_color(page: CardPage) -> u32 {
    match page {
        CardPage::Music => 0xff375f,
        CardPage::Status => 0x8e8e93,
        CardPage::Notifications => 0x32ade6,
        CardPage::Prayer => 0x00c7be,
        CardPage::Calendar => 0xff453a,
        CardPage::Pomodoro => 0xff9f0a,
        CardPage::Todo => 0x30d158,
        CardPage::Calculator => 0x64d2ff,
        CardPage::Usage => 0x5e5ce6,
        CardPage::Shelf => 0xffb340,
    }
}

/// The About page's card, and its button.
const ABOUT_CARD: Rect = Rect::new(LEFT, 50.0, RIGHT, 158.0);
const UPDATE_BUTTON: Rect = Rect::new(RIGHT - 16.0 - 150.0, 66.0, RIGHT - 16.0, 96.0);

/// Where the update check stands.
enum UpdateCheck {
    NotYet,
    Checking,
    Latest,
    /// A newer version, such as "0.2.0".
    Newer(String),
    Failed,
}

// Windows 11 dark palette, as 0xRRGGBB.
const BACKGROUND: u32 = 0x1e1e1e;
const SIDEBAR_FILL: u32 = 0x161616;
const SURFACE: u32 = 0x2a2a2a;
const FIELD: u32 = 0x3a3a3a;
const BORDER: u32 = 0x3d3d3d;
const ACCENT: u32 = 0x4cc2ff;
const SWITCH_ON: u32 = 0x30d158;

#[derive(Clone, Copy)]
struct Rect {
    left: f32,
    top: f32,
    right: f32,
    bottom: f32,
}

impl Rect {
    const fn new(left: f32, top: f32, right: f32, bottom: f32) -> Self {
        Self {
            left,
            top,
            right,
            bottom,
        }
    }

    fn contains(&self, x: f32, y: f32) -> bool {
        x >= self.left && x < self.right && y >= self.top && y < self.bottom
    }

    fn d2d(&self) -> D2D_RECT_F {
        D2D_RECT_F {
            left: self.left,
            top: self.top,
            right: self.right,
            bottom: self.bottom,
        }
    }
}

fn nav_row(index: usize) -> Rect {
    let top = NAV_TOP + index as f32 * NAV_STEP;
    Rect::new(8.0, top, SIDEBAR - 8.0, top + NAV_STEP - 4.0)
}

fn group_top(page: &Page, group: usize) -> f32 {
    page.groups[..group].iter().fold(page.top, |top, group| {
        top + HEADING + group.rows.len() as f32 * ROW + GROUP_GAP
    })
}

/// Every row of a page with its place.
fn rows(page: &'static Page) -> impl Iterator<Item = (Rect, &'static Row)> {
    page.groups.iter().enumerate().flat_map(move |(g, group)| {
        let top = group_top(page, g) + HEADING;
        group.rows.iter().enumerate().map(move |(r, row)| {
            let top = top + r as f32 * ROW;
            (Rect::new(LEFT, top, RIGHT, top + ROW), row)
        })
    })
}

/// Bottom of the group holding the row that ends at `row_bottom`.
fn group_end(page: &Page, row_bottom: f32) -> f32 {
    (0..page.groups.len())
        .map(|g| group_top(page, g + 1) - GROUP_GAP)
        .find(|&bottom| bottom >= row_bottom)
        .unwrap_or(row_bottom)
}

/// Segment `index` of the size picker at the right end of a row.
fn hide_segment(row: Rect, index: usize) -> Rect {
    let cy = (row.top + row.bottom) / 2.0;
    let left = row.right - 16.0 - HIDE_WIDTH * (HIDES.len() - index) as f32;
    Rect::new(left, cy - 15.0, left + HIDE_WIDTH, cy + 15.0)
}

fn zone_segment(row: Rect, index: usize) -> Rect {
    let cy = (row.top + row.bottom) / 2.0;
    let left = row.right - 16.0 - ZONE_WIDTH * (ZONES.len() - index) as f32;
    Rect::new(left, cy - 15.0, left + ZONE_WIDTH, cy + 15.0)
}

fn size_segment(row: Rect, index: usize) -> Rect {
    let cy = (row.top + row.bottom) / 2.0;
    let left = row.right - 16.0 - SIZE_WIDTH * (SIZES.len() - index) as f32;
    Rect::new(left, cy - 15.0, left + SIZE_WIDTH, cy + 15.0)
}

/// Segment `index` of the style picker at the right end of a row.
fn style_segment(row: Rect, index: usize) -> Rect {
    let cy = (row.top + row.bottom) / 2.0;
    let left = row.right - 16.0 - STYLE_WIDTH * (STYLES.len() - index) as f32;
    Rect::new(left, cy - 15.0, left + STYLE_WIDTH, cy + 15.0)
}

/// The text field at the right end of a row.
fn row_field(row: Rect, width: f32) -> Rect {
    let cy = (row.top + row.bottom) / 2.0;
    Rect::new(
        row.right - 16.0 - width,
        cy - 15.0,
        row.right - 16.0,
        cy + 15.0,
    )
}

/// The page a text field lives on and its place there.
fn field_place(id: usize) -> (usize, Rect) {
    let location = [
        (ID_SEARCH, SEARCH_FIELD),
        (ID_CITY, CITY_FIELD),
        (ID_LATITUDE, LATITUDE_FIELD),
        (ID_LONGITUDE, LONGITUDE_FIELD),
        (ID_ELEVATION, ELEVATION_FIELD),
    ];
    if let Some((_, rect)) = location.iter().find(|(field, _)| *field == id) {
        return (PAGE_PRAYER, *rect);
    }
    for (index, page) in PAGES.iter().enumerate() {
        for (rect, row) in rows(page) {
            if let Row::Field {
                id: field, width, ..
            } = row
            {
                if *field == id {
                    return (index, row_field(rect, *width));
                }
            }
        }
    }
    (PAGE_GENERAL, Rect::new(0.0, 0.0, 0.0, 0.0))
}

struct Fonts {
    title: IDWriteTextFormat,
    section: IDWriteTextFormat,
    body: IDWriteTextFormat,
    small: IDWriteTextFormat,
    button: IDWriteTextFormat,
    /// Semibold body text, left-aligned.
    button_left: IDWriteTextFormat,
    time: IDWriteTextFormat,
    glyph: IDWriteTextFormat,
}

struct Window {
    hwnd: HWND,
    /// The notch window, told when settings are saved.
    notch: HWND,
    scale: f32,
    factory: ID2D1Factory,
    target: ID2D1DCRenderTarget,
    fonts: Fonts,
    brush: ID2D1SolidColorBrush,
    edit_font: HFONT,
    field_brush: HBRUSH,
    /// Text fields: ID, control, place and the page they belong to.
    fields: Vec<(usize, HWND, Rect, usize)>,
    page: usize,
    /// Search results while the dropdown is open.
    places: Vec<Place>,
    /// Highlighted result, picked by Enter.
    hover: usize,
    toggles: [bool; TOGGLE_COUNT],
    /// Minutes of use before each health reminder, kept as they were.
    health_minutes: [u32; 3],
    notch_size: NotchSize,
    /// The mouse is on the close button.
    close_hover: bool,
    notch_floating: bool,
    /// Hours from UTC of the second clock on Home.
    second_zone: Option<f64>,
    auto_hide: AutoHide,
    /// Kept as it was: the form does not show it.
    usage_week: bool,
    /// The card's pages in order, and those kept off it.
    page_order: Vec<CardPage>,
    pages_hidden: Vec<CardPage>,
    status: String,
    status_is_error: bool,
    focused: Option<usize>,
    preview: Option<prayer::Schedule>,
    update: UpdateCheck,
    /// How far the page is scrolled down, in logical pixels.
    scroll: f32,
}

thread_local! {
    static WINDOW: RefCell<Option<Window>> = const { RefCell::new(None) };
}

/// Opens the settings window, or brings it forward if already open.
pub fn open(notch: HWND) {
    let existing = WINDOW.with(|cell| cell.borrow().as_ref().map(|window| window.hwnd));
    if let Some(hwnd) = existing {
        unsafe {
            let _ = ShowWindow(hwnd, SW_SHOWNORMAL);
            let _ = SetForegroundWindow(hwnd);
        }
        return;
    }
    if let Ok(window) = unsafe { Window::create(notch) } {
        let hwnd = window.hwnd;
        WINDOW.with(|cell| *cell.borrow_mut() = Some(window));
        with_window(|window| {
            window.show_fields();
            window.update_preview();
        });
        unsafe {
            let _ = ShowWindow(hwnd, SW_SHOWNORMAL);
            let _ = SetForegroundWindow(hwnd);
        }
    }
}

/// Gives the settings window keyboard navigation (Tab, Enter, Esc) and
/// Ctrl+A in its fields, which EDIT controls leave out, when the message is
/// for it. Call from the message loop.
pub fn handle_dialog_message(msg: &MSG) -> bool {
    let hwnd = WINDOW.with(|cell| cell.try_borrow().ok()?.as_ref().map(|w| w.hwnd));
    let Some(hwnd) = hwnd else {
        return false;
    };
    unsafe {
        let select_all = msg.message == WM_KEYDOWN
            && msg.wParam.0 == usize::from(b'A')
            && GetKeyState(VK_CONTROL.0 as i32) < 0
            && IsChild(hwnd, msg.hwnd).as_bool();
        if select_all {
            SendMessageW(msg.hwnd, EM_SETSEL, Some(WPARAM(0)), Some(LPARAM(-1)));
            return true;
        }
        IsDialogMessageW(hwnd, msg).as_bool()
    }
}

fn with_window<R>(f: impl FnOnce(&mut Window) -> R) -> Option<R> {
    WINDOW.with(|cell| {
        let mut window = cell.try_borrow_mut().ok()?;
        window.as_mut().map(f)
    })
}

impl Window {
    unsafe fn create(notch: HWND) -> Result<Self> {
        let instance = GetModuleHandleW(None)?;
        let class = WNDCLASSEXW {
            cbSize: size_of::<WNDCLASSEXW>() as u32,
            lpfnWndProc: Some(wndproc),
            hInstance: instance.into(),
            hCursor: LoadCursorW(None, IDC_ARROW)?,
            lpszClassName: w!("DropSettings"),
            ..Default::default()
        };
        // Fails harmlessly when the class exists from an earlier opening.
        RegisterClassExW(&class);

        let dpi = GetDpiForSystem();
        let scale = dpi as f32 / 96.0;
        // The caption stays for the shadow and the rounded corners, but the
        // client area covers it, and the page draws its own title bar.
        let style = WS_OVERLAPPED | WS_CAPTION | WS_SYSMENU | WS_CLIPCHILDREN;
        let width = (CLIENT.0 * scale).round() as i32;
        let height = ((CLIENT.1 + TITLE_BAR) * scale).round() as i32;
        let mut work = RECT::default();
        let _ = SystemParametersInfoW(
            SPI_GETWORKAREA,
            0,
            Some(&mut work as *mut RECT as *mut c_void),
            SYSTEM_PARAMETERS_INFO_UPDATE_FLAGS(0),
        );
        let x = work.left + ((work.right - work.left) - width) / 2;
        let y = work.top + ((work.bottom - work.top) - height) / 2;

        let hwnd = CreateWindowExW(
            WINDOW_EX_STYLE(0),
            w!("DropSettings"),
            w!("Drop Settings"),
            style,
            x,
            y.max(work.top),
            width,
            height,
            None,
            None,
            Some(instance.into()),
            None,
        )?;
        // Windows sizes the frame before the first WM_NCCALCSIZE that counts.
        let _ = SetWindowPos(
            hwnd,
            None,
            0,
            0,
            0,
            0,
            SWP_FRAMECHANGED | SWP_NOACTIVATE | SWP_NOMOVE | SWP_NOSIZE | SWP_NOZORDER,
        );
        if let Some((small, large)) = tray::window_icons() {
            for (kind, icon) in [(ICON_SMALL, small), (ICON_BIG, large)] {
                SendMessageW(
                    hwnd,
                    WM_SETICON,
                    Some(WPARAM(kind as usize)),
                    Some(LPARAM(icon.0 as isize)),
                );
            }
        }
        // Dark title bar in the same grey as the window.
        let dark: i32 = 1;
        let _ = DwmSetWindowAttribute(
            hwnd,
            DWMWA_USE_IMMERSIVE_DARK_MODE,
            &dark as *const i32 as *const c_void,
            4,
        );
        let caption = colorref(BACKGROUND);
        let _ = DwmSetWindowAttribute(
            hwnd,
            DWMWA_CAPTION_COLOR,
            &caption as *const COLORREF as *const c_void,
            4,
        );
        // Rounded corners, which a window drawing over its own frame can lose.
        let round = DWMWCP_ROUND;
        let _ = DwmSetWindowAttribute(
            hwnd,
            DWMWA_WINDOW_CORNER_PREFERENCE,
            &round as *const _ as *const c_void,
            4,
        );

        let factory: ID2D1Factory = D2D1CreateFactory(D2D1_FACTORY_TYPE_SINGLE_THREADED, None)?;
        let target = factory.CreateDCRenderTarget(&D2D1_RENDER_TARGET_PROPERTIES {
            r#type: D2D1_RENDER_TARGET_TYPE_SOFTWARE,
            pixelFormat: D2D1_PIXEL_FORMAT {
                format: DXGI_FORMAT_B8G8R8A8_UNORM,
                alphaMode: D2D1_ALPHA_MODE_IGNORE,
            },
            dpiX: dpi as f32,
            dpiY: dpi as f32,
            usage: D2D1_RENDER_TARGET_USAGE_NONE,
            minLevel: D2D1_FEATURE_LEVEL_DEFAULT,
        })?;
        let brush = target.CreateSolidColorBrush(&rgb(0xffffff, 1.0), None)?;
        let fonts = Fonts::new()?;

        let edit_font = CreateFontW(
            -(13.0 * scale).round() as i32,
            0,
            0,
            0,
            400,
            0,
            0,
            0,
            DEFAULT_CHARSET,
            OUT_DEFAULT_PRECIS,
            CLIP_DEFAULT_PRECIS,
            CLEARTYPE_QUALITY,
            0,
            w!("Segoe UI Variable Text"),
        );
        let field_brush = CreateSolidBrush(colorref(FIELD));

        let settings = Settings::load();
        let number = |value: Option<f64>| value.map(|v| v.to_string()).unwrap_or_default();
        let field_specs = [
            (ID_SEARCH, String::new()),
            (ID_CITY, settings.city.clone()),
            (ID_LATITUDE, number(settings.latitude)),
            (ID_LONGITUDE, number(settings.longitude)),
            (ID_ELEVATION, settings.elevation.to_string()),
            (ID_REMIND, settings.remind_minutes.to_string()),
            (ID_IDLE, settings.idle_minutes.to_string()),
            (ID_ICAL, settings.ical_urls[0].clone()),
            (ID_ICAL2, settings.ical_urls[1].clone()),
            (ID_ICAL3, settings.ical_urls[2].clone()),
        ];
        let mut fields = Vec::new();
        for (id, text) in field_specs {
            let (page, rect) = field_place(id);
            // Hidden until `show_fields` picks the page's own.
            let edit = create_child(
                hwnd,
                w!("EDIT"),
                id,
                WS_CHILD | WS_TABSTOP | WINDOW_STYLE(ES_AUTOHSCROLL as u32),
                edit_rect(rect),
                scale,
            )?;
            SendMessageW(
                edit,
                WM_SETFONT,
                Some(WPARAM(edit_font.0 as usize)),
                Some(LPARAM(1)),
            );
            let _ = SetWindowTextW(edit, &HSTRING::from(text));
            fields.push((id, edit, rect, page));
        }

        Ok(Self {
            hwnd,
            notch,
            scale,
            factory,
            target,
            fonts,
            brush,
            edit_font,
            field_brush,
            fields,
            page: PAGE_GENERAL,
            close_hover: false,
            places: Vec::new(),
            hover: 0,
            toggles: [
                settings.notify_wifi,
                settings.notify_bluetooth,
                settings.notify_battery,
                settings.notify_prayer,
                settings.sound,
                autostart::is_enabled(),
                settings.weather,
                settings.module_music,
                settings.module_calendar,
                settings.module_shelf,
                settings.module_prayer,
                settings.module_eyes,
                settings.module_todo,
                settings.module_lyrics,
                settings.water,
                settings.eyes,
                settings.stretch,
                settings.notch_minimal,
                settings.open_on_click,
                settings.notify_apps,
                settings.shelf_screenshots,
                settings.card_bounce,
                settings.shelf_clipboard,
                settings.module_usage,
                settings.card_arrows,
                settings.follow_dnd,
                settings.notify_mic,
                settings.privacy_dots,
                settings.charge_limit,
                settings.module_calc,
            ],
            health_minutes: [
                settings.water_minutes,
                settings.eyes_minutes,
                settings.stretch_minutes,
            ],
            notch_size: settings.notch_size,
            notch_floating: settings.notch_floating,
            second_zone: settings.second_zone,
            auto_hide: settings.auto_hide,
            usage_week: settings.usage_week,
            page_order: settings.page_order.clone(),
            pages_hidden: settings.pages_hidden.clone(),
            status: SEARCH_HINT.to_string(),
            status_is_error: false,
            focused: None,
            preview: None,
            update: UpdateCheck::NotYet,
            scroll: 0.0,
        })
    }

    fn field(&self, id: usize) -> HWND {
        self.fields
            .iter()
            .find(|(field, ..)| *field == id)
            .map(|(_, hwnd, ..)| *hwnd)
            .unwrap_or_default()
    }

    fn text(&self, id: usize) -> String {
        let hwnd = self.field(id);
        unsafe {
            let length = GetWindowTextLengthW(hwnd).max(0) as usize;
            let mut buffer = vec![0u16; length + 1];
            let copied = GetWindowTextW(hwnd, &mut buffer).max(0) as usize;
            String::from_utf16_lossy(&buffer[..copied])
                .trim()
                .to_string()
        }
    }

    fn set_text(&self, id: usize, text: &str) {
        unsafe {
            let _ = SetWindowTextW(self.field(id), &HSTRING::from(text));
        }
    }

    fn invalidate(&self) {
        unsafe {
            let _ = InvalidateRect(Some(self.hwnd), None, false);
        }
    }

    /// Moves to a sidebar page. A hidden field can't keep the keyboard, and the
    /// notifications this causes can't reach us here, so `focused` is reset by hand.
    fn set_page(&mut self, index: usize) {
        self.places.clear();
        if index != self.page {
            self.page = index;
            self.scroll = 0.0;
            self.focused = None;
            unsafe {
                let _ = SetFocus(Some(self.hwnd));
            }
            if self.status_is_error {
                self.status = SEARCH_HINT.to_string();
                self.status_is_error = false;
            }
        }
        self.show_fields();
        self.invalidate();
    }

    /// Shows the current page's fields, minus those under an open dropdown.
    /// Child windows always draw on top of the parent's painting.
    fn show_fields(&self) {
        let dropdown = !self.places.is_empty();
        for (id, hwnd, rect, page) in &self.fields {
            let shown = Rect::new(
                rect.left,
                rect.top - self.scroll,
                rect.right,
                rect.bottom - self.scroll,
            );
            let in_view = shown.top >= CONTENT_TOP - 10.0 && shown.bottom <= FOOTER_TOP;
            let visible =
                *page == self.page && in_view && !(dropdown && COVERED_FIELDS.contains(id));
            let edit = edit_rect(shown);
            unsafe {
                if *page == self.page {
                    let _ = SetWindowPos(
                        *hwnd,
                        None,
                        (edit.left * self.scale).round() as i32,
                        ((edit.top + TITLE_BAR) * self.scale).round() as i32,
                        0,
                        0,
                        SWP_NOSIZE | SWP_NOZORDER | SWP_NOACTIVATE,
                    );
                }
                let _ = ShowWindow(*hwnd, if visible { SW_SHOW } else { SW_HIDE });
            }
        }
    }

    /// The bottom of the current page's content.
    fn page_bottom(&self) -> f32 {
        let rows = rows(&PAGES[self.page])
            .map(|(rect, _)| rect.bottom)
            .fold(TITLE.bottom, f32::max);
        match self.page {
            PAGE_PRAYER => rows.max(PREVIEW.bottom),
            PAGE_ABOUT => rows.max(ABOUT_CARD.bottom),
            PAGE_ORDER => rows.max(order_row(self.page_order.len() - 1).bottom),
            _ => rows,
        }
    }

    /// Scrolls a page taller than the window by a wheel's `delta`.
    fn wheel(&mut self, delta: i16) {
        let most = (self.page_bottom() + 16.0 - FOOTER_TOP).max(0.0);
        let scroll = (self.scroll - f32::from(delta) / 120.0 * ROW).clamp(0.0, most);
        if scroll != self.scroll {
            self.scroll = scroll;
            self.hide_results();
            self.show_fields();
            self.invalidate();
        }
    }

    /// A point in the page's own coordinates: below the window's top, the
    /// content area moves with the scroll. None on the footer.
    fn on_page(&self, x: f32, y: f32) -> Option<(f32, f32)> {
        (y < FOOTER_TOP).then_some((x, y + self.scroll))
    }

    fn set_status(&mut self, text: &str, error: bool) {
        self.status = text.to_string();
        self.status_is_error = error;
        self.invalidate();
    }

    /// Recomputes today's times from the coordinates typed so far.
    fn update_preview(&mut self) {
        let latitude =
            config::parse_number(&self.text(ID_LATITUDE)).filter(|&v| config::valid_latitude(v));
        let longitude =
            config::parse_number(&self.text(ID_LONGITUDE)).filter(|&v| config::valid_longitude(v));
        let elevation = config::parse_number(&self.text(ID_ELEVATION))
            .unwrap_or(0.0)
            .max(0.0);
        self.preview = latitude.zip(longitude).map(|(latitude, longitude)| {
            prayer::today_at(prayer::Location {
                latitude,
                longitude,
                elevation,
            })
        });
        self.invalidate();
    }

    fn start_search(&mut self) {
        let query = self.text(ID_SEARCH);
        if query.is_empty() {
            self.set_status("Ketik nama tempat dulu", true);
            return;
        }
        self.set_status("Searching…", false);
        let target = self.hwnd.0 as isize;
        std::thread::spawn(move || {
            let result = Box::new(geo::search(&query));
            let payload = Box::into_raw(result);
            unsafe {
                let posted = PostMessageW(
                    Some(HWND(target as *mut c_void)),
                    SEARCH_DONE_MSG,
                    WPARAM(0),
                    LPARAM(payload as isize),
                );
                if posted.is_err() {
                    drop(Box::from_raw(payload));
                }
            }
        });
    }

    /// Opens the results dropdown over the fields below the search box.
    fn show_results(&mut self, result: std::result::Result<Vec<Place>, String>) {
        match result {
            Ok(places) if !places.is_empty() => {
                self.places = places;
                self.hover = 0;
                self.set_status("", false);
                self.show_fields();
            }
            Ok(_) => self.set_status("Tempat tidak ditemukan; coba nama lain", true),
            Err(message) => self.set_status(&message, true),
        }
    }

    fn result_rect(&self, index: usize) -> Rect {
        let top = RESULTS_TOP + 4.0 + index as f32 * RESULT_ROW;
        Rect::new(
            SEARCH_FIELD.left + 4.0,
            top,
            SEARCH_BUTTON.right - 4.0,
            top + RESULT_ROW,
        )
    }

    fn results_panel(&self) -> Rect {
        let bottom = RESULTS_TOP + 8.0 + self.places.len() as f32 * RESULT_ROW;
        Rect::new(SEARCH_FIELD.left, RESULTS_TOP, SEARCH_BUTTON.right, bottom)
    }

    fn result_at(&self, x: f32, y: f32) -> Option<usize> {
        (0..self.places.len()).find(|&i| self.result_rect(i).contains(x, y))
    }

    /// Follows the mouse over the dropdown.
    fn hover_at(&mut self, x: f32, y: f32) {
        if on_close(x, y) != self.close_hover {
            self.close_hover = on_close(x, y);
            self.invalidate();
        }
        let y = y + self.scroll;
        if let Some(index) = self.result_at(x, y).filter(|&i| i != self.hover) {
            self.hover = index;
            self.invalidate();
        }
    }

    fn pick_result(&mut self, index: usize) {
        let Some(place) = self.places.get(index).cloned() else {
            return;
        };
        self.set_text(ID_CITY, &place.name);
        self.set_text(ID_LATITUDE, &format!("{:.5}", place.latitude));
        self.set_text(ID_LONGITUDE, &format!("{:.5}", place.longitude));
        self.set_text(ID_ELEVATION, &format!("{}", place.elevation.round()));
        self.hide_results();
        self.set_status(&format!("Lokasi: {}", place.region), false);
        self.update_preview();
        self.apply_live();
    }

    /// Closes through WM_CLOSE: destroying the window right away would run
    /// WM_DESTROY while this state is still borrowed.
    fn close(&self) {
        unsafe {
            let _ = PostMessageW(Some(self.hwnd), WM_CLOSE, WPARAM(0), LPARAM(0));
        }
    }

    fn hide_results(&mut self) {
        self.places.clear();
        self.show_fields();
        self.invalidate();
    }

    /// The settings as the form stands, or where it is wrong and why.
    fn form(&self) -> std::result::Result<Settings, (usize, &'static str)> {
        let latitude_text = self.text(ID_LATITUDE);
        let longitude_text = self.text(ID_LONGITUDE);
        let latitude = config::parse_number(&latitude_text);
        let longitude = config::parse_number(&longitude_text);
        let both_empty = latitude_text.is_empty() && longitude_text.is_empty();
        let coordinates_ok = both_empty
            || latitude.is_some_and(config::valid_latitude)
                && longitude.is_some_and(config::valid_longitude);
        if !coordinates_ok {
            return Err((
                PAGE_PRAYER,
                "Lintang (-90 s/d 90) atau bujur (-180 s/d 180) tidak valid",
            ));
        }
        let Some(remind) = whole_number(&self.text(ID_REMIND), 1, 60) else {
            return Err((PAGE_PRAYER, "Pengingat harus 1 sampai 60 menit"));
        };
        let Some(idle) = whole_number(&self.text(ID_IDLE), 1, 120) else {
            return Err((
                PAGE_GENERAL,
                "Waktu munculnya mata harus 1 sampai 120 menit",
            ));
        };

        Ok(Settings {
            city: self.text(ID_CITY),
            latitude: if both_empty { None } else { latitude },
            longitude: if both_empty { None } else { longitude },
            elevation: config::parse_number(&self.text(ID_ELEVATION))
                .unwrap_or(0.0)
                .max(0.0),
            remind_minutes: remind,
            notify_wifi: self.toggles[0],
            notify_bluetooth: self.toggles[1],
            notify_battery: self.toggles[2],
            notify_prayer: self.toggles[3],
            notify_apps: self.toggles[19],
            shelf_screenshots: self.toggles[20],
            shelf_clipboard: self.toggles[22],
            card_bounce: self.toggles[21],
            card_arrows: self.toggles[24],
            follow_dnd: self.toggles[25],
            notify_mic: self.toggles[26],
            privacy_dots: self.toggles[27],
            charge_limit: self.toggles[28],
            module_calc: self.toggles[29],
            sound: self.toggles[4],
            idle_minutes: idle,
            ical_urls: [ID_ICAL, ID_ICAL2, ID_ICAL3].map(|id| self.text(id).trim().to_string()),
            page_order: self.page_order.clone(),
            pages_hidden: self.pages_hidden.clone(),
            weather: self.toggles[6],
            module_music: self.toggles[7],
            module_calendar: self.toggles[8],
            module_shelf: self.toggles[9],
            module_prayer: self.toggles[10],
            module_eyes: self.toggles[11],
            module_todo: self.toggles[12],
            module_usage: self.toggles[23],
            usage_week: self.usage_week,
            module_lyrics: self.toggles[13],
            water: self.toggles[14],
            water_minutes: self.health_minutes[0],
            eyes: self.toggles[15],
            eyes_minutes: self.health_minutes[1],
            stretch: self.toggles[16],
            stretch_minutes: self.health_minutes[2],
            notch_minimal: self.toggles[17],
            open_on_click: self.toggles[18],
            notch_size: self.notch_size,
            notch_floating: self.notch_floating,
            second_zone: self.second_zone,
            auto_hide: self.auto_hide,
        })
    }

    /// Writes `settings` and tells the notch. Returns false when that failed.
    fn apply(&mut self, settings: &Settings) -> bool {
        if settings.save().is_err() {
            self.set_status("Settings gagal disimpan", true);
            return false;
        }
        if self.toggles[5] != autostart::is_enabled() {
            autostart::set_enabled(self.toggles[5]);
        }
        if self.status_is_error {
            self.status = SEARCH_HINT.to_string();
            self.status_is_error = false;
            self.invalidate();
        }
        unsafe {
            let _ = PostMessageW(Some(self.notch), SAVED_MSG, WPARAM(0), LPARAM(0));
        }
        true
    }

    /// Puts the form into effect at once, so every change sticks. A field
    /// still being typed wrong only shows why in the footer.
    fn apply_live(&mut self) {
        match self.form() {
            Ok(settings) => {
                self.apply(&settings);
            }
            Err((_, message)) => self.set_status(message, true),
        }
    }

    /// Applies the form, or moves to the field that is wrong. Returns whether
    /// it was applied.
    fn commit(&mut self) -> bool {
        match self.form() {
            Ok(settings) => self.apply(&settings),
            Err((page, message)) => {
                self.set_page(page);
                self.set_status(message, true);
                false
            }
        }
    }

    /// The switch under a point on the current page.
    fn toggle_at(&self, x: f32, y: f32) -> Option<usize> {
        rows(&PAGES[self.page]).find_map(|(rect, row)| match row {
            Row::Switch { toggle, .. } if rect.contains(x, y) => Some(*toggle),
            _ => None,
        })
    }

    /// The size under a point on the current page.
    /// The auto-hide choice under a point on the current page.
    fn hide_at(&self, x: f32, y: f32) -> Option<AutoHide> {
        rows(&PAGES[self.page]).find_map(|(rect, row)| match row {
            Row::Hide { .. } => (0..HIDES.len())
                .find(|&i| hide_segment(rect, i).contains(x, y))
                .map(|i| HIDES[i].1),
            _ => None,
        })
    }

    /// The second clock's zone under a point on the current page.
    fn zone_at(&self, x: f32, y: f32) -> Option<Option<f64>> {
        rows(&PAGES[self.page]).find_map(|(rect, row)| match row {
            Row::Zone { .. } => (0..ZONES.len())
                .find(|&i| zone_segment(rect, i).contains(x, y))
                .map(|i| ZONES[i].1),
            _ => None,
        })
    }

    fn size_at(&self, x: f32, y: f32) -> Option<NotchSize> {
        rows(&PAGES[self.page]).find_map(|(rect, row)| match row {
            Row::Size { .. } => (0..SIZES.len())
                .find(|&i| size_segment(rect, i).contains(x, y))
                .map(|i| SIZES[i].1),
            _ => None,
        })
    }

    /// The style under a point on the current page.
    fn style_at(&self, x: f32, y: f32) -> Option<bool> {
        rows(&PAGES[self.page]).find_map(|(rect, row)| match row {
            Row::Style { .. } => (0..STYLES.len())
                .find(|&i| style_segment(rect, i).contains(x, y))
                .map(|i| STYLES[i].1),
            _ => None,
        })
    }

    /// What a click at a point on the Halaman list would do. Home can't be
    /// hidden, and the first and last rows can't move further.
    fn order_at(&self, x: f32, y: f32) -> Option<OrderClick> {
        if self.page != PAGE_ORDER {
            return None;
        }
        let count = self.page_order.len();
        let index = (0..count).find(|&i| order_row(i).contains(x, y))?;
        let row = order_row(index);
        if order_button(row, false).contains(x, y) {
            return (index > 0).then_some(OrderClick::Up(index));
        }
        if order_button(row, true).contains(x, y) {
            return (index + 1 < count).then_some(OrderClick::Down(index));
        }
        (self.page_order[index] != CardPage::Status).then_some(OrderClick::Show(index))
    }

    fn press_order(&mut self, click: OrderClick) {
        match click {
            OrderClick::Up(index) => self.page_order.swap(index, index - 1),
            OrderClick::Down(index) => self.page_order.swap(index, index + 1),
            OrderClick::Show(index) => {
                let page = self.page_order[index];
                if let Some(at) = self.pages_hidden.iter().position(|&p| p == page) {
                    self.pages_hidden.remove(at);
                } else {
                    self.pages_hidden.push(page);
                }
            }
        }
        self.invalidate();
        self.apply_live();
    }

    /// What a page shows, or why it is missing from the card although its
    /// switch is on; false for the latter.
    fn order_note(&self, page: CardPage) -> (&'static str, bool) {
        let on = |toggle: usize| self.toggles[toggle];
        let feeds = [ID_ICAL, ID_ICAL2, ID_ICAL3]
            .iter()
            .any(|&id| !self.text(id).is_empty());
        match page {
            CardPage::Status => ("Selalu tampil", true),
            CardPage::Music if !on(7) => ("Modul mati di Music & lirik", false),
            CardPage::Music => ("Muncul saat lagu diputar", true),
            CardPage::Notifications if !on(19) => ("Notifikasi app mati di Notifikasi", false),
            CardPage::Notifications => ("Pesan dari app lain", true),
            CardPage::Prayer if !on(10) => ("Modul mati di Sholat", false),
            CardPage::Prayer if self.preview.is_none() => ("Isi lokasi di Sholat dulu", false),
            CardPage::Prayer => ("Jadwal sholat dan matahari", true),
            CardPage::Calendar if !on(8) && !feeds => ("Modul mati di Kalender", false),
            CardPage::Calendar => ("Kalender bulan ini dan agenda", true),
            CardPage::Pomodoro if !on(8) => ("Modul mati di Kalender", false),
            CardPage::Pomodoro => ("Fokus, timer, dan stopwatch", true),
            CardPage::Todo if !on(12) => ("Modul mati di To-do", false),
            CardPage::Todo => ("Daftar tugas", true),
            CardPage::Calculator if !on(29) => ("Modul mati di To-do", false),
            CardPage::Calculator => ("Kalkulator sederhana", true),
            CardPage::Usage if !on(23) => ("Modul mati di Screen Time", false),
            CardPage::Usage => ("Waktu pakai per app", true),
            CardPage::Shelf if !on(9) => ("Modul mati di Shelf", false),
            CardPage::Shelf => ("File, screenshot, dan copy-an", true),
        }
    }

    fn update_button(&self, x: f32, y: f32) -> bool {
        self.page == PAGE_ABOUT && UPDATE_BUTTON.contains(x, y)
    }

    /// The About page's button: asks GitHub for the latest version, or once
    /// a newer one is known, opens its download page.
    fn press_update(&mut self) {
        match self.update {
            UpdateCheck::Checking => return,
            UpdateCheck::Newer(_) => {
                unsafe {
                    ShellExecuteW(
                        None,
                        w!("open"),
                        &HSTRING::from(update::RELEASES),
                        None,
                        None,
                        SW_SHOWNORMAL,
                    );
                }
                return;
            }
            _ => {}
        }
        self.update = UpdateCheck::Checking;
        self.invalidate();
        let target = self.hwnd.0 as isize;
        std::thread::spawn(move || {
            let payload = Box::into_raw(Box::new(update::latest()));
            unsafe {
                let posted = PostMessageW(
                    Some(HWND(target as *mut c_void)),
                    UPDATE_DONE_MSG,
                    WPARAM(0),
                    LPARAM(payload as isize),
                );
                if posted.is_err() {
                    drop(Box::from_raw(payload));
                }
            }
        });
    }

    fn show_update(&mut self, latest: Option<String>) {
        self.update = match latest {
            Some(version) if update::is_newer(&version, update::CURRENT) => {
                UpdateCheck::Newer(version)
            }
            Some(_) => UpdateCheck::Latest,
            None => UpdateCheck::Failed,
        };
        self.invalidate();
    }

    fn search_button(&self, x: f32, y: f32) -> bool {
        self.page == PAGE_PRAYER && SEARCH_BUTTON.contains(x, y)
    }

    fn click(&mut self, x: f32, y: f32) {
        if on_close(x, y) {
            self.close();
            return;
        }
        if let Some(index) = (0..PAGES.len()).find(|&i| nav_row(i).contains(x, y)) {
            self.set_page(index);
            return;
        }
        if x < SIDEBAR {
            return;
        }
        let Some((x, y)) = self.on_page(x, y) else {
            return;
        };
        // An open dropdown takes the click: a row picks it, anywhere else closes it.
        if !self.places.is_empty() && !SEARCH_BUTTON.contains(x, y) {
            match self.result_at(x, y) {
                Some(index) => self.pick_result(index),
                None if !self.results_panel().contains(x, y) => self.hide_results(),
                None => {}
            }
            return;
        }
        if let Some(order) = self.order_at(x, y) {
            self.press_order(order);
        } else if self.update_button(x, y) {
            self.press_update();
        } else if self.search_button(x, y) {
            self.start_search();
        } else if let Some(floating) = self.style_at(x, y) {
            self.notch_floating = floating;
            self.invalidate();
            self.apply_live();
        } else if let Some(hide) = self.hide_at(x, y) {
            self.auto_hide = hide;
            self.invalidate();
            self.apply_live();
        } else if let Some(zone) = self.zone_at(x, y) {
            self.second_zone = zone;
            self.invalidate();
            self.apply_live();
        } else if let Some(size) = self.size_at(x, y) {
            self.notch_size = size;
            self.invalidate();
            self.apply_live();
        } else if let Some(index) = self.toggle_at(x, y) {
            self.toggles[index] = !self.toggles[index];
            self.invalidate();
            self.apply_live();
        }
    }

    fn clickable(&self, x: f32, y: f32) -> bool {
        if on_close(x, y) {
            return true;
        }
        if (0..PAGES.len()).any(|i| nav_row(i).contains(x, y)) {
            return true;
        }
        let Some((x, y)) = self.on_page(x, y).filter(|_| x >= SIDEBAR) else {
            return false;
        };
        if !self.places.is_empty() {
            return self.result_at(x, y).is_some() || SEARCH_BUTTON.contains(x, y);
        }
        self.search_button(x, y)
            || self.update_button(x, y)
            || self.order_at(x, y).is_some()
            || self.toggle_at(x, y).is_some()
            || self.size_at(x, y).is_some()
            || self.zone_at(x, y).is_some()
            || self.hide_at(x, y).is_some()
            || self.style_at(x, y).is_some()
    }

    unsafe fn paint(&self, hdc: HDC) {
        let mut client = RECT::default();
        let _ = GetClientRect(self.hwnd, &mut client);
        if self.target.BindDC(hdc, &client).is_err() {
            return;
        }
        let t = &self.target;
        t.BeginDraw();
        t.Clear(Some(&rgb(BACKGROUND, 1.0)));
        t.SetTransform(&Matrix3x2::translation(0.0, TITLE_BAR));

        self.title_bar();
        self.sidebar();
        t.PushAxisAlignedClip(
            &Rect::new(SIDEBAR, 0.0, CLIENT.0, FOOTER_TOP).d2d(),
            D2D1_ANTIALIAS_MODE_PER_PRIMITIVE,
        );
        t.SetTransform(&Matrix3x2::translation(0.0, TITLE_BAR - self.scroll));
        let page = &PAGES[self.page];
        self.text_at(page.title, &self.fonts.title, TITLE, 0xffffff, 1.0);
        if self.page == PAGE_PRAYER {
            self.location();
        }
        if self.page == PAGE_ABOUT {
            self.about();
        }
        if self.page == PAGE_ORDER {
            self.order_list();
        }
        self.page_groups(page);
        // Fields under an open dropdown are hidden, and so is their chrome.
        let dropdown = !self.places.is_empty();
        for (id, _, rect, field_page) in &self.fields {
            if *field_page == self.page && !(dropdown && COVERED_FIELDS.contains(id)) {
                self.field_background(*rect, self.focused == Some(*id));
            }
        }
        if self.page == PAGE_PRAYER {
            self.results_dropdown();
        }
        t.SetTransform(&Matrix3x2::translation(0.0, TITLE_BAR));
        t.PopAxisAlignedClip();
        self.scroll_bar();

        // Footer.
        self.brush.SetColor(&rgb(BORDER, 1.0));
        t.FillRectangle(
            &Rect::new(SIDEBAR, FOOTER_TOP, CLIENT.0, FOOTER_TOP + 1.0).d2d(),
            &self.brush,
        );
        // The Sholat page shows its status under the search box.
        let (footer, color, alpha) = if self.status_is_error && self.page != PAGE_PRAYER {
            (self.status.as_str(), 0xff99a4, 1.0)
        } else {
            ("Setiap perubahan langsung tersimpan", 0xffffff, 0.45)
        };
        self.text_at(footer, &self.fonts.small, FOOTER_STATUS, color, alpha);

        let _ = t.EndDraw(None, None);
    }

    /// A thin bar at the right edge on a page taller than the window.
    unsafe fn scroll_bar(&self) {
        let view = FOOTER_TOP - CONTENT_TOP;
        let content = self.page_bottom() + 16.0 - CONTENT_TOP;
        if content <= view {
            return;
        }
        let height = (view * view / content).max(24.0);
        let most = content - view;
        let top = CONTENT_TOP + (view - height) * (self.scroll / most).clamp(0.0, 1.0);
        self.rounded(
            Rect::new(CLIENT.0 - 7.0, top, CLIENT.0 - 3.0, top + height),
            2.0,
            0xffffff,
            0.25,
        );
    }

    unsafe fn sidebar(&self) {
        let f = &self.fonts;
        self.brush.SetColor(&rgb(SIDEBAR_FILL, 1.0));
        self.target
            .FillRectangle(&Rect::new(0.0, 0.0, SIDEBAR, CLIENT.1).d2d(), &self.brush);
        for (index, page) in PAGES.iter().enumerate() {
            let row = nav_row(index);
            if index == self.page {
                self.rounded(row, 8.0, 0x2c2c2c, 1.0);
            }
            let cy = (row.top + row.bottom) / 2.0;
            let tile = Rect::new(row.left + 6.0, cy - 11.0, row.left + 28.0, cy + 11.0);
            self.rounded(tile, 6.0, page.color, 1.0);
            self.text_at(page.glyph, &f.glyph, tile, 0xffffff, 1.0);
            let label = Rect::new(tile.right + 10.0, row.top, row.right - 8.0, row.bottom);
            self.text_at(page.title, &f.body, label, 0xffffff, 0.95);
        }
    }

    /// The Sholat page's top: place search, coordinates and today's times.
    unsafe fn location(&self) {
        let f = &self.fonts;
        self.text_at("Lokasi", &f.section, LOCATION_HEADING, 0xffffff, 0.7);
        self.button("Search", SEARCH_BUTTON, false);
        let status_color = if self.status_is_error {
            0xff99a4
        } else {
            0xffffff
        };
        self.text_at(&self.status, &f.small, STATUS, status_color, 0.6);
        let labels: &[(&str, f32, f32)] = if self.places.is_empty() {
            &FIELD_LABELS
        } else {
            &[]
        };
        for &(label, x, y) in labels {
            self.text_at(
                label,
                &f.small,
                Rect::new(x, y, x + 234.0, y + 18.0),
                0xffffff,
                0.75,
            );
        }
        self.preview_card();
    }

    /// Headed cards of rows, with a thin line between rows.
    unsafe fn page_groups(&self, page: &'static Page) {
        let f = &self.fonts;
        for (index, group) in page.groups.iter().enumerate() {
            let top = group_top(page, index);
            let heading = Rect::new(LEFT + 2.0, top, RIGHT, top + HEADING - 6.0);
            self.text_at(group.title, &f.section, heading, 0xffffff, 0.7);
            let bottom = top + HEADING + group.rows.len() as f32 * ROW;
            self.rounded(
                Rect::new(LEFT, top + HEADING, RIGHT, bottom),
                10.0,
                SURFACE,
                1.0,
            );
        }
        for (rect, row) in rows(page) {
            if rect.bottom < group_end(page, rect.bottom) {
                self.brush.SetColor(&rgb(BORDER, 1.0));
                self.target.FillRectangle(
                    &Rect::new(rect.left + 16.0, rect.bottom, rect.right, rect.bottom + 1.0).d2d(),
                    &self.brush,
                );
            }
            let (title, note, text_right) = match row {
                Row::Switch { title, note, .. } => (*title, *note, rect.right - 72.0),
                Row::Field {
                    title, note, width, ..
                } => (*title, *note, rect.right - 32.0 - width),
                Row::Size { title, note } => {
                    let width = SIZE_WIDTH * SIZES.len() as f32;
                    (*title, *note, rect.right - 32.0 - width)
                }
                Row::Zone { title, note } => (*title, *note, zone_segment(rect, 0).left - 8.0),
                Row::Hide { title, note } => (*title, *note, hide_segment(rect, 0).left - 8.0),
                Row::Style { title, note } => (*title, *note, style_segment(rect, 0).left - 8.0),
            };
            let title_box = Rect::new(
                rect.left + 16.0,
                rect.top + 5.0,
                text_right,
                rect.top + 25.0,
            );
            let note_box = Rect::new(
                title_box.left,
                title_box.bottom,
                text_right,
                rect.bottom - 5.0,
            );
            self.text_at(title, &f.body, title_box, 0xffffff, 0.95);
            if let Row::Style { .. } = row {
                let track = Rect {
                    right: style_segment(rect, STYLES.len() - 1).right,
                    ..style_segment(rect, 0)
                };
                self.rounded(track, 8.0, FIELD, 1.0);
                for (index, (label, floating)) in STYLES.iter().enumerate() {
                    let segment = style_segment(rect, index);
                    if *floating == self.notch_floating {
                        self.rounded(segment, 8.0, 0x5a5a5a, 1.0);
                    }
                    self.text_at(label, &f.button, segment, 0xffffff, 0.95);
                }
            }
            if let Row::Hide { .. } = row {
                let track = Rect {
                    right: hide_segment(rect, HIDES.len() - 1).right,
                    ..hide_segment(rect, 0)
                };
                self.rounded(track, 8.0, FIELD, 1.0);
                for (index, (label, hide)) in HIDES.iter().enumerate() {
                    let segment = hide_segment(rect, index);
                    if *hide == self.auto_hide {
                        self.rounded(segment, 8.0, 0x5a5a5a, 1.0);
                    }
                    self.text_at(label, &f.button, segment, 0xffffff, 0.95);
                }
            }
            if let Row::Zone { .. } = row {
                let track = Rect {
                    right: zone_segment(rect, ZONES.len() - 1).right,
                    ..zone_segment(rect, 0)
                };
                self.rounded(track, 8.0, FIELD, 1.0);
                for (index, (label, zone)) in ZONES.iter().enumerate() {
                    let segment = zone_segment(rect, index);
                    if *zone == self.second_zone {
                        self.rounded(segment, 8.0, 0x5a5a5a, 1.0);
                    }
                    self.text_at(label, &f.button, segment, 0xffffff, 0.95);
                }
            }
            if let Row::Size { .. } = row {
                let track = Rect {
                    right: size_segment(rect, SIZES.len() - 1).right,
                    ..size_segment(rect, 0)
                };
                self.rounded(track, 8.0, FIELD, 1.0);
                for (index, (label, size)) in SIZES.iter().enumerate() {
                    let segment = size_segment(rect, index);
                    if *size == self.notch_size {
                        self.rounded(segment, 8.0, 0x5a5a5a, 1.0);
                    }
                    self.text_at(label, &f.button, segment, 0xffffff, 0.95);
                }
            }
            if let Row::Switch { toggle, .. } = row {
                let minutes = toggle
                    .checked_sub(FIRST_HEALTH)
                    .filter(|&i| i < self.health_minutes.len())
                    .map(|i| self.health_minutes[i]);
                match minutes {
                    Some(minutes) => self.text_at(
                        &format!("{note}, setiap {minutes} menit"),
                        &f.small,
                        note_box,
                        0xffffff,
                        0.55,
                    ),
                    None => self.text_at(note, &f.small, note_box, 0xffffff, 0.55),
                }
                self.toggle(
                    Rect::new(rect.left, rect.top, rect.right - 16.0, rect.bottom),
                    self.toggles[*toggle],
                );
            } else {
                self.text_at(note, &f.small, note_box, 0xffffff, 0.55);
            }
        }
    }

    /// The Halaman page: every card page in its order, each with buttons to
    /// move it and a switch to show it.
    unsafe fn order_list(&self) {
        let f = &self.fonts;
        let count = self.page_order.len();
        let heading = Rect::new(LEFT + 2.0, CONTENT_TOP, RIGHT, CONTENT_TOP + HEADING - 6.0);
        self.text_at(
            "Urutan di kartu, dari kiri ke kanan",
            &f.section,
            heading,
            0xffffff,
            0.7,
        );
        let card = Rect {
            bottom: order_row(count - 1).bottom,
            ..order_row(0)
        };
        self.rounded(card, 10.0, SURFACE, 1.0);
        for (index, &page) in self.page_order.iter().enumerate() {
            let row = order_row(index);
            if index + 1 < count {
                self.brush.SetColor(&rgb(BORDER, 1.0));
                self.target.FillRectangle(
                    &Rect::new(row.left + 50.0, row.bottom, row.right, row.bottom + 1.0).d2d(),
                    &self.brush,
                );
            }
            let shown = !self.pages_hidden.contains(&page);
            let (note, ready) = self.order_note(page);
            let dim = if shown { 1.0 } else { 0.4 };
            let cy = (row.top + row.bottom) / 2.0;
            let tile = Rect::new(row.left + 14.0, cy - 12.0, row.left + 38.0, cy + 12.0);
            self.rounded(tile, 6.0, card_color(page), dim);
            self.text_at(render::page_glyph(page), &f.glyph, tile, 0xffffff, dim);
            let text_right = order_button(row, false).left - 8.0;
            let title = Rect::new(tile.right + 12.0, row.top + 4.0, text_right, cy + 1.0);
            let note_box = Rect::new(title.left, cy + 1.0, text_right, row.bottom - 4.0);
            self.text_at(page.name(), &f.body, title, 0xffffff, 0.95 * dim);
            let note_color = if ready { 0xffffff } else { 0xffb340 };
            let note_alpha = if ready { 0.55 } else { 0.85 };
            self.text_at(note, &f.small, note_box, note_color, note_alpha * dim);
            for (down, glyph, enabled) in [
                (false, "\u{E70E}", index > 0),
                (true, "\u{E70D}", index + 1 < count),
            ] {
                let button = order_button(row, down);
                if enabled {
                    self.rounded(button, 6.0, FIELD, 1.0);
                }
                let alpha = if enabled { 0.9 } else { 0.2 };
                self.text_at(glyph, &f.glyph, button, 0xffffff, alpha);
            }
            if page != CardPage::Status {
                self.toggle(
                    Rect::new(row.left, row.top, row.right - 16.0, row.bottom),
                    shown,
                );
            }
        }
    }

    /// The About page: the name, the version running, and the update check.
    unsafe fn about(&self) {
        let f = &self.fonts;
        self.rounded(ABOUT_CARD, 10.0, SURFACE, 1.0);
        let left = ABOUT_CARD.left + 16.0;
        let text_right = UPDATE_BUTTON.left - 12.0;
        self.text_at(
            "Drop",
            &f.time,
            Rect::new(left, 62.0, text_right, 84.0),
            0xffffff,
            1.0,
        );
        let version = format!("Versi {}", update::CURRENT);
        self.text_at(
            &version,
            &f.small,
            Rect::new(left, 84.0, text_right, 102.0),
            0xffffff,
            0.6,
        );
        let label = match &self.update {
            UpdateCheck::Checking => "Mengecek…".to_string(),
            UpdateCheck::Newer(version) => format!("Unduh {version}"),
            _ => "Cek pembaruan".to_string(),
        };
        let newer = matches!(self.update, UpdateCheck::Newer(_));
        self.button(&label, UPDATE_BUTTON, newer);
        let (status, color, alpha) = match &self.update {
            UpdateCheck::NotYet => (
                "Cek manual, Drop tidak mengecek sendiri".to_string(),
                0xffffff,
                0.55,
            ),
            UpdateCheck::Checking => ("Bertanya ke GitHub…".to_string(), 0xffffff, 0.55),
            UpdateCheck::Latest => ("Sudah versi terbaru".to_string(), SWITCH_ON, 1.0),
            UpdateCheck::Newer(version) => (
                format!("Versi {version} tersedia, unduh dari GitHub"),
                ACCENT,
                1.0,
            ),
            UpdateCheck::Failed => ("Gagal mengecek, coba lagi nanti".to_string(), 0xff99a4, 1.0),
        };
        self.text_at(
            &status,
            &f.small,
            Rect::new(left, 112.0, ABOUT_CARD.right - 16.0, 130.0),
            color,
            alpha,
        );
        self.text_at(
            "github.com/rifqihilal/drop",
            &f.small,
            Rect::new(left, 130.0, ABOUT_CARD.right - 16.0, 148.0),
            0xffffff,
            0.4,
        );
    }

    unsafe fn preview_card(&self) {
        self.rounded(PREVIEW, 8.0, SURFACE, 1.0);
        let f = &self.fonts;
        let inner = Rect::new(
            PREVIEW.left + 14.0,
            PREVIEW.top + 8.0,
            PREVIEW.right - 14.0,
            PREVIEW.top + 28.0,
        );
        self.text_at(
            "Jadwal hari ini (Kemenag RI)",
            &f.small,
            inner,
            0xffffff,
            0.6,
        );
        let Some(times) = self.preview else {
            let rest = Rect::new(inner.left, inner.bottom, inner.right, PREVIEW.bottom - 8.0);
            self.text_at(
                "Isi lintang dan bujur, atau cari lokasi di atas",
                &f.body,
                rest,
                0xffffff,
                0.75,
            );
            return;
        };
        let column = (PREVIEW.right - PREVIEW.left - 28.0) / 5.0;
        for (i, (name, time)) in prayer::NAMES.iter().zip(times).enumerate() {
            let left = inner.left + i as f32 * column;
            let name_box = Rect::new(left, inner.bottom + 2.0, left + column, inner.bottom + 22.0);
            let time_box = Rect::new(left, name_box.bottom, left + column, name_box.bottom + 26.0);
            self.text_at(name, &f.small, name_box, 0xffffff, 0.6);
            self.text_at(&prayer::format_time(time), &f.time, time_box, 0xffffff, 1.0);
        }
    }

    /// Search results as a dark dropdown: place name over its region.
    unsafe fn results_dropdown(&self) {
        if self.places.is_empty() {
            return;
        }
        let panel = self.results_panel();
        self.rounded(panel, 8.0, BORDER, 1.0);
        let inner = Rect::new(
            panel.left + 1.0,
            panel.top + 1.0,
            panel.right - 1.0,
            panel.bottom - 1.0,
        );
        self.rounded(inner, 7.0, 0x2b2b2b, 1.0);
        for (index, place) in self.places.iter().enumerate() {
            let row = self.result_rect(index);
            if index == self.hover {
                self.rounded(row, 6.0, 0x3a3a3a, 1.0);
            }
            let name = Rect::new(
                row.left + 12.0,
                row.top + 2.0,
                row.right - 12.0,
                row.top + 20.0,
            );
            let region = Rect::new(name.left, name.bottom, name.right, row.bottom - 3.0);
            self.text_at(&place.name, &self.fonts.button_left, name, 0xffffff, 1.0);
            self.text_at(&place.region, &self.fonts.small, region, 0xffffff, 0.6);
        }
    }

    unsafe fn field_background(&self, rect: Rect, focused: bool) {
        self.rounded(rect, 6.0, FIELD, 1.0);
        if focused {
            // Windows 11 marks the focused field with an accent underline.
            let line = Rect::new(
                rect.left + 1.0,
                rect.bottom - 2.0,
                rect.right - 1.0,
                rect.bottom,
            );
            self.brush.SetColor(&rgb(ACCENT, 1.0));
            self.target.FillRectangle(&line.d2d(), &self.brush);
        }
    }

    unsafe fn button(&self, label: &str, rect: Rect, primary: bool) {
        let (fill, text) = if primary {
            (ACCENT, 0x0b0b0b)
        } else {
            (0x3a3a3a, 0xffffff)
        };
        self.rounded(rect, 6.0, fill, 1.0);
        self.text_at(label, &self.fonts.button, rect, text, 1.0);
    }

    /// Windows 11 style switch at the right end of `row`.
    unsafe fn toggle(&self, row: Rect, on: bool) {
        let cy = (row.top + row.bottom) / 2.0;
        let track = Rect::new(row.right - 40.0, cy - 10.0, row.right, cy + 10.0);
        let knob_x;
        if on {
            self.rounded(track, 10.0, SWITCH_ON, 1.0);
            knob_x = track.right - 10.0;
            self.brush.SetColor(&rgb(0xffffff, 1.0));
        } else {
            self.brush.SetColor(&rgb(0x9a9a9a, 1.0));
            self.target.DrawRoundedRectangle(
                &D2D1_ROUNDED_RECT {
                    rect: Rect::new(
                        track.left + 0.5,
                        track.top + 0.5,
                        track.right - 0.5,
                        track.bottom - 0.5,
                    )
                    .d2d(),
                    radiusX: 9.5,
                    radiusY: 9.5,
                },
                &self.brush,
                1.0,
                None,
            );
            knob_x = track.left + 10.0;
            self.brush.SetColor(&rgb(0xcfcfcf, 1.0));
        }
        let radius = if on { 7.0 } else { 6.0 };
        self.target.FillEllipse(
            &D2D1_ELLIPSE {
                point: windows_numerics::Vector2 { X: knob_x, Y: cy },
                radiusX: radius,
                radiusY: radius,
            },
            &self.brush,
        );
    }

    /// Drop's eyes and the window's name, and a round close button.
    unsafe fn title_bar(&self) {
        let (cx, cy, radius) = CLOSE;
        self.brush.SetColor(&rgb(SIDEBAR_FILL, 1.0));
        self.target
            .FillRectangle(&Rect::new(0.0, -TITLE_BAR, SIDEBAR, 0.0).d2d(), &self.brush);
        self.rounded(
            Rect::new(14.0, cy - 8.0, 30.0, cy + 8.0),
            4.0,
            0x000000,
            1.0,
        );
        for left in [18.0, 23.0] {
            let eye = Rect::new(left, cy - 4.5, left + 3.0, cy + 4.5);
            self.rounded(eye, 1.5, 0xffffff, 1.0);
        }
        let name = Rect::new(38.0, cy - 10.0, SIDEBAR, cy + 10.0);
        self.text_at("Drop Settings", &self.fonts.small, name, 0xffffff, 0.8);

        let fill = if self.close_hover { CLOSE_RED } else { FIELD };
        self.brush.SetColor(&rgb(fill, 1.0));
        let point = |x: f32, y: f32| windows_numerics::Vector2 { X: x, Y: y };
        self.target.FillEllipse(
            &D2D1_ELLIPSE {
                point: point(cx, cy),
                radiusX: radius,
                radiusY: radius,
            },
            &self.brush,
        );
        self.brush.SetColor(&rgb(0xffffff, 0.95));
        let arm = 3.5;
        for side in [-1.0, 1.0] {
            self.target.DrawLine(
                point(cx - arm, cy - side * arm),
                point(cx + arm, cy + side * arm),
                &self.brush,
                1.4,
                None,
            );
        }
    }

    unsafe fn rounded(&self, rect: Rect, radius: f32, color: u32, alpha: f32) {
        self.brush.SetColor(&rgb(color, alpha));
        self.target.FillRoundedRectangle(
            &D2D1_ROUNDED_RECT {
                rect: rect.d2d(),
                radiusX: radius,
                radiusY: radius,
            },
            &self.brush,
        );
    }

    unsafe fn text_at(
        &self,
        text: &str,
        format: &IDWriteTextFormat,
        rect: Rect,
        color: u32,
        alpha: f32,
    ) {
        let wide: Vec<u16> = text.encode_utf16().collect();
        self.brush.SetColor(&rgb(color, alpha));
        self.target.DrawText(
            &wide,
            format,
            &rect.d2d(),
            &self.brush,
            D2D1_DRAW_TEXT_OPTIONS_NONE,
            DWRITE_MEASURING_MODE_NATURAL,
        );
    }
}

impl Drop for Window {
    fn drop(&mut self) {
        unsafe {
            let _ = DeleteObject(self.edit_font.into());
            let _ = DeleteObject(self.field_brush.into());
        }
        // Keep the factory alive until the render target is gone.
        let _ = &self.factory;
    }
}

impl Fonts {
    unsafe fn new() -> Result<Self> {
        let dwrite: IDWriteFactory = DWriteCreateFactory(DWRITE_FACTORY_TYPE_SHARED)?;
        let make_in = |family: windows::core::PCWSTR,
                       weight: DWRITE_FONT_WEIGHT,
                       size: f32,
                       align: DWRITE_TEXT_ALIGNMENT| {
            let format = dwrite.CreateTextFormat(
                family,
                None,
                weight,
                DWRITE_FONT_STYLE_NORMAL,
                DWRITE_FONT_STRETCH_NORMAL,
                size,
                w!("id-id"),
            )?;
            format.SetTextAlignment(align)?;
            format.SetParagraphAlignment(DWRITE_PARAGRAPH_ALIGNMENT_CENTER)?;
            format.SetWordWrapping(DWRITE_WORD_WRAPPING_NO_WRAP)?;
            Ok::<_, windows::core::Error>(format)
        };
        let make = |weight: DWRITE_FONT_WEIGHT, size: f32, align: DWRITE_TEXT_ALIGNMENT| {
            make_in(w!("Segoe UI Variable Text"), weight, size, align)
        };
        let lead = DWRITE_TEXT_ALIGNMENT_LEADING;
        Ok(Self {
            title: make(DWRITE_FONT_WEIGHT_SEMI_BOLD, 18.0, lead)?,
            section: make(DWRITE_FONT_WEIGHT_SEMI_BOLD, 12.0, lead)?,
            body: make(DWRITE_FONT_WEIGHT_NORMAL, 13.0, lead)?,
            small: make(DWRITE_FONT_WEIGHT_NORMAL, 11.5, lead)?,
            button: make(
                DWRITE_FONT_WEIGHT_SEMI_BOLD,
                13.0,
                DWRITE_TEXT_ALIGNMENT_CENTER,
            )?,
            button_left: make(DWRITE_FONT_WEIGHT_SEMI_BOLD, 13.0, lead)?,
            time: make(DWRITE_FONT_WEIGHT_SEMI_BOLD, 15.0, lead)?,
            glyph: make_in(
                w!("Segoe Fluent Icons"),
                DWRITE_FONT_WEIGHT_NORMAL,
                14.0,
                DWRITE_TEXT_ALIGNMENT_CENTER,
            )?,
        })
    }
}

unsafe extern "system" fn wndproc(hwnd: HWND, msg: u32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
    match msg {
        WM_PAINT => {
            let mut paint = PAINTSTRUCT::default();
            let hdc = BeginPaint(hwnd, &mut paint);
            with_window(|window| window.paint(hdc));
            let _ = EndPaint(hwnd, &paint);
            LRESULT(0)
        }
        // Painting covers the whole client area.
        WM_ERASEBKGND => LRESULT(1),
        WM_CTLCOLOREDIT => {
            let hdc = HDC(wparam.0 as *mut c_void);
            SetTextColor(hdc, colorref(0xf2f2f2));
            SetBkColor(hdc, colorref(FIELD));
            let brush = with_window(|window| window.field_brush).unwrap_or_default();
            LRESULT(brush.0 as isize)
        }
        // The client area takes the whole window, the caption included.
        WM_NCCALCSIZE if wparam.0 != 0 => LRESULT(0),
        // The title bar moves the window, except on its close button.
        WM_NCHITTEST => {
            let mut point = POINT {
                x: lparam.0 as i16 as i32,
                y: (lparam.0 >> 16) as i16 as i32,
            };
            let _ = ScreenToClient(hwnd, &mut point);
            let on_bar = with_window(|window| {
                let (x, y) = (point.x as f32, point.y as f32);
                let (x, y) = (x / window.scale, y / window.scale - TITLE_BAR);
                y < 0.0 && !on_close(x, y)
            });
            LRESULT(if on_bar == Some(true) {
                HTCAPTION
            } else {
                HTCLIENT
            } as isize)
        }
        WM_NCMOUSEMOVE => {
            with_window(|window| window.hover_at(-1.0, -1.0));
            DefWindowProcW(hwnd, msg, wparam, lparam)
        }
        WM_LBUTTONUP => {
            let (x, y) = (lparam.0 as i16 as f32, (lparam.0 >> 16) as i16 as f32);
            with_window(|window| {
                window.click(x / window.scale, y / window.scale - TITLE_BAR);
            });
            LRESULT(0)
        }
        WM_MOUSEMOVE => {
            let (x, y) = (lparam.0 as i16 as f32, (lparam.0 >> 16) as i16 as f32);
            with_window(|window| {
                window.hover_at(x / window.scale, y / window.scale - TITLE_BAR);
            });
            LRESULT(0)
        }
        WM_MOUSEWHEEL => {
            let delta = (wparam.0 >> 16) as i16;
            with_window(|window| window.wheel(delta));
            LRESULT(0)
        }
        WM_SETCURSOR => {
            let mut point = POINT::default();
            let _ = GetCursorPos(&mut point);
            let _ = ScreenToClient(hwnd, &mut point);
            let over = with_window(|window| {
                let (x, y) = (point.x as f32, point.y as f32);
                window.clickable(x / window.scale, y / window.scale - TITLE_BAR)
            });
            if over == Some(true) && (lparam.0 & 0xffff) as i32 == 1 {
                if let Ok(hand) = LoadCursorW(None, IDC_HAND) {
                    SetCursor(Some(hand));
                    return LRESULT(1);
                }
            }
            DefWindowProcW(hwnd, msg, wparam, lparam)
        }
        WM_COMMAND => {
            let id = wparam.0 & 0xffff;
            let code = (wparam.0 >> 16) as u32;
            with_window(|window| match (id, code) {
                // Enter: pick a result, search from the search box, otherwise
                // apply what is typed and close.
                (id, _) if id == IDOK.0 as usize => {
                    if !window.places.is_empty() {
                        window.pick_result(window.hover);
                    } else if window.focused == Some(ID_SEARCH) {
                        window.start_search();
                    } else if window.commit() {
                        window.close();
                    }
                }
                (id, _) if id == IDCANCEL.0 as usize => {
                    if window.places.is_empty() {
                        window.close();
                    } else {
                        window.hide_results();
                    }
                }
                (id, EN_SETFOCUS) => {
                    window.focused = Some(id);
                    window.invalidate();
                }
                (_, EN_KILLFOCUS) => {
                    window.focused = None;
                    window.apply_live();
                    window.invalidate();
                }
                (ID_LATITUDE | ID_LONGITUDE | ID_ELEVATION, EN_CHANGE) => window.update_preview(),
                _ => {}
            });
            LRESULT(0)
        }
        UPDATE_DONE_MSG => {
            let latest = *Box::from_raw(lparam.0 as *mut Option<String>);
            with_window(|window| window.show_update(latest));
            LRESULT(0)
        }
        SEARCH_DONE_MSG => {
            let result = *Box::from_raw(lparam.0 as *mut std::result::Result<Vec<Place>, String>);
            with_window(|window| window.show_results(result));
            LRESULT(0)
        }
        WM_CLOSE => {
            // A field typed just before the window closed still counts.
            with_window(|window| window.apply_live());
            let _ = DestroyWindow(hwnd);
            LRESULT(0)
        }
        WM_DESTROY => {
            // Taken out first so nothing re-enters a half-destroyed window.
            let window = WINDOW.with(|cell| cell.try_borrow_mut().ok().and_then(|mut w| w.take()));
            drop(window);
            LRESULT(0)
        }
        _ => DefWindowProcW(hwnd, msg, wparam, lparam),
    }
}

/// Whether a point is on the close button.
fn on_close(x: f32, y: f32) -> bool {
    let (cx, cy, radius) = CLOSE;
    (x - cx).powi(2) + (y - cy).powi(2) <= (radius + 2.0).powi(2)
}

/// Creates a child control at a logical rect.
unsafe fn create_child(
    parent: HWND,
    class: windows::core::PCWSTR,
    id: usize,
    style: WINDOW_STYLE,
    rect: Rect,
    scale: f32,
) -> Result<HWND> {
    CreateWindowExW(
        WINDOW_EX_STYLE(0),
        class,
        None,
        style,
        (rect.left * scale).round() as i32,
        ((rect.top + TITLE_BAR) * scale).round() as i32,
        ((rect.right - rect.left) * scale).round() as i32,
        ((rect.bottom - rect.top) * scale).round() as i32,
        Some(parent),
        Some(HMENU(id as *mut c_void)),
        Some(GetModuleHandleW(None)?.into()),
        None,
    )
}

/// The edit control sits inside its drawn field, inset and vertically centered.
fn edit_rect(field: Rect) -> Rect {
    let cy = (field.top + field.bottom) / 2.0;
    Rect::new(field.left + 10.0, cy - 10.0, field.right - 10.0, cy + 10.0)
}

fn whole_number(text: &str, min: u32, max: u32) -> Option<u32> {
    text.trim()
        .parse::<u32>()
        .ok()
        .filter(|v| (min..=max).contains(v))
}

fn rgb(color: u32, alpha: f32) -> D2D1_COLOR_F {
    D2D1_COLOR_F {
        r: ((color >> 16) & 0xff) as f32 / 255.0,
        g: ((color >> 8) & 0xff) as f32 / 255.0,
        b: (color & 0xff) as f32 / 255.0,
        a: alpha,
    }
}

/// GDI colours are 0x00BBGGRR.
fn colorref(color: u32) -> COLORREF {
    COLORREF(((color & 0xff) << 16) | (color & 0xff00) | ((color >> 16) & 0xff))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn switches() -> Vec<usize> {
        PAGES
            .iter()
            .flat_map(rows)
            .filter_map(|(_, row)| match row {
                Row::Switch { toggle, .. } => Some(*toggle),
                Row::Field { .. }
                | Row::Size { .. }
                | Row::Zone { .. }
                | Row::Hide { .. }
                | Row::Style { .. } => None,
            })
            .collect()
    }

    #[test]
    fn every_switch_is_on_one_page() {
        let mut found = switches();
        found.sort_unstable();
        assert_eq!(found, (0..TOGGLE_COUNT).collect::<Vec<_>>());
    }

    #[test]
    fn fields_land_on_their_pages() {
        assert_eq!(PAGES[PAGE_PRAYER].title, "Sholat");
        assert_eq!(field_place(ID_REMIND).0, PAGE_PRAYER);
        assert_eq!(field_place(ID_IDLE).0, PAGE_GENERAL);
        assert!(field_place(ID_ICAL).1.right > 0.0);
    }

    #[test]
    fn page_list_fits_without_scrolling() {
        assert_eq!(PAGES[PAGE_ORDER].title, "Halaman");
        assert_eq!(PAGES[PAGE_ABOUT].title, "About");
        assert!(order_row(CardPage::ALL.len() - 1).bottom + 16.0 <= FOOTER_TOP);
        let row = order_row(0);
        assert!(order_button(row, false).right < order_button(row, true).left);
    }

    #[test]
    fn nav_stays_in_the_window() {
        assert!(nav_row(PAGES.len() - 1).bottom <= CLIENT.1);
    }
}
