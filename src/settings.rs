//! The settings window: prayer location with place search and a live preview
//! of today's times, notification switches and general options.
//!
//! Text fields are native EDIT controls (for typing, selection and IME);
//! everything else is drawn with Direct2D in Windows 11's dark style. The
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
    D2D1_DRAW_TEXT_OPTIONS_NONE, D2D1_ELLIPSE, D2D1_FACTORY_TYPE_SINGLE_THREADED,
    D2D1_FEATURE_LEVEL_DEFAULT, D2D1_RENDER_TARGET_PROPERTIES, D2D1_RENDER_TARGET_TYPE_SOFTWARE,
    D2D1_RENDER_TARGET_USAGE_NONE, D2D1_ROUNDED_RECT,
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
};
use windows::Win32::Graphics::Dxgi::Common::DXGI_FORMAT_B8G8R8A8_UNORM;
use windows::Win32::Graphics::Gdi::{
    BeginPaint, CreateFontW, CreateSolidBrush, DeleteObject, EndPaint, InvalidateRect,
    ScreenToClient, SetBkColor, SetTextColor, CLEARTYPE_QUALITY, CLIP_DEFAULT_PRECIS,
    DEFAULT_CHARSET, HBRUSH, HDC, HFONT, OUT_DEFAULT_PRECIS, PAINTSTRUCT,
};
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::UI::HiDpi::{AdjustWindowRectExForDpi, GetDpiForSystem};
use windows::Win32::UI::WindowsAndMessaging::{
    CreateWindowExW, DefWindowProcW, DestroyWindow, GetClientRect, GetCursorPos,
    GetWindowTextLengthW, GetWindowTextW, IsDialogMessageW, LoadCursorW, PostMessageW,
    RegisterClassExW, SendMessageW, SetCursor, SetForegroundWindow, SetWindowTextW, ShowWindow,
    SystemParametersInfoW, EN_CHANGE, EN_KILLFOCUS, EN_SETFOCUS, ES_AUTOHSCROLL, HMENU, IDCANCEL,
    IDC_ARROW, IDC_HAND, IDOK, MSG, SPI_GETWORKAREA, SW_HIDE, SW_SHOW, SW_SHOWNORMAL,
    SYSTEM_PARAMETERS_INFO_UPDATE_FLAGS, WINDOW_EX_STYLE, WINDOW_STYLE, WM_APP, WM_CLOSE,
    WM_COMMAND, WM_CTLCOLOREDIT, WM_DESTROY, WM_ERASEBKGND, WM_LBUTTONUP, WM_MOUSEMOVE, WM_PAINT,
    WM_SETCURSOR, WM_SETFONT, WNDCLASSEXW, WS_CAPTION, WS_CHILD, WS_CLIPCHILDREN, WS_MINIMIZEBOX,
    WS_OVERLAPPED, WS_SYSMENU, WS_TABSTOP, WS_VISIBLE,
};

use crate::config::{self, Settings};
use crate::geo::{self, Place};
use crate::{autostart, prayer};

/// Sent to the notch window after the settings were saved.
pub const SAVED_MSG: u32 = WM_APP + 6;
/// Place search finished. LPARAM: `Box<Result<Vec<Place>, String>>`.
const SEARCH_DONE_MSG: u32 = WM_APP + 7;

const CLIENT: (f32, f32) = (760.0, 690.0);

// Child control IDs.
const ID_SEARCH: usize = 101;
const ID_CITY: usize = 102;
const ID_LATITUDE: usize = 103;
const ID_LONGITUDE: usize = 104;
const ID_ELEVATION: usize = 105;
const ID_REMIND: usize = 106;
const ID_IDLE: usize = 107;

// Layout, in logical pixels.
const SEARCH_FIELD: Rect = Rect::new(24.0, 90.0, 324.0, 124.0);
const SEARCH_BUTTON: Rect = Rect::new(332.0, 90.0, 424.0, 124.0);
const STATUS: Rect = Rect::new(24.0, 128.0, 424.0, 148.0);
/// Labels above the prayer fields: text and top-left corner.
const FIELD_LABELS: [(&str, f32, f32); 5] = [
    ("Nama lokasi", 24.0, 150.0),
    ("Lintang", 24.0, 214.0),
    ("Bujur", 232.0, 214.0),
    ("Ketinggian (meter)", 24.0, 278.0),
    ("Ingatkan sebelum (menit)", 232.0, 278.0),
];
/// Search results drop down from under the search box, over these fields.
const RESULTS_TOP: f32 = 128.0;
const RESULT_ROW: f32 = 44.0;
const COVERED_FIELDS: [usize; 5] = [ID_CITY, ID_LATITUDE, ID_LONGITUDE, ID_ELEVATION, ID_REMIND];
const CITY_FIELD: Rect = Rect::new(24.0, 172.0, 424.0, 206.0);
const LATITUDE_FIELD: Rect = Rect::new(24.0, 236.0, 216.0, 270.0);
const LONGITUDE_FIELD: Rect = Rect::new(232.0, 236.0, 424.0, 270.0);
const ELEVATION_FIELD: Rect = Rect::new(24.0, 300.0, 216.0, 334.0);
const REMIND_FIELD: Rect = Rect::new(232.0, 300.0, 424.0, 334.0);
const PREVIEW: Rect = Rect::new(24.0, 354.0, 424.0, 444.0);
const RIGHT: (f32, f32) = (456.0, 736.0);
const TOGGLE_TOP: f32 = 90.0;
const ROW: f32 = 40.0;
const GENERAL_TOP: f32 = 324.0;
const IDLE_FIELD: Rect = Rect::new(672.0, 367.0, 736.0, 401.0);
const CANCEL_BUTTON: Rect = Rect::new(526.0, 640.0, 626.0, 674.0);
const SAVE_BUTTON: Rect = Rect::new(636.0, 640.0, 736.0, 674.0);

/// Switches: five notifications, start with Windows, then six modules.
const TOGGLES: [&str; 12] = [
    "Wi-Fi tersambung dan terputus",
    "Perangkat Bluetooth",
    "Baterai lemah",
    "Pengingat sholat",
    "Bunyi notifikasi",
    "Jalankan saat Windows menyala",
    "Cuaca",
    "Musik",
    "Kalender & Pomodoro",
    "Shelf",
    "Halaman sholat",
    "Mata saat idle",
];
/// First switch of the modules grid.
const FIRST_MODULE: usize = 6;
/// Top of the modules grid, two switches per row under the prayer preview.
const MODULES_TOP: f32 = 494.0;

// Windows 11 dark palette, as 0xRRGGBB.
const BACKGROUND: u32 = 0x202020;
const SURFACE: u32 = 0x2d2d2d;
const BORDER: u32 = 0x3d3d3d;
const ACCENT: u32 = 0x4cc2ff;

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

/// Row `index` of the switches column (notifications, then general).
fn toggle_row(index: usize) -> Rect {
    if index >= FIRST_MODULE {
        let (column, row) = ((index - FIRST_MODULE) % 2, (index - FIRST_MODULE) / 2);
        let left = 24.0 + column as f32 * 208.0;
        let top = MODULES_TOP + row as f32 * ROW;
        return Rect::new(left, top, left + 192.0, top + ROW);
    }
    let top = if index < 5 {
        TOGGLE_TOP + index as f32 * ROW
    } else {
        GENERAL_TOP
    };
    Rect::new(RIGHT.0, top, RIGHT.1, top + ROW)
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
    fields: Vec<(usize, HWND, Rect)>,
    /// Search results while the dropdown is open.
    places: Vec<Place>,
    /// Highlighted result, picked by Enter.
    hover: usize,
    toggles: [bool; 12],
    status: String,
    status_is_error: bool,
    focused: Option<usize>,
    preview: Option<prayer::Schedule>,
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
        with_window(Window::update_preview);
        unsafe {
            let _ = ShowWindow(hwnd, SW_SHOWNORMAL);
            let _ = SetForegroundWindow(hwnd);
        }
    }
}

/// Gives the settings window keyboard navigation (Tab, Enter, Esc) when the
/// message is for it. Call from the message loop.
pub fn handle_dialog_message(msg: &MSG) -> bool {
    let hwnd = WINDOW.with(|cell| cell.try_borrow().ok()?.as_ref().map(|w| w.hwnd));
    match hwnd {
        Some(hwnd) => unsafe { IsDialogMessageW(hwnd, msg).as_bool() },
        None => false,
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
        let style = WS_OVERLAPPED | WS_CAPTION | WS_SYSMENU | WS_MINIMIZEBOX | WS_CLIPCHILDREN;
        let mut frame = RECT {
            left: 0,
            top: 0,
            right: (CLIENT.0 * scale).round() as i32,
            bottom: (CLIENT.1 * scale).round() as i32,
        };
        AdjustWindowRectExForDpi(&mut frame, style, false, WINDOW_EX_STYLE(0), dpi)?;
        let (width, height) = (frame.right - frame.left, frame.bottom - frame.top);
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
            w!("Pengaturan Drop"),
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
            -(14.0 * scale).round() as i32,
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
        let field_brush = CreateSolidBrush(colorref(SURFACE));

        let settings = Settings::load();
        let number = |value: Option<f64>| value.map(|v| v.to_string()).unwrap_or_default();
        let field_specs = [
            (ID_SEARCH, SEARCH_FIELD, String::new()),
            (ID_CITY, CITY_FIELD, settings.city.clone()),
            (ID_LATITUDE, LATITUDE_FIELD, number(settings.latitude)),
            (ID_LONGITUDE, LONGITUDE_FIELD, number(settings.longitude)),
            (
                ID_ELEVATION,
                ELEVATION_FIELD,
                settings.elevation.to_string(),
            ),
            (ID_REMIND, REMIND_FIELD, settings.remind_minutes.to_string()),
            (ID_IDLE, IDLE_FIELD, settings.idle_minutes.to_string()),
        ];
        let mut fields = Vec::new();
        for (id, rect, text) in field_specs {
            let edit = create_child(
                hwnd,
                w!("EDIT"),
                id,
                WS_CHILD | WS_VISIBLE | WS_TABSTOP | WINDOW_STYLE(ES_AUTOHSCROLL as u32),
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
            fields.push((id, edit, rect));
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
            ],
            status: "Cari nama kota, kabupaten atau desa".to_string(),
            status_is_error: false,
            focused: None,
            preview: None,
        })
    }

    fn field(&self, id: usize) -> HWND {
        self.fields
            .iter()
            .find(|(field, ..)| *field == id)
            .map(|(_, hwnd, _)| *hwnd)
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
        self.set_status("Mencari…", false);
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

    /// Opens the results dropdown over the fields below the search box. The
    /// fields under it are hidden meanwhile: child windows always draw on top
    /// of the parent's painting.
    fn show_results(&mut self, result: std::result::Result<Vec<Place>, String>) {
        match result {
            Ok(places) if !places.is_empty() => {
                self.places = places;
                self.hover = 0;
                self.set_status("", false);
                self.set_covered_fields_visible(false);
            }
            Ok(_) => self.set_status("Tempat tidak ditemukan; coba nama lain", true),
            Err(message) => self.set_status(&message, true),
        }
    }

    fn set_covered_fields_visible(&self, visible: bool) {
        let command = if visible { SW_SHOW } else { SW_HIDE };
        for (id, hwnd, _) in &self.fields {
            if COVERED_FIELDS.contains(id) {
                unsafe {
                    let _ = ShowWindow(*hwnd, command);
                }
            }
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
        self.set_covered_fields_visible(true);
        self.invalidate();
    }

    /// Validates the form; on success writes the file and closes.
    fn save(&mut self) {
        let latitude_text = self.text(ID_LATITUDE);
        let longitude_text = self.text(ID_LONGITUDE);
        let latitude = config::parse_number(&latitude_text);
        let longitude = config::parse_number(&longitude_text);
        let both_empty = latitude_text.is_empty() && longitude_text.is_empty();
        let coordinates_ok = both_empty
            || latitude.is_some_and(config::valid_latitude)
                && longitude.is_some_and(config::valid_longitude);
        if !coordinates_ok {
            self.set_status(
                "Lintang (-90 s/d 90) atau bujur (-180 s/d 180) tidak valid",
                true,
            );
            return;
        }
        let Some(remind) = whole_number(&self.text(ID_REMIND), 1, 60) else {
            self.set_status("Pengingat harus 1 sampai 60 menit", true);
            return;
        };
        let Some(idle) = whole_number(&self.text(ID_IDLE), 1, 120) else {
            self.set_status("Waktu munculnya mata harus 1 sampai 120 menit", true);
            return;
        };

        let settings = Settings {
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
            sound: self.toggles[4],
            idle_minutes: idle,
            weather: self.toggles[6],
            module_music: self.toggles[7],
            module_calendar: self.toggles[8],
            module_shelf: self.toggles[9],
            module_prayer: self.toggles[10],
            module_eyes: self.toggles[11],
        };
        if settings.save().is_err() {
            self.set_status("Pengaturan gagal disimpan", true);
            return;
        }
        if self.toggles[5] != autostart::is_enabled() {
            autostart::set_enabled(self.toggles[5]);
        }
        unsafe {
            let _ = PostMessageW(Some(self.notch), SAVED_MSG, WPARAM(0), LPARAM(0));
        }
        self.close();
    }

    fn click(&mut self, x: f32, y: f32) {
        // An open dropdown takes the click: a row picks it, anywhere else closes it.
        if !self.places.is_empty() && !SEARCH_BUTTON.contains(x, y) {
            match self.result_at(x, y) {
                Some(index) => self.pick_result(index),
                None if !self.results_panel().contains(x, y) => self.hide_results(),
                None => {}
            }
            return;
        }
        if SEARCH_BUTTON.contains(x, y) {
            self.start_search();
        } else if SAVE_BUTTON.contains(x, y) {
            self.save();
        } else if CANCEL_BUTTON.contains(x, y) {
            self.close();
        } else if let Some(index) = (0..TOGGLES.len()).find(|&i| toggle_row(i).contains(x, y)) {
            self.toggles[index] = !self.toggles[index];
            self.invalidate();
        }
    }

    fn clickable(&self, x: f32, y: f32) -> bool {
        if !self.places.is_empty() {
            return self.result_at(x, y).is_some() || SEARCH_BUTTON.contains(x, y);
        }
        [SEARCH_BUTTON, SAVE_BUTTON, CANCEL_BUTTON]
            .iter()
            .any(|r| r.contains(x, y))
            || (0..TOGGLES.len()).any(|i| toggle_row(i).contains(x, y))
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

        let f = &self.fonts;
        self.text_at(
            "Pengaturan",
            &f.title,
            Rect::new(24.0, 14.0, 424.0, 54.0),
            0xffffff,
            1.0,
        );

        // Prayer column.
        self.text_at(
            "Waktu sholat",
            &f.section,
            Rect::new(24.0, 60.0, 424.0, 84.0),
            0xffffff,
            0.7,
        );
        // Fields under an open dropdown are hidden, and so is their chrome.
        let dropdown = !self.places.is_empty();
        for (id, _, rect) in &self.fields {
            if !(dropdown && COVERED_FIELDS.contains(id)) {
                self.field_background(*rect, self.focused == Some(*id));
            }
        }
        self.button("Cari", SEARCH_BUTTON, false);
        let status_color = if self.status_is_error {
            0xff99a4
        } else {
            0xffffff
        };
        self.text_at(&self.status, &f.small, STATUS, status_color, 0.6);
        let labels: &[(&str, f32, f32)] = if dropdown { &[] } else { &FIELD_LABELS };
        for &(label, x, y) in labels {
            self.text_at(
                label,
                &f.small,
                Rect::new(x, y, x + 192.0, y + 20.0),
                0xffffff,
                0.75,
            );
        }
        self.preview_card();
        self.text_at(
            "Modul",
            &f.section,
            Rect::new(24.0, MODULES_TOP - 30.0, 424.0, MODULES_TOP - 6.0),
            0xffffff,
            0.7,
        );
        self.results_dropdown();

        // Switches column.
        self.text_at(
            "Notifikasi",
            &f.section,
            Rect::new(RIGHT.0, 60.0, RIGHT.1, 84.0),
            0xffffff,
            0.7,
        );
        self.text_at(
            "Umum",
            &f.section,
            Rect::new(RIGHT.0, GENERAL_TOP - 30.0, RIGHT.1, GENERAL_TOP - 6.0),
            0xffffff,
            0.7,
        );
        for (index, label) in TOGGLES.iter().enumerate() {
            let row = toggle_row(index);
            self.text_at(
                label,
                &f.body,
                Rect::new(row.left, row.top, row.right - 56.0, row.bottom),
                0xffffff,
                0.95,
            );
            self.toggle(row, self.toggles[index]);
        }
        let idle_row = Rect::new(
            RIGHT.0,
            IDLE_FIELD.top,
            IDLE_FIELD.left - 8.0,
            IDLE_FIELD.bottom,
        );
        self.text_at(
            "Mata muncul setelah (menit)",
            &f.body,
            idle_row,
            0xffffff,
            0.95,
        );

        // Footer.
        self.brush.SetColor(&rgb(BORDER, 1.0));
        t.FillRectangle(&Rect::new(0.0, 624.0, CLIENT.0, 625.0).d2d(), &self.brush);
        self.button("Batal", CANCEL_BUTTON, false);
        self.button("Simpan", SAVE_BUTTON, true);

        let _ = t.EndDraw(None, None);
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
                row.top + 3.0,
                row.right - 12.0,
                row.top + 23.0,
            );
            let region = Rect::new(name.left, name.bottom, name.right, row.bottom - 3.0);
            self.text_at(&place.name, &self.fonts.button_left, name, 0xffffff, 1.0);
            self.text_at(&place.region, &self.fonts.small, region, 0xffffff, 0.6);
        }
    }

    unsafe fn field_background(&self, rect: Rect, focused: bool) {
        self.rounded(rect, 6.0, SURFACE, 1.0);
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
            self.rounded(track, 10.0, ACCENT, 1.0);
            knob_x = track.right - 10.0;
            self.brush.SetColor(&rgb(0x0b0b0b, 1.0));
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
        let make = |weight: DWRITE_FONT_WEIGHT, size: f32, align: DWRITE_TEXT_ALIGNMENT| {
            let format = dwrite.CreateTextFormat(
                w!("Segoe UI Variable Text"),
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
        let lead = DWRITE_TEXT_ALIGNMENT_LEADING;
        Ok(Self {
            title: make(DWRITE_FONT_WEIGHT_SEMI_BOLD, 22.0, lead)?,
            section: make(DWRITE_FONT_WEIGHT_SEMI_BOLD, 13.0, lead)?,
            body: make(DWRITE_FONT_WEIGHT_NORMAL, 14.0, lead)?,
            small: make(DWRITE_FONT_WEIGHT_NORMAL, 12.5, lead)?,
            button: make(
                DWRITE_FONT_WEIGHT_SEMI_BOLD,
                14.0,
                DWRITE_TEXT_ALIGNMENT_CENTER,
            )?,
            button_left: make(DWRITE_FONT_WEIGHT_SEMI_BOLD, 14.0, lead)?,
            time: make(DWRITE_FONT_WEIGHT_SEMI_BOLD, 16.0, lead)?,
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
            SetBkColor(hdc, colorref(SURFACE));
            let brush = with_window(|window| window.field_brush).unwrap_or_default();
            LRESULT(brush.0 as isize)
        }
        WM_LBUTTONUP => {
            let (x, y) = (lparam.0 as i16 as f32, (lparam.0 >> 16) as i16 as f32);
            with_window(|window| window.click(x / window.scale, y / window.scale));
            LRESULT(0)
        }
        WM_MOUSEMOVE => {
            let (x, y) = (lparam.0 as i16 as f32, (lparam.0 >> 16) as i16 as f32);
            with_window(|window| window.hover_at(x / window.scale, y / window.scale));
            LRESULT(0)
        }
        WM_SETCURSOR => {
            let mut point = POINT::default();
            let _ = GetCursorPos(&mut point);
            let _ = ScreenToClient(hwnd, &mut point);
            let over = with_window(|window| {
                window.clickable(point.x as f32 / window.scale, point.y as f32 / window.scale)
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
                // Enter: pick a result, search from the search box, otherwise save.
                (id, _) if id == IDOK.0 as usize => {
                    if !window.places.is_empty() {
                        window.pick_result(window.hover);
                    } else if window.focused == Some(ID_SEARCH) {
                        window.start_search();
                    } else {
                        window.save();
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
                    window.invalidate();
                }
                (ID_LATITUDE | ID_LONGITUDE | ID_ELEVATION, EN_CHANGE) => window.update_preview(),
                _ => {}
            });
            LRESULT(0)
        }
        SEARCH_DONE_MSG => {
            let result = *Box::from_raw(lparam.0 as *mut std::result::Result<Vec<Place>, String>);
            with_window(|window| window.show_results(result));
            LRESULT(0)
        }
        WM_CLOSE => {
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
        (rect.top * scale).round() as i32,
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
