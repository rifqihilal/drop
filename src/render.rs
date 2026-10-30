//! Draws the notch with Direct2D into a DIB and pushes it to the layered window.
//!
//! Rendering is software-only on purpose: the surface is tiny, and skipping a
//! Direct3D device keeps the GPU driver out of the process, which is most of
//! what makes a webview-based notch heavy.

use std::ffi::c_void;
use std::path::{Path, PathBuf};

use windows::core::{w, Result, HSTRING};
use windows::Win32::Foundation::{COLORREF, HWND, POINT, RECT, SIZE};
use windows::Win32::Graphics::Direct2D::Common::{
    D2D1_ALPHA_MODE_PREMULTIPLIED, D2D1_COLOR_F, D2D1_FIGURE_BEGIN_FILLED,
    D2D1_FIGURE_BEGIN_HOLLOW, D2D1_FIGURE_END_CLOSED, D2D1_FIGURE_END_OPEN, D2D1_GRADIENT_STOP,
    D2D1_PIXEL_FORMAT, D2D_RECT_F, D2D_SIZE_F,
};
use windows::Win32::Graphics::Direct2D::{
    D2D1CreateFactory, ID2D1Bitmap, ID2D1DCRenderTarget, ID2D1Factory, ID2D1LinearGradientBrush,
    ID2D1SolidColorBrush, ID2D1StrokeStyle, D2D1_ANTIALIAS_MODE_PER_PRIMITIVE, D2D1_ARC_SEGMENT,
    D2D1_ARC_SIZE_LARGE, D2D1_ARC_SIZE_SMALL, D2D1_BITMAP_INTERPOLATION_MODE_LINEAR,
    D2D1_CAP_STYLE_FLAT, D2D1_CAP_STYLE_ROUND, D2D1_DASH_STYLE_DASH, D2D1_DASH_STYLE_SOLID,
    D2D1_DRAW_TEXT_OPTIONS_NONE, D2D1_ELLIPSE, D2D1_EXTEND_MODE_CLAMP,
    D2D1_FACTORY_TYPE_SINGLE_THREADED, D2D1_FEATURE_LEVEL_DEFAULT, D2D1_GAMMA_2_2,
    D2D1_LINEAR_GRADIENT_BRUSH_PROPERTIES, D2D1_LINE_JOIN_ROUND, D2D1_QUADRATIC_BEZIER_SEGMENT,
    D2D1_RENDER_TARGET_PROPERTIES, D2D1_RENDER_TARGET_TYPE_SOFTWARE, D2D1_RENDER_TARGET_USAGE_NONE,
    D2D1_ROUNDED_RECT, D2D1_STROKE_STYLE_PROPERTIES, D2D1_SWEEP_DIRECTION_CLOCKWISE,
};
use windows::Win32::Graphics::DirectWrite::{
    DWriteCreateFactory, IDWriteFactory, IDWriteTextFormat, IDWriteTextLayout,
    DWRITE_FACTORY_TYPE_SHARED, DWRITE_FONT_STRETCH_NORMAL, DWRITE_FONT_STYLE_NORMAL,
    DWRITE_FONT_WEIGHT, DWRITE_FONT_WEIGHT_NORMAL, DWRITE_FONT_WEIGHT_SEMI_BOLD,
    DWRITE_FONT_WEIGHT_SEMI_LIGHT, DWRITE_MEASURING_MODE_NATURAL,
    DWRITE_PARAGRAPH_ALIGNMENT_CENTER, DWRITE_PARAGRAPH_ALIGNMENT_NEAR,
    DWRITE_TEXT_ALIGNMENT_CENTER, DWRITE_TEXT_ALIGNMENT_LEADING, DWRITE_TEXT_METRICS,
    DWRITE_TEXT_RANGE, DWRITE_TRIMMING, DWRITE_TRIMMING_GRANULARITY_CHARACTER,
    DWRITE_WORD_WRAPPING_NO_WRAP,
};
use windows::Win32::Graphics::Dxgi::Common::DXGI_FORMAT_B8G8R8A8_UNORM;
use windows::Win32::Graphics::Gdi::{
    CreateCompatibleDC, CreateDIBSection, DeleteDC, DeleteObject, SelectObject, AC_SRC_ALPHA,
    AC_SRC_OVER, BITMAPINFO, BITMAPINFOHEADER, BI_RGB, BLENDFUNCTION, DIB_RGB_COLORS, HBITMAP, HDC,
    HGDIOBJ, HPALETTE,
};
use windows::Win32::Graphics::Imaging::{
    CLSID_WICImagingFactory, GUID_WICPixelFormat32bppPBGRA, IWICImagingFactory,
    WICBitmapDitherTypeNone, WICBitmapInterpolationModeFant, WICBitmapPaletteTypeMedianCut,
    WICBitmapUsePremultipliedAlpha, WICDecodeMetadataCacheOnLoad, WICRect,
};
use windows::Win32::System::Com::{CoCreateInstance, CLSCTX_INPROC_SERVER};
use windows::Win32::UI::Shell::{
    IShellItemImageFactory, SHCreateItemFromParsingName, SHCreateMemStream, SIIGBF_RESIZETOFIT,
};
use windows::Win32::UI::WindowsAndMessaging::{UpdateLayeredWindow, ULW_ALPHA};

use windows_numerics::{Matrix3x2, Vector2};

use crate::anim::{
    Accent, Content, LevelKind, Notice, Trailing, AGENDA_ROWS, CARD, CARD_NAV, CARD_SIDE, CARD_TOP,
    DROP_ZONE, IDLE, LEVEL, NOTICE_HEIGHT, NOTICE_WIDTH, TODO_ROW,
};
use crate::media::Command;
use crate::todo::Task;
use crate::{calendar, prayer, shelf, todo};

/// A point on the open card (its `y`, and the card's `width`) as the page
/// sees it: below the header, between the side arrows.
fn on_page(y: f32, width: f32) -> (f32, f32) {
    (y - CARD_TOP, width - 2.0 * CARD_SIDE)
}

/// Size of the side arrows on the open card.
const ARROW_RADIUS: f32 = 11.0;

/// Center of the side arrow toward `direction` (-1 back, 1 on) on a card
/// `width` by `height`.
fn card_arrow(direction: f32, width: f32, height: f32) -> (f32, f32) {
    let cx = CANVAS.0 / 2.0;
    let height = height.min(CARD.1);
    let x = cx + direction * (width / 2.0 - CARD_SIDE / 2.0 - 2.0);
    (x, CARD_TOP + (height - CARD_TOP) / 2.0 - 4.0)
}

/// A page's name in the card header: a white word, then one in its colour.
/// The calendar names the month it shows.
fn page_title<'a>(
    page: Page,
    place: &'a str,
    month: &'a str,
    year: &'a str,
) -> (&'a str, &'a str, Accent) {
    match page {
        Page::Music => ("Sedang", "Diputar", Accent::Green),
        Page::Status => ("Hari", "Ini", Accent::Teal),
        Page::Prayer if !place.is_empty() => ("Sholat", place, Accent::Green),
        Page::Prayer => ("Waktu", "Sholat", Accent::Green),
        Page::Calendar => (month, year, Accent::Red),
        Page::Pomodoro => ("Fokus", "Pomodoro", Accent::Red),
        Page::Todo => ("Daftar", "Tugas", Accent::Green),
        Page::Shelf => ("Rak", "Berkas", Accent::Teal),
    }
}

/// A page's icon in the row that opens from the page dots.
fn page_glyph(page: Page) -> &'static str {
    match page {
        Page::Music => glyph::MUSIC,
        Page::Status => glyph::HOME,
        Page::Prayer => glyph::PRAYER,
        Page::Calendar => glyph::CALENDAR,
        Page::Pomodoro => glyph::TIMER,
        Page::Todo => glyph::CHECK,
        Page::Shelf => glyph::FILE,
    }
}

// The row of page icons, in the part the card grows by.
const NAV_STEP: f32 = 30.0;
const NAV_DROP: f32 = 6.0;
/// Extra room before Settings, for the line that sets it apart.
const NAV_GAP: f32 = 10.0;

/// Center of icon `index` of `count`; the last is Settings.
fn nav_item(index: usize, count: usize) -> (f32, f32) {
    let total = (count - 1) as f32 * NAV_STEP + NAV_GAP;
    let settings = if index + 1 == count { NAV_GAP } else { 0.0 };
    let x = CANVAS.0 / 2.0 - total / 2.0 + index as f32 * NAV_STEP + settings;
    (x, CARD.1 + NAV_DROP)
}

/// What the home page says is next: today's first event, else the first task
/// not done, and how many more of either wait behind it.
fn next_up(agenda: &[AgendaItem], tasks: &[Task]) -> Option<(String, usize)> {
    let events: Vec<&AgendaItem> = agenda.iter().filter(|item| item.today).collect();
    let open: Vec<&Task> = tasks.iter().filter(|task| !task.done).collect();
    let more = (events.len() + open.len()).checked_sub(1)?;
    let line = match (events.first(), open.first()) {
        (Some(event), _) => format!("{} {}", event.time, event.summary),
        (None, Some(task)) => task.text.clone(),
        (None, None) => return None,
    };
    Some((line, more))
}

/// The hour of a "22:38" clock.
fn clock_hour(clock: &str) -> u32 {
    clock
        .split(':')
        .next()
        .and_then(|hour| hour.trim().parse().ok())
        .unwrap_or(12)
}

fn greeting(hour: u32) -> &'static str {
    match hour {
        4..=10 => "Selamat pagi",
        11..=14 => "Selamat siang",
        15..=17 => "Selamat sore",
        _ => "Selamat malam",
    }
}

/// The sky at `hour`, top to bottom, for the home panel.
fn sky_colors(hour: u32) -> (D2D1_COLOR_F, D2D1_COLOR_F) {
    match hour {
        4..=6 => (color(0.2, 0.25, 0.55, 1.0), color(0.85, 0.45, 0.3, 1.0)),
        7..=15 => (color(0.13, 0.36, 0.75, 1.0), color(0.32, 0.62, 0.88, 1.0)),
        16..=18 => (color(0.35, 0.2, 0.55, 1.0), color(0.88, 0.45, 0.3, 1.0)),
        _ => (color(0.09, 0.09, 0.28, 1.0), color(0.27, 0.16, 0.47, 1.0)),
    }
}

/// The cover's main colour, from BGRA pixels: vivid pixels count most, so a
/// small bright subject beats a large grey background. Scaled to one
/// brightness, so the card's white text stays readable on it.
fn art_tint(pixels: &[u8]) -> Option<[f32; 3]> {
    let mut sum = [0.0_f32; 3];
    for pixel in pixels.as_chunks::<4>().0 {
        let rgb = [pixel[2], pixel[1], pixel[0]].map(|v| v as f32 / 255.0);
        let high = rgb[0].max(rgb[1]).max(rgb[2]);
        let low = rgb[0].min(rgb[1]).min(rgb[2]);
        let weight = (high - low).powi(2) + 0.01;
        for (total, value) in sum.iter_mut().zip(rgb) {
            *total += value * weight;
        }
    }
    let high = sum[0].max(sum[1]).max(sum[2]);
    (high > 0.0).then(|| sum.map(|value| value / high * 0.6))
}

/// Height of the sun's path at `position` (0 at Subuh, 4 at Isya), 0..1. The
/// peak leans toward the morning prayers, as the columns are evenly spaced.
fn sun_arc(position: f32) -> f32 {
    (std::f32::consts::PI * (position / 4.0).clamp(0.0, 1.0).powf(0.85)).sin()
}

/// Width of the concave curves joining the notch to the top screen edge.
const EAR: f32 = 10.0;
/// The canvas fits the largest notch shape; everything outside it is transparent.
pub const CANVAS: (f32, f32) = (
    max_of(&[IDLE.0, LEVEL.0, NOTICE_WIDTH.1, CARD_NAV.0, DROP_ZONE.0]) + 2.0 * EAR,
    max_of(&[IDLE.1, LEVEL.1, NOTICE_HEIGHT, CARD_NAV.1, DROP_ZONE.1]),
);

/// Album art is decoded at most this many pixels square.
const ART_PIXELS: u32 = 160;
/// Click radius around each music control.
const HIT_RADIUS: f32 = 20.0;

/// Positions on the music card, measured from the top of the page.
struct CardRects {
    art: D2D_RECT_F,
    text_left: f32,
    text_right: f32,
    progress: (f32, f32, f32),
    lyrics: (f32, f32),
    previous: (f32, f32),
    play: (f32, f32),
    next: (f32, f32),
    speaker: (f32, f32),
    volume: (f32, f32, f32),
}

impl CardRects {
    fn new(width: f32, lyrics: bool) -> Self {
        let cx = CANVAS.0 / 2.0;
        let (x0, x1) = (cx - width / 2.0, cx + width / 2.0);
        // Lyrics take the foot; without them the rest sits centered.
        let shift = if lyrics { -10.0 } else { 8.0 };
        let (bar, row) = (84.0 + shift, 116.0 + shift);
        Self {
            art: rect(x0 + 18.0, 10.0 + shift, x0 + 74.0, 66.0 + shift),
            text_left: x0 + 86.0,
            text_right: x1 - 18.0,
            progress: (x0 + 56.0, x1 - 56.0, bar),
            lyrics: (x0 + 30.0, row),
            previous: (cx - 44.0, row),
            play: (cx, row),
            next: (cx + 44.0, row),
            speaker: (x1 - 28.0, row),
            volume: (x1 - 116.0, x1 - 44.0, row),
        }
    }
}

/// "3:07"
fn format_duration(seconds: f64) -> String {
    let total = seconds.max(0.0).round() as u64;
    format!("{}:{:02}", total / 60, total % 60)
}

/// Shelf thumbnails are made at most this many pixels square.
const THUMB_PIXELS: u32 = 96;

// Shelf card layout: five 64 px tiles per row.
const TILE: (f32, f32) = (64.0, 46.0);
const TILE_GAP: f32 = 11.0;

/// Tasks in each of the card's two columns.
const TODO_COLUMN: usize = 3;

/// Task card layout between the card's edges.
struct TodoRects {
    x0: f32,
    x1: f32,
}

impl TodoRects {
    /// The add button, which saves while typing.
    fn add(&self) -> D2D_RECT_F {
        rect(self.x1 - 104.0, 8.0, self.x1 - 14.0, 34.0)
    }

    fn clear(&self) -> D2D_RECT_F {
        rect(self.x1 - 230.0, 8.0, self.x1 - 112.0, 34.0)
    }

    /// The field for a new task, in place of the count.
    fn field(&self) -> D2D_RECT_F {
        rect(self.x0 + 14.0, 6.0, self.x1 - 112.0, 36.0)
    }

    fn row(&self, index: usize) -> D2D_RECT_F {
        let (column, row) = (index / TODO_COLUMN, index % TODO_COLUMN);
        let width = (self.x1 - self.x0 - 32.0) / 2.0;
        let left = self.x0 + 12.0 + column as f32 * (width + 8.0);
        let top = 42.0 + row as f32 * TODO_ROW;
        rect(left, top, left + width, top + TODO_ROW)
    }

    fn check(&self, index: usize) -> (f32, f32) {
        let row = self.row(index);
        (row.left + 18.0, (row.top + row.bottom) / 2.0)
    }

    fn remove(&self, index: usize) -> (f32, f32) {
        let row = self.row(index);
        (row.right - 18.0, (row.top + row.bottom) / 2.0)
    }
}

struct ShelfRects {
    left: f32,
    clear: D2D_RECT_F,
}

impl ShelfRects {
    fn new(x0: f32, x1: f32) -> Self {
        Self {
            left: x0 + 18.0,
            clear: rect(x1 - 130.0, 6.0, x1 - 14.0, 32.0),
        }
    }

    fn tile(&self, index: usize) -> D2D_RECT_F {
        let (column, row) = ((index % 5) as f32, (index / 5) as f32);
        let left = self.left + column * (TILE.0 + TILE_GAP);
        let top = 38.0 + row * (TILE.1 + 4.0);
        rect(left, top, left + TILE.0, top + TILE.1)
    }

    /// Center of the remove button on a tile.
    fn remove(&self, index: usize) -> (f32, f32) {
        let tile = self.tile(index);
        (tile.right - 6.0, tile.top + 6.0)
    }
}

/// How far, in logical pixels, pages slide while switching.
const PAGE_SLIDE: f32 = 18.0;

// Calendar card layout.
const CELL: (f32, f32) = (28.0, 19.0);
const GRID_TOP: f32 = 34.0;

/// The header row's middle, as a page sees it.
const CARD_TOP_ROW: f32 = 17.0 - CARD_TOP;

struct CalendarRects {
    grid_left: f32,
    previous_month: (f32, f32),
    next_month: (f32, f32),
    agenda_left: f32,
    agenda_right: f32,
}

impl CalendarRects {
    fn new(x0: f32, x1: f32) -> Self {
        Self {
            grid_left: x0 + 20.0,
            // In the header row, right of the date.
            previous_month: (x1 - 66.0, CARD_TOP_ROW),
            next_month: (x1 - 38.0, CARD_TOP_ROW),
            agenda_left: x0 + 232.0,
            agenda_right: x1 - 14.0,
        }
    }
}

/// Top of the first event beside the month, and the height of each.
const AGENDA_TOP: f32 = 36.0;
const AGENDA_ROW: f32 = 34.0;

// Pomodoro card layout: the ring on the left, its controls on the right.
const POMODORO_RING: (f32, f32) = (112.0, 78.0);
const POMODORO_LEFT: f32 = 214.0;

struct PomodoroRects {
    start_pause: D2D_RECT_F,
    reset: (f32, f32),
}

impl PomodoroRects {
    fn new(x0: f32) -> Self {
        let left = x0 + POMODORO_LEFT;
        Self {
            start_pause: rect(left, 82.0, left + 92.0, 110.0),
            reset: (left + 114.0, 96.0),
        }
    }
}

// Notice card layout, left to right.
const CARD_PAD_LEFT: f32 = 14.0;
const RING: f32 = 38.0;
const RING_STROKE: f32 = 3.5;
const RING_GAP: f32 = 12.0;
/// Longest title or subtitle before it is cut with an ellipsis.
const TEXT_MAX: f32 = 220.0;
const TRAILING_GAP: f32 = 22.0;
const TRAILING_ICON: f32 = 38.0;
const CARD_PAD_RIGHT: f32 = 18.0;

const fn max(a: f32, b: f32) -> f32 {
    if a > b {
        a
    } else {
        b
    }
}

const fn max_of(values: &[f32]) -> f32 {
    let mut largest = values[0];
    let mut i = 1;
    while i < values.len() {
        largest = max(largest, values[i]);
        i += 1;
    }
    largest
}

/// Segoe Fluent Icons glyphs.
pub mod glyph {
    pub const MUTE: &str = "\u{E74F}";
    pub const VOLUME_LOW: &str = "\u{E993}";
    pub const VOLUME_MID: &str = "\u{E994}";
    pub const VOLUME_HIGH: &str = "\u{E995}";
    pub const BRIGHTNESS: &str = "\u{E706}";
    pub const WIFI: &str = "\u{E701}";
    pub const BLUETOOTH: &str = "\u{E702}";
    pub const HEADPHONES: &str = "\u{E7F6}";
    pub const PRAYER: &str = "\u{E823}";
    pub const MUSIC: &str = "\u{E8D6}";
    pub const PLAY: &str = "\u{E768}";
    pub const PAUSE: &str = "\u{E769}";
    pub const CHEVRON_LEFT: &str = "\u{E76B}";
    pub const CHEVRON_RIGHT: &str = "\u{E76C}";
    pub const RESET: &str = "\u{E72C}";
    pub const TIMER: &str = "\u{E916}";
    pub const DOWNLOAD: &str = "\u{E896}";
    pub const CLOSE: &str = "\u{E711}";
    pub const FILE: &str = "\u{E8A5}";
    pub const ADD: &str = "\u{E710}";
    pub const CALENDAR: &str = "\u{E787}";
    pub const DELETE: &str = "\u{E74D}";
    pub const CHECK: &str = "\u{E73E}";
    pub const WATER: &str = "\u{EB42}";
    pub const EYE: &str = "\u{E890}";
    pub const WALK: &str = "\u{E805}";
    pub const HOME: &str = "\u{E80F}";
    pub const SETTINGS: &str = "\u{E713}";

    /// Battery outline filled to `percent`.
    pub fn battery(percent: u32) -> &'static str {
        const LEVELS: [&str; 11] = [
            "\u{EBA0}", "\u{EBA1}", "\u{EBA2}", "\u{EBA3}", "\u{EBA4}", "\u{EBA5}", "\u{EBA6}",
            "\u{EBA7}", "\u{EBA8}", "\u{EBA9}", "\u{EBAA}",
        ];
        LEVELS[((percent.min(100) + 5) / 10) as usize]
    }
}

const EYE_COLOR: D2D1_COLOR_F = D2D1_COLOR_F {
    r: 0.93,
    g: 0.96,
    b: 1.0,
    a: 1.0,
};

fn accent_color(accent: Accent) -> D2D1_COLOR_F {
    match accent {
        Accent::Green => color(0.19, 0.82, 0.35, 1.0),
        Accent::Red => color(1.0, 0.27, 0.23, 1.0),
        Accent::Blue => color(0.25, 0.62, 1.0, 1.0),
        Accent::Teal => color(0.39, 0.82, 1.0, 1.0),
    }
}

/// Everything the renderer needs for one frame, in logical pixels.
pub struct Scene<'a> {
    pub width: f32,
    pub height: f32,
    pub eyes_visible: f32,
    pub blink: f32,
    pub look: f32,
    pub clock: &'a str,
    pub banner_visible: f32,
    pub banner: Option<&'a Content>,
    pub music: Option<MusicScene<'a>>,
    /// How visible the playing strip and the music card are, 0..1.
    pub music_visible: f32,
    pub card_visible: f32,
    /// Which page the open card shows, and all of them in order.
    pub page: Page,
    pub page_index: usize,
    pub page_list: Vec<Page>,
    /// How far the page dots have turned into page icons, 0..1.
    pub nav_visible: f32,
    /// How far the notch has widened to show the date, 0..1.
    pub peek_visible: f32,
    /// The closed notch's size relative to the usual one.
    pub closed_scale: f32,
    pub calendar: CalendarScene,
    /// The Pomodoro timer; `timer_strip` puts it in the live strip.
    pub timer: Option<TimerScene>,
    /// What the live strip beside the clock shows.
    pub strip: Strip,
    /// "Maghrib 12 mnt", while a prayer is close.
    pub prayer_soon: &'a str,
    pub shelf: ShelfScene<'a>,
    pub todo: TodoScene<'a>,
    pub agenda: Vec<AgendaItem<'a>>,
    pub prayer: Option<PrayerScene>,
    /// The prayer location's name.
    pub place: &'a str,
    /// The closed notch is the thin bar, with nothing on it.
    pub minimal: bool,
    /// Files are being dragged over the notch.
    pub drop_zone: bool,
    /// While switching pages: the page being left, how far the switch has
    /// gone (0..1), and its direction (1 toward later pages, -1 back).
    pub previous_page: Option<Page>,
    pub page_mix: f32,
    pub page_direction: f32,
    pub status: StatusScene<'a>,
}

#[derive(Clone, Copy, PartialEq)]
pub enum Page {
    Music,
    Status,
    Calendar,
    Shelf,
    Prayer,
    Todo,
    Pomodoro,
}

/// One upcoming event beside the month.
pub struct AgendaItem<'a> {
    /// "Hari ini", "Besok", "Sab 7".
    pub day: String,
    /// "09:00", or "Seharian" for an all-day event.
    pub time: String,
    pub summary: &'a str,
    pub today: bool,
}

/// Live information shown on both sides of the clock in the closed notch.
#[derive(Clone, Copy, PartialEq)]
pub enum Strip {
    None,
    Music,
    Timer,
    Prayer,
}

pub struct PrayerScene {
    pub next_name: &'static str,
    /// "15:20"
    pub next_time: String,
    /// "1 jam 12 mnt"
    pub wait: String,
    /// Today's five times, "04:37" and so on.
    pub times: [String; 5],
    /// Which of today's prayers comes next, if any is left today.
    pub next_today: Option<usize>,
    /// How far the wait from the last prayer to the next has gone, 0..1.
    pub progress: f32,
    /// The sun's place on its path: 0 at Subuh, 4 at Isya.
    pub sun: f32,
}

pub struct ShelfScene<'a> {
    pub items: &'a [PathBuf],
    /// The tile under the mouse.
    pub hover: Option<usize>,
}

#[derive(Clone, Copy, PartialEq, Debug)]
pub enum ShelfHit {
    Tile(usize),
    Remove(usize),
    ClearAll,
}

pub struct TodoScene<'a> {
    pub tasks: &'a [Task],
    pub left: usize,
    /// The task under the mouse.
    pub hover: Option<usize>,
    /// Text being typed for a new task, while typing.
    pub editing: Option<&'a str>,
    /// The text cursor is lit (it blinks).
    pub caret: bool,
}

#[derive(Clone, Copy, PartialEq, Debug)]
pub enum TodoHit {
    Add,
    ClearDone,
    Toggle(usize),
    Remove(usize),
}

pub struct TimerScene {
    pub label: &'static str,
    /// "24:12"
    pub left: String,
    pub running: bool,
    pub on_break: bool,
    pub session: u32,
    /// How much of the phase has gone, 0..1.
    pub progress: f32,
}

/// The month shown on the calendar page.
pub struct CalendarScene {
    pub year: i32,
    pub month: u32,
    /// Today's day number when the shown month is the current one.
    pub today: Option<u32>,
    /// Days of the shown month with an event on the agenda.
    pub marked: Vec<u32>,
}

#[derive(Clone, Copy, PartialEq, Debug)]
pub enum CalendarControl {
    PreviousMonth,
    NextMonth,
}

#[derive(Clone, Copy, PartialEq, Debug)]
pub enum PomodoroControl {
    StartPause,
    Reset,
}

pub struct StatusScene<'a> {
    pub date: &'a str,
    pub weather: Option<WeatherScene<'a>>,
    /// Shown in place of the weather when there is none.
    pub weather_hint: &'a str,
    /// Charge and whether it is charging; None without a battery.
    pub battery: Option<(u32, bool)>,
}

pub struct WeatherScene<'a> {
    pub glyph: &'a str,
    pub temperature: String,
    pub description: &'a str,
    pub place: &'a str,
}

pub struct MusicScene<'a> {
    pub title: &'a str,
    pub artist: &'a str,
    pub playing: bool,
    /// The song has lyrics to show.
    pub lyrics_ready: bool,
    /// The lyrics button is on.
    pub lyrics_on: bool,
    /// Master volume, 0..1.
    pub volume: f32,
    /// How far the volume slider has opened, 0..1.
    pub volume_visible: f32,
    /// Seconds.
    pub position: f64,
    pub duration: f64,
    /// Level bar heights, 0..1.
    pub bars: [f32; 4],
    /// The lyric line being sung and the next, when the song has synced lyrics.
    pub lyric: Option<(&'a str, &'a str)>,
}

/// A measured line of text, ready to draw.
struct TextBlock {
    layout: IDWriteTextLayout,
    width: f32,
    height: f32,
}

/// Measured parts of a notice card.
struct CardLayout {
    /// Notch width the card needs.
    width: f32,
    /// Width of the title/subtitle column.
    text_width: f32,
    title: TextBlock,
    subtitle: TextBlock,
    /// The trailing large text, if any.
    big: Option<TextBlock>,
}

pub struct Renderer {
    factory: ID2D1Factory,
    dwrite: IDWriteFactory,
    target: ID2D1DCRenderTarget,
    text: IDWriteTextFormat,
    small_text: IDWriteTextFormat,
    title_text: IDWriteTextFormat,
    subtitle_text: IDWriteTextFormat,
    trailing_text: IDWriteTextFormat,
    /// Small left-aligned text: track times.
    small_left: IDWriteTextFormat,
    /// Clock-sized left-aligned text: the timer countdown in the strip.
    text_left: IDWriteTextFormat,
    /// Large clock on the status card.
    big_clock: IDWriteTextFormat,
    /// Small centered text: calendar days.
    tiny: IDWriteTextFormat,
    /// Centered single-line file names that end in an ellipsis.
    tile_text: IDWriteTextFormat,
    /// Centered single-line lyrics: the line being sung, and the next.
    lyric_text: IDWriteTextFormat,
    lyric_next: IDWriteTextFormat,
    /// Small icon glyph for the remove buttons.
    close_glyph: IDWriteTextFormat,
    /// Icons inside pill buttons.
    pill_glyph: IDWriteTextFormat,
    /// Icons in the page navigation pill.
    nav_glyph: IDWriteTextFormat,
    /// Weather symbols (Segoe UI Symbol).
    symbols: IDWriteTextFormat,
    icons: IDWriteTextFormat,
    black: ID2D1SolidColorBrush,
    light: ID2D1SolidColorBrush,
    /// Recolored per notice.
    accent: ID2D1SolidColorBrush,
    round_caps: ID2D1StrokeStyle,
    /// Album art of the current track, decoded once per song.
    art: Option<ID2D1Bitmap>,
    /// The art's main colour, for the music card's background.
    art_tint: Option<[f32; 3]>,
    /// Shelf thumbnails, in shelf order.
    thumbs: Vec<(PathBuf, Option<ID2D1Bitmap>)>,
    /// Dashed outline of the drop zone.
    dashes: ID2D1StrokeStyle,
    dc: HDC,
    bitmap: HBITMAP,
    previous: HGDIOBJ,
    size: SIZE,
}

impl Renderer {
    pub fn new(scale: f32) -> Result<Self> {
        unsafe {
            let size = SIZE {
                cx: (CANVAS.0 * scale).ceil() as i32,
                cy: (CANVAS.1 * scale).ceil() as i32,
            };

            let dc = CreateCompatibleDC(None);
            let info = BITMAPINFO {
                bmiHeader: BITMAPINFOHEADER {
                    biSize: size_of::<BITMAPINFOHEADER>() as u32,
                    biWidth: size.cx,
                    // Negative height: top-down rows, matching Direct2D.
                    biHeight: -size.cy,
                    biPlanes: 1,
                    biBitCount: 32,
                    biCompression: BI_RGB.0,
                    ..Default::default()
                },
                ..Default::default()
            };
            let mut bits: *mut c_void = std::ptr::null_mut();
            let bitmap = CreateDIBSection(Some(dc), &info, DIB_RGB_COLORS, &mut bits, None, 0)?;
            let previous = SelectObject(dc, bitmap.into());

            let factory: ID2D1Factory = D2D1CreateFactory(D2D1_FACTORY_TYPE_SINGLE_THREADED, None)?;
            let props = D2D1_RENDER_TARGET_PROPERTIES {
                r#type: D2D1_RENDER_TARGET_TYPE_SOFTWARE,
                pixelFormat: D2D1_PIXEL_FORMAT {
                    format: DXGI_FORMAT_B8G8R8A8_UNORM,
                    alphaMode: D2D1_ALPHA_MODE_PREMULTIPLIED,
                },
                // Drawing happens in logical pixels; Direct2D scales to the DPI.
                dpiX: 96.0 * scale,
                dpiY: 96.0 * scale,
                usage: D2D1_RENDER_TARGET_USAGE_NONE,
                minLevel: D2D1_FEATURE_LEVEL_DEFAULT,
            };
            let target = factory.CreateDCRenderTarget(&props)?;
            target.BindDC(
                dc,
                &RECT {
                    left: 0,
                    top: 0,
                    right: size.cx,
                    bottom: size.cy,
                },
            )?;

            let black = target.CreateSolidColorBrush(&color(0.0, 0.0, 0.0, 1.0), None)?;
            let light = target.CreateSolidColorBrush(&EYE_COLOR, None)?;
            let accent = target.CreateSolidColorBrush(&accent_color(Accent::Blue), None)?;
            let dashes = factory.CreateStrokeStyle(
                &D2D1_STROKE_STYLE_PROPERTIES {
                    startCap: D2D1_CAP_STYLE_ROUND,
                    endCap: D2D1_CAP_STYLE_ROUND,
                    dashCap: D2D1_CAP_STYLE_ROUND,
                    lineJoin: D2D1_LINE_JOIN_ROUND,
                    miterLimit: 10.0,
                    dashStyle: D2D1_DASH_STYLE_DASH,
                    dashOffset: 0.0,
                },
                None,
            )?;
            let round_caps = factory.CreateStrokeStyle(
                &D2D1_STROKE_STYLE_PROPERTIES {
                    startCap: D2D1_CAP_STYLE_ROUND,
                    endCap: D2D1_CAP_STYLE_ROUND,
                    dashCap: D2D1_CAP_STYLE_FLAT,
                    lineJoin: D2D1_LINE_JOIN_ROUND,
                    miterLimit: 10.0,
                    dashStyle: D2D1_DASH_STYLE_SOLID,
                    dashOffset: 0.0,
                },
                None,
            )?;

            let dwrite: IDWriteFactory = DWriteCreateFactory(DWRITE_FACTORY_TYPE_SHARED)?;
            let font = w!("Segoe UI Variable Display");
            let format = |family, weight: DWRITE_FONT_WEIGHT, size| -> Result<IDWriteTextFormat> {
                let format = dwrite.CreateTextFormat(
                    family,
                    None,
                    weight,
                    DWRITE_FONT_STYLE_NORMAL,
                    DWRITE_FONT_STRETCH_NORMAL,
                    size,
                    w!("id-id"),
                )?;
                format.SetTextAlignment(DWRITE_TEXT_ALIGNMENT_CENTER)?;
                format.SetParagraphAlignment(DWRITE_PARAGRAPH_ALIGNMENT_CENTER)?;
                Ok(format)
            };
            // Left-aligned single line from the top; too long ends in an ellipsis.
            let line = |weight, size| -> Result<IDWriteTextFormat> {
                let format = format(font, weight, size)?;
                format.SetTextAlignment(DWRITE_TEXT_ALIGNMENT_LEADING)?;
                format.SetParagraphAlignment(DWRITE_PARAGRAPH_ALIGNMENT_NEAR)?;
                format.SetWordWrapping(DWRITE_WORD_WRAPPING_NO_WRAP)?;
                let ellipsis = dwrite.CreateEllipsisTrimmingSign(&format)?;
                format.SetTrimming(
                    &DWRITE_TRIMMING {
                        granularity: DWRITE_TRIMMING_GRANULARITY_CHARACTER,
                        delimiter: 0,
                        delimiterCount: 0,
                    },
                    &ellipsis,
                )?;
                Ok(format)
            };
            let text = format(font, DWRITE_FONT_WEIGHT_SEMI_BOLD, 13.0)?;
            let small_text = format(font, DWRITE_FONT_WEIGHT_SEMI_BOLD, 12.0)?;
            let icons = format(w!("Segoe Fluent Icons"), DWRITE_FONT_WEIGHT_NORMAL, 15.0)?;
            let title_text = line(DWRITE_FONT_WEIGHT_SEMI_BOLD, 14.5)?;
            let subtitle_text = line(DWRITE_FONT_WEIGHT_NORMAL, 12.5)?;
            let trailing_text = line(DWRITE_FONT_WEIGHT_SEMI_LIGHT, 25.0)?;
            let small_left = line(DWRITE_FONT_WEIGHT_NORMAL, 11.5)?;
            let text_left = line(DWRITE_FONT_WEIGHT_SEMI_BOLD, 13.0)?;
            let big_clock = line(DWRITE_FONT_WEIGHT_SEMI_BOLD, 30.0)?;
            let tiny = format(font, DWRITE_FONT_WEIGHT_NORMAL, 12.0)?;
            let tile_text = line(DWRITE_FONT_WEIGHT_NORMAL, 11.0)?;
            tile_text.SetTextAlignment(DWRITE_TEXT_ALIGNMENT_CENTER)?;
            tile_text.SetParagraphAlignment(DWRITE_PARAGRAPH_ALIGNMENT_CENTER)?;
            let centered_line = |weight, size| -> Result<IDWriteTextFormat> {
                let format = line(weight, size)?;
                format.SetTextAlignment(DWRITE_TEXT_ALIGNMENT_CENTER)?;
                format.SetParagraphAlignment(DWRITE_PARAGRAPH_ALIGNMENT_CENTER)?;
                Ok(format)
            };
            let lyric_text = centered_line(DWRITE_FONT_WEIGHT_SEMI_BOLD, 14.0)?;
            let lyric_next = centered_line(DWRITE_FONT_WEIGHT_NORMAL, 12.0)?;
            let close_glyph = format(w!("Segoe Fluent Icons"), DWRITE_FONT_WEIGHT_NORMAL, 8.0)?;
            let pill_glyph = format(w!("Segoe Fluent Icons"), DWRITE_FONT_WEIGHT_NORMAL, 11.0)?;
            let nav_glyph = format(w!("Segoe Fluent Icons"), DWRITE_FONT_WEIGHT_NORMAL, 13.0)?;
            let symbols = format(w!("Segoe UI Symbol"), DWRITE_FONT_WEIGHT_NORMAL, 26.0)?;
            symbols.SetTextAlignment(DWRITE_TEXT_ALIGNMENT_LEADING)?;
            symbols.SetParagraphAlignment(DWRITE_PARAGRAPH_ALIGNMENT_NEAR)?;

            Ok(Self {
                factory,
                dwrite,
                target,
                text,
                small_text,
                title_text,
                subtitle_text,
                trailing_text,
                small_left,
                text_left,
                big_clock,
                tiny,
                tile_text,
                lyric_text,
                lyric_next,
                close_glyph,
                pill_glyph,
                nav_glyph,
                symbols,
                icons,
                black,
                light,
                accent,
                round_caps,
                art: None,
                art_tint: None,
                thumbs: Vec::new(),
                dashes,
                dc,
                bitmap,
                previous,
                size,
            })
        }
    }

    /// Size of the window in physical pixels.
    pub fn size(&self) -> SIZE {
        self.size
    }

    pub fn present(&self, hwnd: HWND, origin: POINT, scene: &Scene) -> Result<()> {
        unsafe {
            let t = &self.target;
            t.BeginDraw();
            t.Clear(Some(&color(0.0, 0.0, 0.0, 0.0)));
            self.draw_shape(scene)?;
            self.draw_eyes(scene);
            self.draw_banner(scene);
            self.draw_music(scene);
            self.with_scale(scene.closed_scale, || self.draw_clock(scene));
            t.EndDraw(None, None)?;

            let blend = BLENDFUNCTION {
                BlendOp: AC_SRC_OVER as u8,
                BlendFlags: 0,
                SourceConstantAlpha: 255,
                AlphaFormat: AC_SRC_ALPHA as u8,
            };
            UpdateLayeredWindow(
                hwnd,
                None,
                Some(&origin),
                Some(&self.size),
                Some(self.dc),
                Some(&POINT { x: 0, y: 0 }),
                COLORREF(0),
                Some(&blend),
                ULW_ALPHA,
            )
        }
    }

    /// Black body with rounded bottom corners and concave ears at the top edge.
    unsafe fn draw_shape(&self, scene: &Scene) -> Result<()> {
        let cx = CANVAS.0 / 2.0;
        let (w, h) = (scene.width, scene.height);
        let (x0, x1) = (cx - w / 2.0, cx + w / 2.0);
        let r = (h / 2.0).min(24.0);
        let ear = EAR.min(h - r);

        let geometry = self.factory.CreatePathGeometry()?;
        let sink = geometry.Open()?;
        sink.BeginFigure(pt(x0 - ear, 0.0), D2D1_FIGURE_BEGIN_FILLED);
        sink.AddLine(pt(x1 + ear, 0.0));
        sink.AddQuadraticBezier(&D2D1_QUADRATIC_BEZIER_SEGMENT {
            point1: pt(x1, 0.0),
            point2: pt(x1, ear),
        });
        sink.AddLine(pt(x1, h - r));
        sink.AddArc(&arc(pt(x1 - r, h), r));
        sink.AddLine(pt(x0 + r, h));
        sink.AddArc(&arc(pt(x0, h - r), r));
        sink.AddLine(pt(x0, ear));
        sink.AddQuadraticBezier(&D2D1_QUADRATIC_BEZIER_SEGMENT {
            point1: pt(x0, 0.0),
            point2: pt(x0 - ear, 0.0),
        });
        sink.EndFigure(D2D1_FIGURE_END_CLOSED);
        sink.Close()?;

        self.target.FillGeometry(&geometry, &self.black, None);

        // The music card glows with the colour of the cover.
        let shown = match (scene.page, scene.previous_page) {
            (Page::Music, Some(_)) => scene.page_mix,
            (Page::Music, None) => 1.0,
            (_, Some(Page::Music)) => 1.0 - scene.page_mix,
            _ => 0.0,
        };
        let opacity = shown * scene.card_visible.clamp(0.0, 1.0);
        if let Some([r, g, b]) = self.art_tint.filter(|_| opacity > 0.01 && !scene.drop_zone) {
            let glow = self.gradient(
                pt(x0, 0.0),
                pt(x1, h),
                &[(0.0, color(r, g, b, 1.0)), (0.9, color(0.0, 0.0, 0.0, 0.0))],
            )?;
            glow.SetOpacity(opacity);
            self.target.FillGeometry(&geometry, &glow, None);
        }
        Ok(())
    }

    unsafe fn draw_eyes(&self, scene: &Scene) {
        let visible = scene.eyes_visible.clamp(0.0, 1.0);
        if visible < 0.01 {
            return;
        }
        // Eyes grow in slightly as they fade in.
        let scale = 0.7 + 0.3 * visible;
        let eye_w = 9.0 * scale;
        let eye_h = (18.0 * scale * (1.0 - 0.85 * scene.blink)).max(2.0);
        let cy = scene.height / 2.0 + 1.0;
        let offset = scene.look * 4.0;

        for side in [-1.0, 1.0] {
            let cx = CANVAS.0 / 2.0 + side * 16.0 * scale + offset;
            // A soft halo first, then the eye itself.
            for (grow, alpha) in [(4.5, 0.05), (2.0, 0.10), (0.0, 1.0)] {
                let (hw, hh) = (eye_w / 2.0 + grow, eye_h / 2.0 + grow);
                let radius = hw.min(hh);
                self.light.SetOpacity(alpha * visible);
                self.target.FillRoundedRectangle(
                    &D2D1_ROUNDED_RECT {
                        rect: D2D_RECT_F {
                            left: cx - hw,
                            top: cy - hh,
                            right: cx + hw,
                            bottom: cy + hh,
                        },
                        radiusX: radius,
                        radiusY: radius,
                    },
                    &self.light,
                );
            }
        }
    }

    /// Banner content, laid out against the notch's current edges so it rides
    /// along while the notch grows.
    unsafe fn draw_banner(&self, scene: &Scene) {
        let Some(content) = scene.banner else {
            return;
        };
        let alpha = scene.banner_visible.clamp(0.0, 1.0);
        if alpha < 0.01 {
            return;
        }
        let cx = CANVAS.0 / 2.0;
        let (x0, x1) = (cx - scene.width / 2.0, cx + scene.width / 2.0);
        let h = scene.height;
        let icon_box = rect(x0 + 8.0, 0.0, x0 + 38.0, h);

        match content {
            Content::Level { kind, level, muted } => {
                let level = level.value.clamp(0.0, 1.0);
                let glyph = match kind {
                    LevelKind::Brightness => glyph::BRIGHTNESS,
                    LevelKind::Volume if *muted || level < 0.005 => glyph::MUTE,
                    LevelKind::Volume if level < 0.34 => glyph::VOLUME_LOW,
                    LevelKind::Volume if level < 0.67 => glyph::VOLUME_MID,
                    LevelKind::Volume => glyph::VOLUME_HIGH,
                };
                self.draw_text(glyph, &self.icons, icon_box, alpha);
                self.draw_level(x0, x1, h, level, *muted, alpha);
            }
            Content::Notice(notice) => self.draw_notice(notice, x0, x1, h, alpha),
        }
    }

    /// Decodes album art for the current track; None clears it.
    pub fn set_album_art(&mut self, bytes: Option<&[u8]>) {
        let decoded = bytes.and_then(|bytes| unsafe { self.decode_art(bytes).ok() });
        (self.art, self.art_tint) = decoded.map_or((None, None), |(art, tint)| (Some(art), tint));
    }

    /// Center-crops to a square and scales to a small bitmap: the art is never
    /// drawn larger than the card shows it. Also returns its main colour.
    unsafe fn decode_art(&self, bytes: &[u8]) -> Result<(ID2D1Bitmap, Option<[f32; 3]>)> {
        let wic: IWICImagingFactory =
            CoCreateInstance(&CLSID_WICImagingFactory, None, CLSCTX_INPROC_SERVER)?;
        let stream = SHCreateMemStream(Some(bytes)).ok_or(windows::core::Error::empty())?;
        let decoder =
            wic.CreateDecoderFromStream(&stream, std::ptr::null(), WICDecodeMetadataCacheOnLoad)?;
        let frame = decoder.GetFrame(0)?;
        let (mut width, mut height) = (0, 0);
        frame.GetSize(&mut width, &mut height)?;
        let side = width.min(height);
        let clipper = wic.CreateBitmapClipper()?;
        clipper.Initialize(
            &frame,
            &WICRect {
                X: ((width - side) / 2) as i32,
                Y: ((height - side) / 2) as i32,
                Width: side as i32,
                Height: side as i32,
            },
        )?;
        let pixels = ART_PIXELS.min(side);
        let scaler = wic.CreateBitmapScaler()?;
        scaler.Initialize(&clipper, pixels, pixels, WICBitmapInterpolationModeFant)?;
        let converter = wic.CreateFormatConverter()?;
        converter.Initialize(
            &scaler,
            &GUID_WICPixelFormat32bppPBGRA,
            WICBitmapDitherTypeNone,
            None,
            0.0,
            WICBitmapPaletteTypeMedianCut,
        )?;
        let mut buffer = vec![0u8; (pixels * pixels * 4) as usize];
        converter.CopyPixels(std::ptr::null(), pixels * 4, &mut buffer)?;
        let bitmap = self.target.CreateBitmapFromWicBitmap(&converter, None)?;
        Ok((bitmap, art_tint(&buffer)))
    }

    /// Which music control sits at (`x`, `y`) on the open card.
    pub fn music_control_at(&self, x: f32, y: f32, width: f32, lyrics: bool) -> Option<Command> {
        let (y, width) = on_page(y, width);
        let card = CardRects::new(width, lyrics);
        [
            (card.previous, Command::Previous),
            (card.play, Command::TogglePlay),
            (card.next, Command::Next),
        ]
        .into_iter()
        .find(|((cx, cy), _)| (x - cx).powi(2) + (y - cy).powi(2) <= HIT_RADIUS.powi(2))
        .map(|(_, command)| command)
    }

    /// Whether the lyrics button is at (`x`, `y`) on the open card.
    pub fn lyrics_button_at(&self, x: f32, y: f32, width: f32, lyrics: bool) -> bool {
        let (y, width) = on_page(y, width);
        let (bx, by) = CardRects::new(width, lyrics).lyrics;
        (x - bx).powi(2) + (y - by).powi(2) <= HIT_RADIUS.powi(2)
    }

    /// Whether the mouse at (`x`, `y`) opens the volume slider: on the
    /// speaker, or while `open`, anywhere along the slider too.
    pub fn volume_zone_at(&self, x: f32, y: f32, width: f32, lyrics: bool, open: bool) -> bool {
        let (y, width) = on_page(y, width);
        let card = CardRects::new(width, lyrics);
        let left = if open {
            card.volume.0 - 10.0
        } else {
            card.speaker.0 - 14.0
        };
        x >= left && x <= card.speaker.0 + 14.0 && (y - card.speaker.1).abs() <= 16.0
    }

    /// Whether the volume slider is at (`x`, `y`) on the open card.
    pub fn volume_at(&self, x: f32, y: f32, width: f32, lyrics: bool) -> bool {
        let (y, width) = on_page(y, width);
        let (left, right, row) = CardRects::new(width, lyrics).volume;
        x >= left - 6.0 && x <= right + 6.0 && (y - row).abs() <= 12.0
    }

    /// The volume the slider gives at `x`, past its ends included.
    pub fn volume_level(&self, x: f32, width: f32, lyrics: bool) -> f32 {
        let (left, right, _) = CardRects::new(width - 2.0 * CARD_SIDE, lyrics).volume;
        ((x - left) / (right - left)).clamp(0.0, 1.0)
    }

    /// The live strip (timer or music) and the open card's page.
    unsafe fn draw_music(&self, scene: &Scene) {
        let cx = CANVAS.0 / 2.0;
        let (x0, x1) = (cx - scene.width / 2.0, cx + scene.width / 2.0);
        let notch = rect(x0, 0.0, x1, scene.height);
        if scene.music_visible > 0.01 {
            let alpha = scene.music_visible.min(1.0);
            self.target
                .PushAxisAlignedClip(&notch, D2D1_ANTIALIAS_MODE_PER_PRIMITIVE);
            // The strip's own coordinates, before the notch size scales it.
            let k = scene.closed_scale;
            let (left, right) = (cx - scene.width / 2.0 / k, cx + scene.width / 2.0 / k);
            let height = scene.height / k;
            self.with_scale(k, || match (scene.strip, &scene.timer, &scene.music) {
                (Strip::Timer, Some(timer), _) => {
                    self.draw_timer_strip(timer, left, right, height, alpha)
                }
                (Strip::Music, _, Some(music)) => {
                    self.draw_music_strip(music, left, right, height, alpha)
                }
                (Strip::Prayer, _, _) => {
                    self.draw_prayer_strip(scene.prayer_soon, left, right, height, alpha)
                }
                _ => {}
            });
            self.target.PopAxisAlignedClip();
        }
        if scene.card_visible <= 0.01 {
            return;
        }
        let alpha = scene.card_visible.min(1.0);
        self.target
            .PushAxisAlignedClip(&notch, D2D1_ANTIALIAS_MODE_PER_PRIMITIVE);
        if scene.drop_zone {
            self.draw_drop_zone(x0, x1, scene.height, alpha);
            self.target.PopAxisAlignedClip();
            return;
        }
        // The page sits between the side arrows, under the header row.
        let (inner0, inner1) = (x0 + CARD_SIDE, x1 - CARD_SIDE);
        match scene.previous_page {
            // Switching pages: the old one fades out sliding away, the new one
            // fades in from the side the wheel moved toward.
            Some(previous) if scene.page_mix < 1.0 => {
                let mix = scene.page_mix;
                let slide = PAGE_SLIDE * scene.page_direction;
                self.draw_card_header(scene, previous, x0, x1, alpha * (1.0 - mix), false);
                self.draw_card_header(scene, scene.page, x0, x1, alpha * mix, true);
                self.with_offset(-slide * mix, CARD_TOP, || {
                    self.draw_page(scene, previous, inner0, inner1, alpha * (1.0 - mix))
                });
                self.with_offset(slide * (1.0 - mix), CARD_TOP, || {
                    self.draw_page(scene, scene.page, inner0, inner1, alpha * mix)
                });
            }
            _ => {
                self.draw_card_header(scene, scene.page, x0, x1, alpha, true);
                self.with_offset(0.0, CARD_TOP, || {
                    self.draw_page(scene, scene.page, inner0, inner1, alpha)
                });
            }
        }
        if scene.page_list.len() > 1 {
            // Arrows to the pages on either side.
            for (direction, icon) in [(-1.0, glyph::CHEVRON_LEFT), (1.0, glyph::CHEVRON_RIGHT)] {
                let (ax, ay) = card_arrow(direction, scene.width, scene.height);
                self.light.SetOpacity(0.12 * alpha);
                self.target.FillEllipse(
                    &D2D1_ELLIPSE {
                        point: pt(ax, ay),
                        radiusX: ARROW_RADIUS,
                        radiusY: ARROW_RADIUS,
                    },
                    &self.light,
                );
                self.light.SetOpacity(0.85 * alpha);
                self.draw_small_glyph(icon, ax, ay, &self.light);
            }
        }
        self.draw_page_dots(
            scene,
            alpha * (1.0 - scene.nav_visible * 1.6).clamp(0.0, 1.0),
        );
        self.draw_page_icons(scene, alpha);
        self.target.PopAxisAlignedClip();
    }

    /// Page dots at the card's foot, the one showing drawn as a pill.
    unsafe fn draw_page_dots(&self, scene: &Scene, alpha: f32) {
        if alpha < 0.01 {
            return;
        }
        let y = scene.height.min(CARD.1) - 8.0;
        let (dot, pill, gap) = (5.0, 16.0, 5.0);
        let pages = scene.page_list.len();
        let total = (pages - 1) as f32 * (dot + gap) + pill;
        let mut x = CANVAS.0 / 2.0 - total / 2.0;
        for index in 0..pages {
            let current = index == scene.page_index;
            let width = if current { pill } else { dot };
            self.light
                .SetOpacity(if current { 0.9 } else { 0.3 } * alpha);
            self.target.FillRoundedRectangle(
                &D2D1_ROUNDED_RECT {
                    rect: rect(x, y - dot / 2.0, x + width, y + dot / 2.0),
                    radiusX: dot / 2.0,
                    radiusY: dot / 2.0,
                },
                &self.light,
            );
            x += width + gap;
        }
    }

    /// What the dots open into: a pill of small icons, the showing page's in
    /// its colour, then Settings past a thin line. Each icon rises into place
    /// a moment after the one before it.
    unsafe fn draw_page_icons(&self, scene: &Scene, alpha: f32) {
        let visible = scene.nav_visible.clamp(0.0, 1.0);
        if visible < 0.01 {
            return;
        }
        const STAGGER: f32 = 0.05;
        let icons: Vec<&str> = scene
            .page_list
            .iter()
            .map(|page| page_glyph(*page))
            .chain([glyph::SETTINGS])
            .collect();
        let count = icons.len();
        let (first, y) = nav_item(0, count);
        let (last, _) = nav_item(count - 1, count);
        self.light.SetOpacity(0.1 * visible * alpha);
        self.target.FillRoundedRectangle(
            &D2D1_ROUNDED_RECT {
                rect: rect(first - 17.0, y - 13.0, last + 17.0, y + 13.0),
                radiusX: 13.0,
                radiusY: 13.0,
            },
            &self.light,
        );
        let line = last - NAV_STEP / 2.0 - NAV_GAP / 2.0;
        self.light.SetOpacity(0.2 * visible * alpha);
        self.target
            .FillRectangle(&rect(line - 0.5, y - 7.0, line + 0.5, y + 7.0), &self.light);
        let span = 1.0 - STAGGER * (count - 1) as f32;
        for (index, icon) in icons.into_iter().enumerate() {
            let t = ((visible - index as f32 * STAGGER) / span).clamp(0.0, 1.0);
            let eased = t * t * (3.0 - 2.0 * t);
            let (x, y) = nav_item(index, count);
            let y = y + 6.0 * (1.0 - eased);
            if index == scene.page_index {
                let (_, _, accent) = page_title(scene.page, "", "", "");
                self.accent.SetColor(&accent_color(accent));
                self.accent.SetOpacity(eased * alpha);
                self.draw_nav_glyph(icon, x, y, &self.accent);
            } else {
                self.light.SetOpacity(0.75 * eased * alpha);
                self.draw_nav_glyph(icon, x, y, &self.light);
            }
        }
    }

    /// Whether the mouse at (`x`, `y`) keeps the page icons open, or, while
    /// they are closed, is on the dots that open them.
    pub fn nav_zone_at(&self, x: f32, y: f32, open: bool, pages: usize) -> bool {
        if open {
            return y >= CARD.1 - 16.0;
        }
        let half = (pages as f32 * 10.0 + 40.0) / 2.0;
        (x - CANVAS.0 / 2.0).abs() <= half && y >= CARD.1 - 16.0
    }

    /// Which of the `count` page icons is at (`x`, `y`).
    pub fn nav_item_at(&self, x: f32, y: f32, count: usize) -> Option<usize> {
        (0..count).find(|&index| {
            let (ix, iy) = nav_item(index, count);
            (x - ix).abs() <= NAV_STEP / 2.0 && (y - iy).abs() <= 14.0
        })
    }

    /// The row across the top of the open card: the page's name on the left,
    /// battery, date and time in the middle, music bars on the right. The
    /// date and time stay put while pages switch (`fixed` draws them).
    unsafe fn draw_card_header(
        &self,
        scene: &Scene,
        page: Page,
        x0: f32,
        x1: f32,
        alpha: f32,
        fixed: bool,
    ) {
        let cy = 17.0;
        let month = calendar::MONTHS[scene.calendar.month as usize - 1];
        let year = scene.calendar.year.to_string();
        let (first, second, accent) = page_title(page, scene.place, month, &year);
        // A place name stops short of the date in the middle.
        let second_room = if page == Page::Prayer { 84.0 } else { 140.0 };
        let mut x = x0 + 20.0;
        for (word, brush, room) in [
            (first, &self.light, 140.0),
            (second, &self.accent, second_room),
        ] {
            if let Some(text) = self.text_layout(word, &self.title_text, room) {
                self.accent.SetColor(&accent_color(accent));
                brush.SetOpacity(alpha);
                self.target.DrawTextLayout(
                    pt(x, cy - text.height / 2.0),
                    &text.layout,
                    brush,
                    D2D1_DRAW_TEXT_OPTIONS_NONE,
                );
                x += text.width + 5.0;
            }
        }
        if !fixed {
            return;
        }
        let card_alpha = scene.card_visible.min(1.0);
        // The home page shows its own clock, so this one steps aside there.
        let shown = match (scene.page, scene.previous_page) {
            (Page::Status, Some(_)) => 1.0 - scene.page_mix,
            (Page::Status, None) => 0.0,
            (_, Some(Page::Status)) => scene.page_mix,
            _ => 1.0,
        };
        let alpha = card_alpha * shown;

        // Battery ring, date, and the time in bold, centred as one group.
        let date = self.text_layout(scene.status.date, &self.subtitle_text, 160.0);
        let time = self.text_layout(scene.clock, &self.text_left, 80.0);
        let (Some(date), Some(time)) = (date, time) else {
            return;
        };
        let ring = if scene.status.battery.is_some() {
            20.0
        } else {
            0.0
        };
        let total = ring + date.width + 8.0 + time.width;
        let mut x = (x0 + x1) / 2.0 - total / 2.0;
        if let Some((percent, charging)) = scene.status.battery {
            self.draw_battery(x + 6.0, cy, 6.0, percent, charging, alpha);
            x += ring;
        }
        self.draw_layout(&date.layout, x, cy - date.height / 2.0, 0.75 * alpha);
        x += date.width + 8.0;
        self.draw_layout(&time.layout, x, cy - time.height / 2.0, alpha);

        // Music bars while something plays.
        if let Some(music) = scene.music.as_ref().filter(|music| music.playing) {
            let (bar, gap, tallest) = (2.5, 2.5, 12.0);
            let right = x1 - 20.0;
            for (i, level) in music.bars.iter().enumerate() {
                let left = right - (4 - i) as f32 * (bar + gap) + gap;
                let half = (3.0 + (tallest - 3.0) * level.clamp(0.0, 1.0)) / 2.0;
                self.light.SetOpacity(0.85 * card_alpha);
                self.target.FillRoundedRectangle(
                    &D2D1_ROUNDED_RECT {
                        rect: rect(left, cy - half, left + bar, cy + half),
                        radiusX: bar / 2.0,
                        radiusY: bar / 2.0,
                    },
                    &self.light,
                );
            }
        }
    }

    /// Which side arrow is at (`x`, `y`) on the open card: -1 for the
    /// previous page, 1 for the next.
    pub fn card_arrow_at(&self, x: f32, y: f32, width: f32, height: f32) -> Option<f32> {
        [-1.0, 1.0].into_iter().find(|&direction| {
            let (ax, ay) = card_arrow(direction, width, height);
            (x - ax).powi(2) + (y - ay).powi(2) <= (ARROW_RADIUS + 4.0).powi(2)
        })
    }

    /// One page of the open card.
    unsafe fn draw_page(&self, scene: &Scene, page: Page, x0: f32, x1: f32, alpha: f32) {
        if alpha < 0.01 {
            return;
        }
        match (page, &scene.music, &scene.timer) {
            (Page::Shelf, _, _) => self.draw_shelf_card(&scene.shelf, x0, x1, alpha),
            (Page::Todo, _, _) => self.draw_todo_card(&scene.todo, x0, x1, alpha),
            (Page::Prayer, _, _) if scene.prayer.is_some() => {
                if let Some(prayer) = &scene.prayer {
                    self.draw_prayer_card(prayer, x0, x1, alpha)
                }
            }
            (Page::Music, Some(music), _) => {
                self.draw_music_card(music, scene.width - 2.0 * CARD_SIDE, alpha)
            }
            (Page::Calendar, _, _) => {
                self.draw_calendar_card(&scene.calendar, &scene.agenda, x0, x1, alpha)
            }
            (Page::Pomodoro, _, Some(timer)) => self.draw_pomodoro_card(timer, x0, alpha),
            _ => self.draw_status_card(scene, x0, x1, alpha),
        }
    }

    /// Draws with everything scaled by `scale` from the top center.
    unsafe fn with_scale(&self, scale: f32, draw: impl FnOnce()) {
        let scaled = Matrix3x2 {
            M11: scale,
            M12: 0.0,
            M21: 0.0,
            M22: scale,
            M31: CANVAS.0 / 2.0 * (1.0 - scale),
            M32: 0.0,
        };
        self.target.SetTransform(&scaled);
        draw();
        self.target.SetTransform(&Matrix3x2 {
            M11: 1.0,
            M22: 1.0,
            M31: 0.0,
            ..scaled
        });
    }

    /// Draws with everything shifted `dx` to the right and `dy` down.
    unsafe fn with_offset(&self, dx: f32, dy: f32, draw: impl FnOnce()) {
        let shifted = Matrix3x2 {
            M11: 1.0,
            M12: 0.0,
            M21: 0.0,
            M22: 1.0,
            M31: dx,
            M32: dy,
        };
        self.target.SetTransform(&shifted);
        draw();
        self.target.SetTransform(&Matrix3x2 {
            M31: 0.0,
            M32: 0.0,
            ..shifted
        });
    }

    /// The phase's progress as a small ring, and the time left. Both change
    /// once a second, so a running timer costs next to nothing.
    unsafe fn draw_timer_strip(&self, timer: &TimerScene, x0: f32, x1: f32, h: f32, alpha: f32) {
        let cy = h / 2.0;
        let color = accent_color(if timer.on_break {
            Accent::Teal
        } else {
            Accent::Red
        });
        // Ring on the left, the countdown on the right; the clock stays in
        // the middle.
        let ring = if timer.running { 1.0 } else { 0.45 };
        let _ = self.draw_ring(x0 + 20.0, cy, 6.5, timer.progress, color, ring * alpha);
        if let Some(text) = self.text_layout(&timer.left, &self.text_left, 80.0) {
            let x = x1 - 14.0 - text.width;
            self.accent
                .SetOpacity(if timer.running { 1.0 } else { 0.6 } * alpha);
            self.target.DrawTextLayout(
                pt(x, cy - text.height / 2.0),
                &text.layout,
                &self.accent,
                D2D1_DRAW_TEXT_OPTIONS_NONE,
            );
        }
    }

    /// A prayer icon on the left, "Maghrib 12 mnt" on the right.
    unsafe fn draw_prayer_strip(&self, label: &str, x0: f32, x1: f32, h: f32, alpha: f32) {
        let cy = h / 2.0;
        self.accent.SetColor(&accent_color(Accent::Green));
        self.accent.SetOpacity(alpha);
        self.draw_glyph(glyph::PRAYER, x0 + 24.0, cy, &self.accent);
        if let Some(text) = self.text_layout(label, &self.small_left, 120.0) {
            let x = x1 - 14.0 - text.width;
            self.draw_layout(&text.layout, x, cy - text.height / 2.0, 0.9 * alpha);
        }
    }

    /// Month grid on the left, the next events on the right.
    unsafe fn draw_calendar_card(
        &self,
        calendar: &CalendarScene,
        agenda: &[AgendaItem],
        x0: f32,
        x1: f32,
        alpha: f32,
    ) {
        let layout = CalendarRects::new(x0, x1);
        self.light.SetOpacity(0.8 * alpha);
        for ((cx, cy), icon) in [
            (layout.previous_month, glyph::CHEVRON_LEFT),
            (layout.next_month, glyph::CHEVRON_RIGHT),
        ] {
            self.draw_glyph(icon, cx, cy, &self.light);
        }

        let cell = |column: u32, row: f32| {
            let left = layout.grid_left + column as f32 * CELL.0;
            rect(left, row, left + CELL.0, row + CELL.1)
        };
        for (column, name) in calendar::WEEKDAYS.iter().enumerate() {
            self.draw_text(name, &self.tiny, cell(column as u32, 15.0), 0.45 * alpha);
        }
        let first = calendar::weekday(calendar.year, calendar.month, 1);
        let days = calendar::days_in_month(calendar.year, calendar.month);
        for day in 1..=days {
            let index = first + day - 1;
            let (column, row) = (index % 7, index / 7);
            let bounds = cell(column, GRID_TOP + row as f32 * CELL.1);
            let today = calendar.today == Some(day);
            if today {
                self.accent.SetColor(&accent_color(Accent::Teal));
                self.accent.SetOpacity(alpha);
                self.target.FillEllipse(
                    &D2D1_ELLIPSE {
                        point: pt(
                            (bounds.left + bounds.right) / 2.0,
                            (bounds.top + bounds.bottom) / 2.0,
                        ),
                        radiusX: 9.0,
                        radiusY: 9.0,
                    },
                    &self.accent,
                );
            }
            let number = day.to_string();
            if today {
                self.black.SetOpacity(alpha);
                let wide: Vec<u16> = number.encode_utf16().collect();
                self.target.DrawText(
                    &wide,
                    &self.tiny,
                    &bounds,
                    &self.black,
                    D2D1_DRAW_TEXT_OPTIONS_NONE,
                    DWRITE_MEASURING_MODE_NATURAL,
                );
                self.black.SetOpacity(1.0);
            } else if column == 6 {
                // Sundays in red, as on Indonesian calendars.
                self.accent.SetColor(&color(1.0, 0.45, 0.45, 1.0));
                self.accent.SetOpacity(0.9 * alpha);
                let wide: Vec<u16> = number.encode_utf16().collect();
                self.target.DrawText(
                    &wide,
                    &self.tiny,
                    &bounds,
                    &self.accent,
                    D2D1_DRAW_TEXT_OPTIONS_NONE,
                    DWRITE_MEASURING_MODE_NATURAL,
                );
            } else {
                self.draw_text(&number, &self.tiny, bounds, 0.9 * alpha);
            }
            if calendar.marked.contains(&day) {
                // A dot under days with something on the agenda.
                self.accent.SetColor(&accent_color(Accent::Blue));
                self.accent.SetOpacity(alpha);
                self.target.FillEllipse(
                    &D2D1_ELLIPSE {
                        point: pt((bounds.left + bounds.right) / 2.0, bounds.bottom - 1.0),
                        radiusX: 1.6,
                        radiusY: 1.6,
                    },
                    &self.accent,
                );
            }
        }

        // The next events.
        let left = layout.agenda_left;
        let room = layout.agenda_right - left;
        if let Some(text) = self.text_layout("Agenda", &self.subtitle_text, room) {
            self.draw_layout(&text.layout, left, 16.0, 0.55 * alpha);
        }
        if agenda.is_empty() {
            if let Some(text) = self.text_layout("Tidak ada acara", &self.small_left, room) {
                self.draw_layout(&text.layout, left, AGENDA_TOP + 2.0, 0.5 * alpha);
            }
        }
        for (index, item) in agenda.iter().take(AGENDA_ROWS).enumerate() {
            let top = AGENDA_TOP + index as f32 * AGENDA_ROW;
            let when = format!("{} \u{b7} {}", item.day, item.time);
            if let Some(text) = self.text_layout(&when, &self.small_left, room) {
                self.draw_layout(&text.layout, left, top, 0.55 * alpha);
            }
            if let Some(text) = self.text_layout(item.summary, &self.subtitle_text, room) {
                self.draw_layout(&text.layout, left, top + 14.0, 0.95 * alpha);
            }
            if item.today {
                self.accent.SetColor(&accent_color(Accent::Blue));
                self.accent.SetOpacity(alpha);
                self.target.FillEllipse(
                    &D2D1_ELLIPSE {
                        point: pt(left - 8.0, top + 8.0),
                        radiusX: 2.5,
                        radiusY: 2.5,
                    },
                    &self.accent,
                );
            }
        }
    }

    /// The time left in a ring, with the phase and controls beside it.
    unsafe fn draw_pomodoro_card(&self, timer: &TimerScene, x0: f32, alpha: f32) {
        let layout = PomodoroRects::new(x0);
        let (cx, cy) = (x0 + POMODORO_RING.0, POMODORO_RING.1);
        let accent = if timer.on_break {
            Accent::Teal
        } else {
            Accent::Red
        };
        let _ = self.draw_ring(cx, cy, 56.0, timer.progress, accent_color(accent), alpha);
        if let Some(text) = self.text_layout(&timer.left, &self.big_clock, 120.0) {
            self.draw_layout(
                &text.layout,
                cx - text.width / 2.0,
                cy - text.height / 2.0,
                alpha,
            );
        }
        let left = x0 + POMODORO_LEFT;
        if let Some(text) = self.text_layout(timer.label, &self.title_text, 150.0) {
            self.draw_layout(&text.layout, left, 32.0, alpha);
        }
        let session = format!("Sesi {}", timer.session);
        if let Some(text) = self.text_layout(&session, &self.subtitle_text, 150.0) {
            self.draw_layout(&text.layout, left, 54.0, 0.6 * alpha);
        }
        let (icon, label) = if timer.running {
            (glyph::PAUSE, "Jeda")
        } else {
            (glyph::PLAY, "Mulai")
        };
        self.draw_pill(layout.start_pause, icon, label, Some(accent), alpha);
        let (rx, ry) = layout.reset;
        self.light.SetOpacity(0.12 * alpha);
        self.target.FillEllipse(
            &D2D1_ELLIPSE {
                point: pt(rx, ry),
                radiusX: 14.0,
                radiusY: 14.0,
            },
            &self.light,
        );
        self.light.SetOpacity(0.85 * alpha);
        self.draw_small_glyph_at(glyph::RESET, rx, ry, &self.light);
    }

    /// Keeps a thumbnail for each shelf file, making new ones as files arrive.
    pub fn sync_thumbnails(&mut self, paths: &[PathBuf]) {
        let mut thumbs = Vec::with_capacity(paths.len());
        for path in paths {
            let existing = self
                .thumbs
                .iter()
                .position(|(known, _)| known == path)
                .map(|index| self.thumbs.swap_remove(index));
            thumbs.push(existing.unwrap_or_else(|| {
                let bitmap = unsafe { self.thumbnail(path).ok() };
                (path.clone(), bitmap)
            }));
        }
        self.thumbs = thumbs;
    }

    /// The shell's own preview: a picture for images and videos, the file
    /// type icon for everything else.
    unsafe fn thumbnail(&self, path: &Path) -> Result<ID2D1Bitmap> {
        let factory: IShellItemImageFactory =
            SHCreateItemFromParsingName(&HSTRING::from(path.as_os_str()), None)?;
        let side = THUMB_PIXELS as i32;
        let bitmap = factory.GetImage(SIZE { cx: side, cy: side }, SIIGBF_RESIZETOFIT)?;
        let wic: IWICImagingFactory =
            CoCreateInstance(&CLSID_WICImagingFactory, None, CLSCTX_INPROC_SERVER)?;
        let source = wic.CreateBitmapFromHBITMAP(
            bitmap,
            HPALETTE::default(),
            WICBitmapUsePremultipliedAlpha,
        );
        let _ = DeleteObject(bitmap.into());
        let converter = wic.CreateFormatConverter()?;
        converter.Initialize(
            &source?,
            &GUID_WICPixelFormat32bppPBGRA,
            WICBitmapDitherTypeNone,
            None,
            0.0,
            WICBitmapPaletteTypeMedianCut,
        )?;
        self.target.CreateBitmapFromWicBitmap(&converter, None)
    }

    /// Dashed outline with an invitation to drop.
    unsafe fn draw_drop_zone(&self, x0: f32, x1: f32, h: f32, alpha: f32) {
        let inset = rect(x0 + 10.0, 8.0, x1 - 10.0, h - 10.0);
        self.light.SetOpacity(0.45 * alpha);
        self.target.DrawRoundedRectangle(
            &D2D1_ROUNDED_RECT {
                rect: inset,
                radiusX: 14.0,
                radiusY: 14.0,
            },
            &self.light,
            1.5,
            &self.dashes,
        );
        let cy = (inset.top + inset.bottom) / 2.0;
        self.light.SetOpacity(alpha);
        self.draw_glyph(glyph::DOWNLOAD, x0 + 42.0, cy, &self.light);
        if let Some(title) = self.text_layout("Lepaskan untuk menyimpan", &self.title_text, 230.0) {
            self.draw_layout(&title.layout, x0 + 66.0, cy - 19.0, alpha);
        }
        if let Some(note) =
            self.text_layout("Berkas tetap di tempat aslinya", &self.subtitle_text, 230.0)
        {
            self.draw_layout(&note.layout, x0 + 66.0, cy + 1.0, 0.55 * alpha);
        }
    }

    /// File tiles with their names, a remove button on the one under the
    /// mouse, and "Hapus semua".
    unsafe fn draw_shelf_card(&self, shelf: &ShelfScene, x0: f32, x1: f32, alpha: f32) {
        let layout = ShelfRects::new(x0, x1);
        let heading = format!("{} berkas", shelf.items.len());
        if let Some(text) = self.text_layout(&heading, &self.title_text, 220.0) {
            self.draw_layout(&text.layout, x0 + 20.0, 8.0, alpha);
        }
        self.draw_pill(layout.clear, glyph::DELETE, "Hapus semua", None, alpha);

        for (index, path) in shelf.items.iter().enumerate() {
            let tile = layout.tile(index);
            if shelf.hover == Some(index) {
                self.light.SetOpacity(0.08 * alpha);
                self.target.FillRoundedRectangle(
                    &D2D1_ROUNDED_RECT {
                        rect: tile,
                        radiusX: 10.0,
                        radiusY: 10.0,
                    },
                    &self.light,
                );
            }
            let thumb = rect(
                tile.left + 18.0,
                tile.top + 1.0,
                tile.left + 46.0,
                tile.top + 29.0,
            );
            let bitmap = self
                .thumbs
                .iter()
                .find(|(known, _)| known == path)
                .and_then(|(_, bitmap)| bitmap.as_ref());
            match bitmap {
                Some(bitmap) => {
                    // Fit inside the square, keeping the picture's shape.
                    let size = bitmap.GetSize();
                    let fit = (28.0 / size.width).min(28.0 / size.height);
                    let (w, h) = (size.width * fit, size.height * fit);
                    let cx = (thumb.left + thumb.right) / 2.0;
                    let cy = (thumb.top + thumb.bottom) / 2.0;
                    self.target.DrawBitmap(
                        bitmap,
                        Some(&rect(
                            cx - w / 2.0,
                            cy - h / 2.0,
                            cx + w / 2.0,
                            cy + h / 2.0,
                        )),
                        alpha,
                        D2D1_BITMAP_INTERPOLATION_MODE_LINEAR,
                        None,
                    );
                }
                None => {
                    self.light.SetOpacity(0.7 * alpha);
                    let cx = (thumb.left + thumb.right) / 2.0;
                    self.draw_glyph(
                        glyph::FILE,
                        cx,
                        (thumb.top + thumb.bottom) / 2.0,
                        &self.light,
                    );
                }
            }
            let name = rect(
                tile.left - 2.0,
                tile.top + 29.0,
                tile.right + 2.0,
                tile.top + 45.0,
            );
            self.draw_text(
                &shelf::display_name(path),
                &self.tile_text,
                name,
                0.85 * alpha,
            );

            if shelf.hover == Some(index) {
                let (cx, cy) = layout.remove(index);
                self.light.SetOpacity(0.9 * alpha);
                self.target.FillEllipse(
                    &D2D1_ELLIPSE {
                        point: pt(cx, cy),
                        radiusX: 8.0,
                        radiusY: 8.0,
                    },
                    &self.light,
                );
                self.black.SetOpacity(alpha);
                let wide: Vec<u16> = glyph::CLOSE.encode_utf16().collect();
                self.target.DrawText(
                    &wide,
                    &self.close_glyph,
                    &rect(cx - 8.0, cy - 8.0, cx + 8.0, cy + 8.0),
                    &self.black,
                    D2D1_DRAW_TEXT_OPTIONS_NONE,
                    DWRITE_MEASURING_MODE_NATURAL,
                );
                self.black.SetOpacity(1.0);
            }
        }
    }

    /// What sits at (`x`, `y`) on the shelf card holding `count` files.
    pub fn shelf_hit(&self, x: f32, y: f32, width: f32, count: usize) -> Option<ShelfHit> {
        let (y, width) = on_page(y, width);
        let cx = CANVAS.0 / 2.0;
        let layout = ShelfRects::new(cx - width / 2.0, cx + width / 2.0);
        let inside = |r: D2D_RECT_F| x >= r.left && x < r.right && y >= r.top && y < r.bottom;
        if inside(layout.clear) {
            return Some(ShelfHit::ClearAll);
        }
        for index in 0..count {
            let (rx, ry) = layout.remove(index);
            if (x - rx).powi(2) + (y - ry).powi(2) <= 81.0 {
                return Some(ShelfHit::Remove(index));
            }
        }
        (0..count)
            .find(|&index| inside(layout.tile(index)))
            .map(ShelfHit::Tile)
    }

    /// The task list: a heading with the add button, or the field for a new
    /// task while typing, then one row per task.
    unsafe fn draw_todo_card(&self, todo: &TodoScene, x0: f32, x1: f32, alpha: f32) {
        let layout = TodoRects { x0, x1 };
        match todo.editing {
            Some(text) => self.draw_todo_field(text, todo.caret, layout.field(), alpha),
            None => {
                let done = todo.tasks.len() - todo.left;
                if !todo.tasks.is_empty() {
                    let count = format!("{done}/{}", todo.tasks.len());
                    if let Some(text) = self.text_layout(&count, &self.title_text, 80.0) {
                        self.draw_layout(&text.layout, x0 + 20.0, 21.0 - text.height / 2.0, alpha);
                        let caption = if todo.left == 0 {
                            "semua selesai"
                        } else {
                            "selesai"
                        };
                        if let Some(label) = self.text_layout(caption, &self.small_left, 120.0) {
                            let x = x0 + 26.0 + text.width;
                            self.draw_layout(
                                &label.layout,
                                x,
                                21.0 - label.height / 2.0,
                                0.55 * alpha,
                            );
                        }
                    }
                }
                if done > 0 {
                    self.draw_pill(layout.clear(), glyph::DELETE, "Hapus selesai", None, alpha);
                }
            }
        }

        // Add, or save while typing.
        let full = todo.tasks.len() >= todo::CAPACITY;
        if todo.editing.is_some() {
            self.draw_pill(
                layout.add(),
                glyph::CHECK,
                "Simpan",
                Some(Accent::Green),
                alpha,
            );
        } else {
            let opacity = if full { 0.4 } else { 1.0 };
            self.draw_pill(layout.add(), glyph::ADD, "Tambah", None, opacity * alpha);
        }

        if todo.tasks.is_empty() && todo.editing.is_none() {
            self.draw_text(
                "Belum ada tugas \u{b7} klik Tambah",
                &self.tiny,
                rect(x0, 54.0, x1, 54.0 + TODO_ROW),
                0.5 * alpha,
            );
        }
        for (index, task) in todo.tasks.iter().enumerate() {
            let row = layout.row(index);
            let hovered = todo.hover == Some(index);
            if hovered {
                self.light.SetOpacity(0.06 * alpha);
                self.target.FillRoundedRectangle(
                    &D2D1_ROUNDED_RECT {
                        rect: row,
                        radiusX: 8.0,
                        radiusY: 8.0,
                    },
                    &self.light,
                );
            }
            let (cx, cy) = layout.check(index);
            if task.done {
                self.accent.SetColor(&accent_color(Accent::Green));
                self.accent.SetOpacity(alpha);
                self.target.FillEllipse(
                    &D2D1_ELLIPSE {
                        point: pt(cx, cy),
                        radiusX: 8.0,
                        radiusY: 8.0,
                    },
                    &self.accent,
                );
                self.black.SetOpacity(alpha);
                self.draw_small_glyph(glyph::CHECK, cx, cy, &self.black);
                self.black.SetOpacity(1.0);
            } else {
                self.light.SetOpacity(0.55 * alpha);
                self.target.DrawEllipse(
                    &D2D1_ELLIPSE {
                        point: pt(cx, cy),
                        radiusX: 7.5,
                        radiusY: 7.5,
                    },
                    &self.light,
                    1.4,
                    None,
                );
            }
            let text_left = row.left + 34.0;
            let room = row.right - 34.0 - text_left;
            if let Some(text) = self.text_layout(&task.text, &self.subtitle_text, room) {
                if task.done {
                    let _ = text.layout.SetStrikethrough(
                        true,
                        DWRITE_TEXT_RANGE {
                            startPosition: 0,
                            length: u32::MAX,
                        },
                    );
                }
                let opacity = if task.done { 0.4 } else { 0.92 };
                self.draw_layout(
                    &text.layout,
                    text_left,
                    cy - text.height / 2.0,
                    opacity * alpha,
                );
            }
            if hovered {
                let (rx, ry) = layout.remove(index);
                self.light.SetOpacity(0.6 * alpha);
                self.draw_small_glyph(glyph::CLOSE, rx, ry, &self.light);
            }
        }
    }

    /// The field for a new task: what was typed so far, scrolled to keep the
    /// end in view, and the blinking cursor.
    unsafe fn draw_todo_field(&self, text: &str, caret: bool, field: D2D_RECT_F, alpha: f32) {
        self.light.SetOpacity(0.1 * alpha);
        let height = field.bottom - field.top;
        self.target.FillRoundedRectangle(
            &D2D1_ROUNDED_RECT {
                rect: field,
                radiusX: height / 2.0,
                radiusY: height / 2.0,
            },
            &self.light,
        );
        let left = field.left + 14.0;
        let room = field.right - 14.0 - left;
        let cy = (field.top + field.bottom) / 2.0;
        let mut end = left;
        if text.is_empty() {
            if let Some(hint) =
                self.text_layout("Tugas baru, Enter untuk simpan", &self.subtitle_text, room)
            {
                self.draw_layout(&hint.layout, left, cy - hint.height / 2.0, 0.4 * alpha);
            }
        } else {
            let wide: Vec<u16> = text.encode_utf16().collect();
            let Ok(layout) =
                self.dwrite
                    .CreateTextLayout(&wide, &self.subtitle_text, 4000.0, 100.0)
            else {
                return;
            };
            let mut metrics = DWRITE_TEXT_METRICS::default();
            let _ = layout.GetMetrics(&mut metrics);
            let width = metrics.widthIncludingTrailingWhitespace;
            let shift = (width - room).max(0.0);
            self.target.PushAxisAlignedClip(
                &rect(left - 2.0, field.top, field.right - 10.0, field.bottom),
                D2D1_ANTIALIAS_MODE_PER_PRIMITIVE,
            );
            self.draw_layout(&layout, left - shift, cy - metrics.height / 2.0, alpha);
            self.target.PopAxisAlignedClip();
            end = left + width - shift;
        }
        if caret {
            self.light.SetOpacity(0.9 * alpha);
            self.target
                .FillRectangle(&rect(end + 1.0, cy - 8.0, end + 2.5, cy + 8.0), &self.light);
        }
    }

    /// What sits at (`x`, `y`) on the task card.
    pub fn todo_hit(&self, x: f32, y: f32, width: f32, todo: &TodoScene) -> Option<TodoHit> {
        let (y, width) = on_page(y, width);
        let cx = CANVAS.0 / 2.0;
        let layout = TodoRects {
            x0: cx - width / 2.0,
            x1: cx + width / 2.0,
        };
        let inside = |r: D2D_RECT_F| x >= r.left && x < r.right && y >= r.top && y < r.bottom;
        if inside(layout.add()) {
            return Some(TodoHit::Add);
        }
        if todo.editing.is_none() && todo.tasks.len() > todo.left && inside(layout.clear()) {
            return Some(TodoHit::ClearDone);
        }
        let index = (0..todo.tasks.len()).find(|&index| inside(layout.row(index)))?;
        let (rx, ry) = layout.remove(index);
        if (x - rx).powi(2) + (y - ry).powi(2) <= 12.0_f32.powi(2) {
            Some(TodoHit::Remove(index))
        } else {
            Some(TodoHit::Toggle(index))
        }
    }

    /// The next prayer and the wait for it on the left; today's times along
    /// the sun's path on the right.
    unsafe fn draw_prayer_card(&self, prayer: &PrayerScene, x0: f32, x1: f32, alpha: f32) {
        let left = x0 + 16.0;
        let caption = if prayer.next_today.is_none() {
            "Berikutnya, besok"
        } else {
            "Berikutnya"
        };
        if let Some(text) = self.text_layout(caption, &self.subtitle_text, 150.0) {
            self.draw_layout(&text.layout, left, 14.0, 0.55 * alpha);
        }
        if let Some(text) = self.text_layout(prayer.next_name, &self.big_clock, 150.0) {
            self.draw_layout(&text.layout, left, 30.0, alpha);
        }
        self.accent.SetColor(&accent_color(Accent::Blue));
        self.accent.SetOpacity(alpha);
        if let Some(time) = self.text_layout(&prayer.next_time, &self.text_left, 60.0) {
            let cy = 84.0;
            self.draw_layout(&time.layout, left, cy - time.height / 2.0, alpha);
            let wait = format!("dalam {}", prayer.wait);
            if let Some(text) = self.text_layout(&wait, &self.small_left, 104.0) {
                self.target.DrawTextLayout(
                    pt(left + time.width + 8.0, cy - text.height / 2.0),
                    &text.layout,
                    &self.accent,
                    D2D1_DRAW_TEXT_OPTIONS_NONE,
                );
            }
        }
        let bar = |right: f32, brush: &ID2D1SolidColorBrush| {
            self.target.FillRoundedRectangle(
                &D2D1_ROUNDED_RECT {
                    rect: rect(left, 104.0, right, 108.0),
                    radiusX: 2.0,
                    radiusY: 2.0,
                },
                brush,
            );
        };
        self.light.SetOpacity(0.14 * alpha);
        bar(left + 150.0, &self.light);
        bar(left + 150.0 * prayer.progress.clamp(0.0, 1.0), &self.accent);

        // The sun's path, one evenly spaced column per prayer.
        let first = x0 + 176.0;
        let column = (x1 - 16.0 - first) / 5.0;
        let at_x = |position: f32| first + column * (position + 0.5);
        let (base, rise) = (100.0, 68.0);
        let at_y = |position: f32| base - rise * sun_arc(position);
        let _ = self.draw_sun_path(&at_x, &at_y, base, base - rise, alpha);
        for index in 0..5 {
            let position = index as f32;
            let opacity = if position <= prayer.sun { 0.95 } else { 0.5 };
            self.light.SetOpacity(opacity * alpha);
            self.target.FillEllipse(
                &D2D1_ELLIPSE {
                    point: pt(at_x(position), at_y(position)),
                    radiusX: 3.0,
                    radiusY: 3.0,
                },
                &self.light,
            );
        }
        // The sun itself, softly glowing.
        self.accent.SetColor(&color(1.0, 0.82, 0.45, 1.0));
        for (radius, opacity) in [(13.0, 0.14), (9.0, 0.3), (5.5, 1.0)] {
            self.accent.SetOpacity(opacity * alpha);
            self.target.FillEllipse(
                &D2D1_ELLIPSE {
                    point: pt(at_x(prayer.sun), at_y(prayer.sun)),
                    radiusX: radius,
                    radiusY: radius,
                },
                &self.accent,
            );
        }

        // Today's five, the next one brightest, past ones faded.
        for (index, name) in prayer::NAMES.iter().enumerate() {
            let center = at_x(index as f32);
            let opacity = match prayer.next_today {
                Some(next) if index == next => 1.0,
                Some(next) if index > next => 0.75,
                _ => 0.45,
            };
            let name_box = rect(center - 26.0, 110.0, center + 26.0, 125.0);
            let time_box = rect(name_box.left, 125.0, name_box.right, 140.0);
            self.draw_text(name, &self.tiny, name_box, 0.8 * opacity * alpha);
            self.draw_text(&prayer.times[index], &self.tiny, time_box, opacity * alpha);
        }
    }

    /// The curve and the glow under it, from Subuh to Isya.
    unsafe fn draw_sun_path(
        &self,
        at_x: &dyn Fn(f32) -> f32,
        at_y: &dyn Fn(f32) -> f32,
        base: f32,
        top: f32,
        alpha: f32,
    ) -> Result<()> {
        const STEPS: u32 = 48;
        let point = |step: u32| {
            let position = step as f32 * 4.0 / STEPS as f32;
            pt(at_x(position), at_y(position))
        };
        let area = self.factory.CreatePathGeometry()?;
        let sink = area.Open()?;
        sink.BeginFigure(pt(at_x(0.0), base), D2D1_FIGURE_BEGIN_FILLED);
        (0..=STEPS).for_each(|step| sink.AddLine(point(step)));
        sink.AddLine(pt(at_x(4.0), base));
        sink.EndFigure(D2D1_FIGURE_END_CLOSED);
        sink.Close()?;
        let curve = self.factory.CreatePathGeometry()?;
        let sink = curve.Open()?;
        sink.BeginFigure(point(0), D2D1_FIGURE_BEGIN_HOLLOW);
        (1..=STEPS).for_each(|step| sink.AddLine(point(step)));
        sink.EndFigure(D2D1_FIGURE_END_OPEN);
        sink.Close()?;

        let blue = color(0.3, 0.55, 1.0, 1.0);
        let violet = color(0.62, 0.42, 1.0, 1.0);
        let glow = self.gradient(
            pt(0.0, top),
            pt(0.0, base),
            &[
                (0.0, color(0.3, 0.55, 1.0, 0.38)),
                (1.0, color(0.3, 0.55, 1.0, 0.0)),
            ],
        )?;
        glow.SetOpacity(alpha);
        self.target.FillGeometry(&area, &glow, None);
        let line = self.gradient(
            pt(at_x(0.0), 0.0),
            pt(at_x(4.0), 0.0),
            &[(0.0, blue), (1.0, violet)],
        )?;
        line.SetOpacity(alpha);
        self.target
            .DrawGeometry(&curve, &line, 2.2, &self.round_caps);
        Ok(())
    }

    /// A straight gradient between two points.
    unsafe fn gradient(
        &self,
        start: Vector2,
        end: Vector2,
        stops: &[(f32, D2D1_COLOR_F)],
    ) -> Result<ID2D1LinearGradientBrush> {
        let stops: Vec<D2D1_GRADIENT_STOP> = stops
            .iter()
            .map(|&(position, color)| D2D1_GRADIENT_STOP { position, color })
            .collect();
        let collection = self.target.CreateGradientStopCollection(
            &stops,
            D2D1_GAMMA_2_2,
            D2D1_EXTEND_MODE_CLAMP,
        )?;
        self.target.CreateLinearGradientBrush(
            &D2D1_LINEAR_GRADIENT_BRUSH_PROPERTIES {
                startPoint: start,
                endPoint: end,
            },
            None,
            &collection,
        )
    }

    /// Which calendar control sits at (`x`, `y`) on the open card.
    pub fn calendar_control_at(&self, x: f32, y: f32, width: f32) -> Option<CalendarControl> {
        let (y, width) = on_page(y, width);
        let cx = CANVAS.0 / 2.0;
        let layout = CalendarRects::new(cx - width / 2.0, cx + width / 2.0);
        [
            (layout.previous_month, CalendarControl::PreviousMonth),
            (layout.next_month, CalendarControl::NextMonth),
        ]
        .into_iter()
        .find(|((cx, cy), _)| (x - cx).powi(2) + (y - cy).powi(2) <= HIT_RADIUS.powi(2))
        .map(|(_, control)| control)
    }

    /// Which timer control sits at (`x`, `y`) on the open card.
    pub fn pomodoro_control_at(&self, x: f32, y: f32, width: f32) -> Option<PomodoroControl> {
        let (y, width) = on_page(y, width);
        let layout = PomodoroRects::new(CANVAS.0 / 2.0 - width / 2.0);
        let pill = layout.start_pause;
        if x >= pill.left && x < pill.right && y >= pill.top && y < pill.bottom {
            return Some(PomodoroControl::StartPause);
        }
        let (rx, ry) = layout.reset;
        ((x - rx).powi(2) + (y - ry).powi(2) <= HIT_RADIUS.powi(2))
            .then_some(PomodoroControl::Reset)
    }

    /// Home: the time in a panel coloured like the sky, then the weather, the
    /// next prayer and what is left to do today.
    unsafe fn draw_status_card(&self, scene: &Scene, x0: f32, x1: f32, alpha: f32) {
        let text =
            |text: &str, format: &IDWriteTextFormat, x: f32, y: f32, room: f32, opacity: f32| {
                if let Some(block) = self.text_layout(text, format, room) {
                    self.draw_layout(&block.layout, x, y, opacity * alpha);
                }
            };
        let status = &scene.status;
        let hour = clock_hour(scene.clock);

        // The panel.
        let panel = rect(x0 + 6.0, 6.0, x0 + 190.0, 146.0);
        let (top, bottom) = sky_colors(hour);
        if let Ok(sky) = self.gradient(
            pt(0.0, panel.top),
            pt(0.0, panel.bottom),
            &[(0.0, top), (1.0, bottom)],
        ) {
            sky.SetOpacity(alpha);
            self.target.FillRoundedRectangle(
                &D2D1_ROUNDED_RECT {
                    rect: panel,
                    radiusX: 16.0,
                    radiusY: 16.0,
                },
                &sky,
            );
        }
        let left = panel.left + 16.0;
        let room = panel.right - 12.0 - left;
        text(greeting(hour), &self.subtitle_text, left, 16.0, room, 0.9);
        text(scene.clock, &self.big_clock, left, 33.0, room, 1.0);
        text(status.date, &self.subtitle_text, left, 75.0, room, 0.85);
        if let Some((percent, charging)) = status.battery {
            let cy = 122.0;
            self.draw_battery(left + 7.0, cy, 7.0, percent, charging, alpha);
            let label = if charging {
                format!("{percent}% \u{b7} mengisi")
            } else {
                format!("{percent}%")
            };
            text(
                &label,
                &self.small_left,
                left + 22.0,
                cy - 8.0,
                room - 22.0,
                0.9,
            );
        }

        // Beside it, three rows split by thin lines.
        let left = panel.right + 16.0;
        let right = x1 - 14.0;
        let room = right - left;
        let next = next_up(&scene.agenda, scene.todo.tasks);
        let lines: &[f32] = if next.is_some() {
            &[52.0, 108.0]
        } else {
            &[52.0]
        };
        for &y in lines {
            self.light.SetOpacity(0.08 * alpha);
            self.target
                .FillRectangle(&rect(left, y, right, y + 1.0), &self.light);
        }

        match &status.weather {
            Some(weather) => {
                text(weather.glyph, &self.symbols, left, 10.0, 40.0, 0.95);
                text(
                    &weather.temperature,
                    &self.title_text,
                    left + 38.0,
                    10.0,
                    60.0,
                    1.0,
                );
                let line = format!("{} \u{b7} {}", weather.description, weather.place);
                text(
                    &line,
                    &self.small_left,
                    left + 38.0,
                    30.0,
                    room - 38.0,
                    0.55,
                );
            }
            None => {
                text("Cuaca", &self.subtitle_text, left, 12.0, room, 0.75);
                text(status.weather_hint, &self.small_left, left, 30.0, room, 0.5);
            }
        }

        match &scene.prayer {
            Some(prayer) => {
                text(prayer.next_name, &self.title_text, left, 58.0, 100.0, 1.0);
                if let Some(time) = self.text_layout(&prayer.next_time, &self.text_left, 60.0) {
                    self.draw_layout(&time.layout, right - time.width, 60.0, alpha);
                }
                let wait = format!("dalam {}", prayer.wait);
                text(&wait, &self.small_left, left, 78.0, room, 0.55);
                let bar = |end: f32, brush: &ID2D1SolidColorBrush| {
                    self.target.FillRoundedRectangle(
                        &D2D1_ROUNDED_RECT {
                            rect: rect(left, 96.0, end, 99.0),
                            radiusX: 1.5,
                            radiusY: 1.5,
                        },
                        brush,
                    );
                };
                self.light.SetOpacity(0.14 * alpha);
                bar(right, &self.light);
                self.accent.SetColor(&accent_color(Accent::Blue));
                self.accent.SetOpacity(alpha);
                bar(left + room * prayer.progress.clamp(0.0, 1.0), &self.accent);
            }
            None => {
                text("Sholat", &self.subtitle_text, left, 60.0, room, 0.75);
                text(
                    "Atur lokasi di Pengaturan",
                    &self.small_left,
                    left,
                    78.0,
                    room,
                    0.5,
                );
            }
        }

        if let Some((line, more)) = next {
            let count = format!("+{more}");
            let badge = (more > 0)
                .then(|| self.text_layout(&count, &self.small_left, 40.0))
                .flatten();
            let room = room - badge.as_ref().map_or(0.0, |badge| badge.width + 8.0);
            text(&line, &self.subtitle_text, left, 118.0, room, 0.9);
            if let Some(badge) = badge {
                let x = right - badge.width;
                self.draw_layout(&badge.layout, x, 119.0, 0.55 * alpha);
            }
        }
    }

    /// Small art on the left, level bars on the right.
    unsafe fn draw_music_strip(&self, music: &MusicScene, x0: f32, x1: f32, h: f32, alpha: f32) {
        let cy = h / 2.0;
        self.draw_art(rect(x0 + 12.0, cy - 10.0, x0 + 32.0, cy + 10.0), 5.0, alpha);
        let (bar, gap, tallest) = (3.0, 3.0, 14.0);
        let right = x1 - 14.0;
        for (i, level) in music.bars.iter().enumerate() {
            let left = right - (4 - i) as f32 * (bar + gap) + gap;
            let half = (3.0 + (tallest - 3.0) * level.clamp(0.0, 1.0)) / 2.0;
            self.light.SetOpacity(0.92 * alpha);
            self.target.FillRoundedRectangle(
                &D2D1_ROUNDED_RECT {
                    rect: rect(left, cy - half, left + bar, cy + half),
                    radiusX: 1.5,
                    radiusY: 1.5,
                },
                &self.light,
            );
        }
    }

    /// Art beside the title and artist, progress in the cover's colour, the
    /// controls, the volume, and the lyrics when there are any.
    unsafe fn draw_music_card(&self, music: &MusicScene, width: f32, alpha: f32) {
        let card = CardRects::new(width, music.lyric.is_some());
        self.draw_art(card.art, 10.0, alpha);
        // The tint at full strength: bright on the dark card.
        let accent = self
            .art_tint
            .map_or(color(1.0, 1.0, 1.0, 1.0), |[r, g, b]| {
                let high = r.max(g).max(b).max(0.01);
                color(r / high, g / high, b / high, 1.0)
            });

        let column = card.text_right - card.text_left;
        if let Some(title) = self.text_layout(music.title, &self.title_text, column) {
            self.draw_layout(&title.layout, card.text_left, card.art.top + 6.0, alpha);
        }
        if let Some(artist) = self.text_layout(music.artist, &self.subtitle_text, column) {
            self.draw_layout(
                &artist.layout,
                card.text_left,
                card.art.top + 28.0,
                0.6 * alpha,
            );
        }

        let slider = |left: f32, right: f32, y: f32, fraction: f32, opacity: f32| {
            let bar = |end: f32, brush: &ID2D1SolidColorBrush| {
                self.target.FillRoundedRectangle(
                    &D2D1_ROUNDED_RECT {
                        rect: rect(left, y - 2.0, end, y + 2.0),
                        radiusX: 2.0,
                        radiusY: 2.0,
                    },
                    brush,
                );
            };
            self.light.SetOpacity(0.18 * opacity);
            bar(right, &self.light);
            self.accent.SetColor(&accent);
            self.accent.SetOpacity(opacity);
            bar(
                left + (right - left) * fraction.clamp(0.0, 1.0),
                &self.accent,
            );
        };

        // Progress, when the player reports a length. Whole seconds on both
        // sides, so the two always add up to the song.
        if music.duration > 0.0 {
            let (left, right, y) = card.progress;
            slider(
                left,
                right,
                y,
                (music.position / music.duration) as f32,
                alpha,
            );
            let elapsed = music.position.floor();
            let remaining = (music.duration.round() - elapsed).max(0.0);
            if let Some(text) = self.text_layout(&format_duration(elapsed), &self.small_left, 40.0)
            {
                let x = card.art.left;
                self.draw_layout(&text.layout, x, y - text.height / 2.0, 0.6 * alpha);
            }
            let left_text = format!("-{}", format_duration(remaining));
            if let Some(text) = self.text_layout(&left_text, &self.small_left, 44.0) {
                let x = card.text_right - text.width;
                self.draw_layout(&text.layout, x, y - text.height / 2.0, 0.6 * alpha);
            }
        }

        // Controls, as solid shapes.
        self.light.SetOpacity(alpha);
        let (px, py) = card.play;
        if music.playing {
            self.fill_pause(px, py, &self.light);
        } else {
            let _ = self.fill_play(px, py, &self.light);
        }
        self.light.SetOpacity(0.9 * alpha);
        let _ = self.fill_skip(card.previous.0, card.previous.1, -1.0, &self.light);
        let _ = self.fill_skip(card.next.0, card.next.1, 1.0, &self.light);

        // The lyrics button lights up in the cover's colour.
        let (lx, ly) = card.lyrics;
        if music.lyrics_ready && music.lyrics_on {
            self.accent.SetColor(&accent);
            self.accent.SetOpacity(alpha);
            let _ = self.fill_lyrics(lx, ly, &self.accent);
        } else {
            let opacity = if music.lyrics_ready { 0.6 } else { 0.25 };
            self.light.SetOpacity(opacity * alpha);
            let _ = self.fill_lyrics(lx, ly, &self.light);
        }

        // Volume: the slider slides out of the speaker on hover.
        let (sx, sy) = card.speaker;
        self.light.SetOpacity(0.85 * alpha);
        let _ = self.fill_speaker(sx, sy, &self.light);
        let reveal = music.volume_visible.clamp(0.0, 1.0);
        if reveal > 0.01 {
            let (left, right, y) = card.volume;
            let start = right - (right - left) * reveal;
            slider(start, right, y, music.volume, reveal * alpha);
        }

        // Lyrics: the line being sung, the next one dimmer below it.
        if let Some((line, next)) = music.lyric {
            let cx = CANVAS.0 / 2.0;
            let (x0, x1) = (cx - width / 2.0 + 18.0, cx + width / 2.0 - 18.0);
            let top = card.play.1 + 16.0;
            let line = if line.is_empty() { "\u{266A}" } else { line };
            self.draw_text(line, &self.lyric_text, rect(x0, top, x1, top + 16.0), alpha);
            self.draw_text(
                next,
                &self.lyric_next,
                rect(x0, top + 16.0, x1, top + 31.0),
                0.4 * alpha,
            );
        }
    }

    /// Album art clipped to a rounded square; a note icon when there is none.
    unsafe fn draw_art(&self, bounds: D2D_RECT_F, radius: f32, alpha: f32) {
        let rounded = D2D1_ROUNDED_RECT {
            rect: bounds,
            radiusX: radius,
            radiusY: radius,
        };
        let size = bounds.right - bounds.left;
        let brush = self.art.as_ref().and_then(|art| {
            let brush = self.target.CreateBitmapBrush(art, None, None).ok()?;
            let source = art.GetSize();
            brush.SetTransform(&Matrix3x2 {
                M11: size / source.width,
                M12: 0.0,
                M21: 0.0,
                M22: size / source.height,
                M31: bounds.left,
                M32: bounds.top,
            });
            brush.SetOpacity(alpha);
            Some(brush)
        });
        match brush {
            Some(brush) => self.target.FillRoundedRectangle(&rounded, &brush),
            None => {
                self.light.SetOpacity(0.12 * alpha);
                self.target.FillRoundedRectangle(&rounded, &self.light);
                self.light.SetOpacity(0.7 * alpha);
                let (cx, cy) = (
                    (bounds.left + bounds.right) / 2.0,
                    (bounds.top + bounds.bottom) / 2.0,
                );
                self.draw_glyph(glyph::MUSIC, cx, cy, &self.light);
            }
        }
    }

    /// One icon glyph centered on (`cx`, `cy`).
    /// A rounded button with an icon and a label, filled with `accent` or a
    /// faint white.
    unsafe fn draw_pill(
        &self,
        bounds: D2D_RECT_F,
        icon: &str,
        label: &str,
        accent: Option<Accent>,
        alpha: f32,
    ) {
        let radius = (bounds.bottom - bounds.top) / 2.0;
        let shape = D2D1_ROUNDED_RECT {
            rect: bounds,
            radiusX: radius,
            radiusY: radius,
        };
        match accent {
            Some(accent) => {
                self.accent.SetColor(&accent_color(accent));
                self.accent.SetOpacity(0.9 * alpha);
                self.target.FillRoundedRectangle(&shape, &self.accent);
            }
            None => {
                self.light.SetOpacity(0.12 * alpha);
                self.target.FillRoundedRectangle(&shape, &self.light);
            }
        }
        let Some(text) = self.text_layout(label, &self.text_left, 200.0) else {
            return;
        };
        let (icon_width, gap) = (14.0, 6.0);
        let total = icon_width + gap + text.width;
        let left = (bounds.left + bounds.right) / 2.0 - total / 2.0;
        let cy = (bounds.top + bounds.bottom) / 2.0;
        self.light.SetOpacity(alpha);
        self.draw_small_glyph_at(icon, left + icon_width / 2.0, cy, &self.light);
        self.draw_layout(
            &text.layout,
            left + icon_width + gap,
            cy - text.height / 2.0,
            alpha,
        );
    }

    /// An icon a little smaller than the regular ones, as in pills.
    unsafe fn draw_small_glyph_at(
        &self,
        icon: &str,
        cx: f32,
        cy: f32,
        brush: &ID2D1SolidColorBrush,
    ) {
        let wide: Vec<u16> = icon.encode_utf16().collect();
        self.target.DrawText(
            &wide,
            &self.pill_glyph,
            &rect(cx - 10.0, cy - 10.0, cx + 10.0, cy + 10.0),
            brush,
            D2D1_DRAW_TEXT_OPTIONS_NONE,
            DWRITE_MEASURING_MODE_NATURAL,
        );
    }

    /// A solid shape with softly rounded corners.
    unsafe fn fill_shape(&self, points: &[(f32, f32)], brush: &ID2D1SolidColorBrush) -> Result<()> {
        let geometry = self.factory.CreatePathGeometry()?;
        let sink = geometry.Open()?;
        let (x, y) = points[0];
        sink.BeginFigure(pt(x, y), D2D1_FIGURE_BEGIN_FILLED);
        for &(x, y) in &points[1..] {
            sink.AddLine(pt(x, y));
        }
        sink.EndFigure(D2D1_FIGURE_END_CLOSED);
        sink.Close()?;
        self.target.FillGeometry(&geometry, brush, None);
        self.target
            .DrawGeometry(&geometry, brush, 1.6, &self.round_caps);
        Ok(())
    }

    unsafe fn fill_play(&self, cx: f32, cy: f32, brush: &ID2D1SolidColorBrush) -> Result<()> {
        self.fill_shape(
            &[(cx - 7.0, cy - 9.0), (cx + 9.0, cy), (cx - 7.0, cy + 9.0)],
            brush,
        )
    }

    unsafe fn fill_pause(&self, cx: f32, cy: f32, brush: &ID2D1SolidColorBrush) {
        for left in [cx - 7.5, cx + 2.0] {
            self.target.FillRoundedRectangle(
                &D2D1_ROUNDED_RECT {
                    rect: rect(left, cy - 9.5, left + 5.5, cy + 9.5),
                    radiusX: 1.8,
                    radiusY: 1.8,
                },
                brush,
            );
        }
    }

    /// Two triangles pointing toward `direction` (-1 back, 1 on).
    unsafe fn fill_skip(
        &self,
        cx: f32,
        cy: f32,
        direction: f32,
        brush: &ID2D1SolidColorBrush,
    ) -> Result<()> {
        for base in [cx - 10.0 * direction, cx] {
            self.fill_shape(
                &[
                    (base, cy - 6.5),
                    (base + 10.0 * direction, cy),
                    (base, cy + 6.5),
                ],
                brush,
            )?;
        }
        Ok(())
    }

    /// A speaker with one sound wave.
    unsafe fn fill_speaker(&self, cx: f32, cy: f32, brush: &ID2D1SolidColorBrush) -> Result<()> {
        self.fill_shape(
            &[
                (cx - 8.0, cy - 2.5),
                (cx - 5.0, cy - 2.5),
                (cx - 1.0, cy - 6.0),
                (cx - 1.0, cy + 6.0),
                (cx - 5.0, cy + 2.5),
                (cx - 8.0, cy + 2.5),
            ],
            brush,
        )?;
        let geometry = self.factory.CreatePathGeometry()?;
        let sink = geometry.Open()?;
        sink.BeginFigure(pt(cx + 3.0, cy - 4.5), D2D1_FIGURE_BEGIN_HOLLOW);
        sink.AddArc(&D2D1_ARC_SEGMENT {
            point: pt(cx + 3.0, cy + 4.5),
            size: D2D_SIZE_F {
                width: 5.5,
                height: 5.5,
            },
            rotationAngle: 0.0,
            sweepDirection: D2D1_SWEEP_DIRECTION_CLOCKWISE,
            arcSize: D2D1_ARC_SIZE_SMALL,
        });
        sink.EndFigure(D2D1_FIGURE_END_OPEN);
        sink.Close()?;
        self.target
            .DrawGeometry(&geometry, brush, 1.8, &self.round_caps);
        Ok(())
    }

    /// A speech bubble: a line being sung.
    unsafe fn fill_lyrics(&self, cx: f32, cy: f32, brush: &ID2D1SolidColorBrush) -> Result<()> {
        self.target.FillRoundedRectangle(
            &D2D1_ROUNDED_RECT {
                rect: rect(cx - 9.0, cy - 8.0, cx + 9.0, cy + 4.0),
                radiusX: 3.5,
                radiusY: 3.5,
            },
            brush,
        );
        self.fill_shape(
            &[
                (cx - 4.5, cy + 3.0),
                (cx - 4.5, cy + 8.0),
                (cx + 1.0, cy + 3.0),
            ],
            brush,
        )
    }

    /// An icon in the page navigation pill.
    unsafe fn draw_nav_glyph(&self, icon: &str, cx: f32, cy: f32, brush: &ID2D1SolidColorBrush) {
        let wide: Vec<u16> = icon.encode_utf16().collect();
        self.target.DrawText(
            &wide,
            &self.nav_glyph,
            &rect(cx - 11.0, cy - 11.0, cx + 11.0, cy + 11.0),
            brush,
            D2D1_DRAW_TEXT_OPTIONS_NONE,
            DWRITE_MEASURING_MODE_NATURAL,
        );
    }

    /// A small icon, as on the shelf's remove buttons.
    unsafe fn draw_small_glyph(&self, icon: &str, cx: f32, cy: f32, brush: &ID2D1SolidColorBrush) {
        let wide: Vec<u16> = icon.encode_utf16().collect();
        self.target.DrawText(
            &wide,
            &self.close_glyph,
            &rect(cx - 8.0, cy - 8.0, cx + 8.0, cy + 8.0),
            brush,
            D2D1_DRAW_TEXT_OPTIONS_NONE,
            DWRITE_MEASURING_MODE_NATURAL,
        );
    }

    unsafe fn draw_glyph(&self, icon: &str, cx: f32, cy: f32, brush: &ID2D1SolidColorBrush) {
        let wide: Vec<u16> = icon.encode_utf16().collect();
        self.target.DrawText(
            &wide,
            &self.icons,
            &rect(cx - 16.0, cy - 16.0, cx + 16.0, cy + 16.0),
            brush,
            D2D1_DRAW_TEXT_OPTIONS_NONE,
            DWRITE_MEASURING_MODE_NATURAL,
        );
    }

    /// Width of the notch needed to show `notice` in full (up to the cap).
    pub fn notice_width(&self, notice: &Notice) -> f32 {
        unsafe {
            self.notice_layout(notice)
                .map_or(NOTICE_WIDTH.0, |card| card.width)
        }
    }

    /// Lays out a notice card; the same numbers size the notch and draw it.
    unsafe fn notice_layout(&self, notice: &Notice) -> Option<CardLayout> {
        let title = self.text_layout(&notice.title, &self.title_text, TEXT_MAX)?;
        let subtitle = self.text_layout(&notice.subtitle, &self.subtitle_text, TEXT_MAX)?;
        let text_width = title.width.max(subtitle.width).min(TEXT_MAX);
        let big = match &notice.trailing {
            Trailing::Text(text) => Some(self.text_layout(text, &self.trailing_text, 120.0)?),
            _ => None,
        };
        let trailing_width = match (&notice.trailing, &big) {
            (Trailing::Text(_), Some(big)) => TRAILING_GAP + big.width,
            (Trailing::Icon(_), _) => TRAILING_GAP + TRAILING_ICON,
            _ => 0.0,
        };
        let width = CARD_PAD_LEFT + RING + RING_GAP + text_width + trailing_width + CARD_PAD_RIGHT;
        Some(CardLayout {
            width,
            text_width,
            title,
            subtitle,
            big,
        })
    }

    unsafe fn text_layout(
        &self,
        text: &str,
        format: &IDWriteTextFormat,
        max_width: f32,
    ) -> Option<TextBlock> {
        let wide: Vec<u16> = text.encode_utf16().collect();
        let layout = self
            .dwrite
            .CreateTextLayout(&wide, format, max_width, 100.0)
            .ok()?;
        let mut metrics = DWRITE_TEXT_METRICS::default();
        layout.GetMetrics(&mut metrics).ok()?;
        Some(TextBlock {
            layout,
            width: metrics.width.min(max_width),
            height: metrics.height,
        })
    }

    /// Ring with icon, title over subtitle, then the trailing element; centered
    /// as one group and clipped to the notch while it grows.
    unsafe fn draw_notice(&self, notice: &Notice, x0: f32, x1: f32, h: f32, alpha: f32) {
        let Some(card) = self.notice_layout(notice) else {
            return;
        };
        self.target
            .PushAxisAlignedClip(&rect(x0, 0.0, x1, h), D2D1_ANTIALIAS_MODE_PER_PRIMITIVE);

        let start = (x0 + x1) / 2.0 - card.width / 2.0;
        let cy = h / 2.0;
        let accent = accent_color(notice.accent);

        // Ring: a faint track, then the filled part in the accent colour.
        let ring_cx = start + CARD_PAD_LEFT + RING / 2.0;
        let (ring_color, ring_alpha, icon_alpha) = if notice.dim {
            (color(1.0, 1.0, 1.0, 1.0), 0.35, 0.55)
        } else {
            (accent, 1.0, 1.0)
        };
        let _ = self.draw_ring(
            ring_cx,
            cy,
            (RING - RING_STROKE) / 2.0,
            notice.progress,
            ring_color,
            ring_alpha * alpha,
        );
        let half = RING / 2.0;
        self.draw_text(
            notice.icon,
            &self.icons,
            rect(ring_cx - half, cy - half, ring_cx + half, cy + half),
            icon_alpha * alpha,
        );

        // Title over subtitle, the pair centered vertically.
        let text_x = start + CARD_PAD_LEFT + RING + RING_GAP;
        let stack = card.title.height + 1.0 + card.subtitle.height;
        let top = cy - stack / 2.0;
        self.draw_layout(&card.title.layout, text_x, top, 0.97 * alpha);
        self.draw_layout(
            &card.subtitle.layout,
            text_x,
            top + card.title.height + 1.0,
            0.55 * alpha,
        );

        let trailing_x = text_x + card.text_width + TRAILING_GAP;
        match (&notice.trailing, &card.big) {
            (Trailing::Text(_), Some(big)) => {
                let y = cy - big.height / 2.0;
                self.draw_layout(&big.layout, trailing_x, y, 0.97 * alpha);
            }
            (Trailing::Icon(icon), _) => {
                self.draw_badge(icon, trailing_x + TRAILING_ICON / 2.0, cy, accent, alpha);
            }
            _ => {}
        }

        self.target.PopAxisAlignedClip();
    }

    /// The charge as a ring: green with a bolt inside while charging, red
    /// when low.
    unsafe fn draw_battery(
        &self,
        cx: f32,
        cy: f32,
        radius: f32,
        percent: u32,
        charging: bool,
        alpha: f32,
    ) {
        let fill = if charging {
            accent_color(Accent::Green)
        } else if percent <= 20 {
            accent_color(Accent::Red)
        } else {
            color(1.0, 1.0, 1.0, 1.0)
        };
        let _ = self.draw_ring(cx, cy, radius, percent as f32 / 100.0, fill, alpha);
        if charging {
            let _ = self.fill_bolt(cx, cy, radius * 0.72);
        }
    }

    /// A lightning bolt `size` tall each way, in the accent brush.
    unsafe fn fill_bolt(&self, cx: f32, cy: f32, size: f32) -> Result<()> {
        const BOLT: [(f32, f32); 6] = [
            (0.25, -1.0),
            (-0.6, 0.18),
            (-0.05, 0.18),
            (-0.25, 1.0),
            (0.6, -0.18),
            (0.05, -0.18),
        ];
        let geometry = self.factory.CreatePathGeometry()?;
        let sink = geometry.Open()?;
        let point = |(x, y): (f32, f32)| pt(cx + x * size, cy + y * size);
        sink.BeginFigure(point(BOLT[0]), D2D1_FIGURE_BEGIN_FILLED);
        BOLT[1..]
            .iter()
            .for_each(|&corner| sink.AddLine(point(corner)));
        sink.EndFigure(D2D1_FIGURE_END_CLOSED);
        sink.Close()?;
        self.target.FillGeometry(&geometry, &self.accent, None);
        Ok(())
    }

    /// Progress ring starting at twelve and running clockwise.
    unsafe fn draw_ring(
        &self,
        cx: f32,
        cy: f32,
        radius: f32,
        progress: f32,
        fill: D2D1_COLOR_F,
        alpha: f32,
    ) -> Result<()> {
        let ellipse = D2D1_ELLIPSE {
            point: pt(cx, cy),
            radiusX: radius,
            radiusY: radius,
        };
        // Thinner on small rings.
        let stroke = (radius * 0.17).clamp(2.0, RING_STROKE);
        self.light.SetOpacity(0.14 * alpha);
        self.target.DrawEllipse(&ellipse, &self.light, stroke, None);

        self.accent.SetColor(&fill);
        self.accent.SetOpacity(alpha);
        let progress = progress.clamp(0.0, 1.0);
        if progress >= 0.999 {
            self.target
                .DrawEllipse(&ellipse, &self.accent, stroke, None);
        } else if progress > 0.005 {
            let angle = progress * std::f32::consts::TAU;
            let geometry = self.factory.CreatePathGeometry()?;
            let sink = geometry.Open()?;
            sink.BeginFigure(pt(cx, cy - radius), D2D1_FIGURE_BEGIN_HOLLOW);
            sink.AddArc(&D2D1_ARC_SEGMENT {
                point: pt(cx + radius * angle.sin(), cy - radius * angle.cos()),
                size: D2D_SIZE_F {
                    width: radius,
                    height: radius,
                },
                rotationAngle: 0.0,
                sweepDirection: D2D1_SWEEP_DIRECTION_CLOCKWISE,
                arcSize: if progress > 0.5 {
                    D2D1_ARC_SIZE_LARGE
                } else {
                    D2D1_ARC_SIZE_SMALL
                },
            });
            sink.EndFigure(D2D1_FIGURE_END_OPEN);
            sink.Close()?;
            self.target
                .DrawGeometry(&geometry, &self.accent, stroke, &self.round_caps);
        }
        Ok(())
    }

    /// An accent-coloured icon in a softly glowing circle.
    unsafe fn draw_badge(&self, icon: &str, cx: f32, cy: f32, accent: D2D1_COLOR_F, alpha: f32) {
        let r = TRAILING_ICON / 2.0;
        let circle = |radius: f32| D2D1_ELLIPSE {
            point: pt(cx, cy),
            radiusX: radius,
            radiusY: radius,
        };
        self.accent.SetColor(&accent);
        self.accent.SetOpacity(0.12 * alpha);
        self.target.FillEllipse(&circle(r), &self.accent);
        self.accent.SetOpacity(0.18 * alpha);
        self.target
            .DrawEllipse(&circle(r + 1.5), &self.accent, 4.0, None);
        self.accent.SetOpacity(0.8 * alpha);
        self.target
            .DrawEllipse(&circle(r - 4.0), &self.accent, 1.8, None);

        let wide: Vec<u16> = icon.encode_utf16().collect();
        self.accent.SetOpacity(alpha);
        self.target.DrawText(
            &wide,
            &self.icons,
            &rect(cx - r, cy - r, cx + r, cy + r),
            &self.accent,
            D2D1_DRAW_TEXT_OPTIONS_NONE,
            DWRITE_MEASURING_MODE_NATURAL,
        );
    }

    unsafe fn draw_layout(&self, layout: &IDWriteTextLayout, x: f32, y: f32, alpha: f32) {
        self.light.SetOpacity(alpha);
        self.target
            .DrawTextLayout(pt(x, y), layout, &self.light, D2D1_DRAW_TEXT_OPTIONS_NONE);
    }

    /// Level bar and percentage.
    unsafe fn draw_level(&self, x0: f32, x1: f32, h: f32, level: f32, muted: bool, alpha: f32) {
        let cy = h / 2.0;
        let (left, right) = (x0 + 42.0, x1 - 46.0);
        let bar = |end: f32, opacity: f32| {
            self.light.SetOpacity(opacity * alpha);
            self.target.FillRoundedRectangle(
                &D2D1_ROUNDED_RECT {
                    rect: rect(left, cy - 2.5, end, cy + 2.5),
                    radiusX: 2.5,
                    radiusY: 2.5,
                },
                &self.light,
            );
        };
        bar(right, 0.22);
        if level > 0.0 {
            // Never thinner than its own rounded ends.
            bar(
                (left + (right - left) * level).max(left + 5.0),
                if muted { 0.4 } else { 1.0 },
            );
        }

        let percent = format!("{}", (level * 100.0).round() as u32);
        self.draw_text(
            &percent,
            &self.small_text,
            rect(x1 - 44.0, 0.0, x1 - 10.0, h),
            0.92 * alpha,
        );
    }

    unsafe fn draw_clock(&self, scene: &Scene) {
        let peek = scene.peek_visible.clamp(0.0, 1.0);
        if scene.minimal && peek < 0.01 {
            return;
        }
        // The clock steps aside quickly for the eyes, a banner or a card; the
        // live strip keeps it, with its own details on either side.
        let cover = scene
            .eyes_visible
            .max(scene.banner_visible)
            .max(scene.card_visible);
        let alpha = (1.0 - cover * 1.6).clamp(0.0, 1.0);
        // The thin bar has no clock until it widens.
        let alpha = if scene.minimal { alpha * peek } else { alpha };
        if alpha <= 0.0 {
            return;
        }
        let cx = CANVAS.0 / 2.0;
        // Drawn scaled by the notch size, so measured before it.
        let (width, height) = (
            scene.width / scene.closed_scale,
            scene.height / scene.closed_scale,
        );
        let bounds = rect(cx - width / 2.0, 0.0, cx + width / 2.0, height);
        self.draw_text(scene.clock, &self.text, bounds, 0.92 * alpha);

        // With nothing live beside it, the clock sits between the battery and
        // the temperature.
        let alpha = alpha * (1.0 - scene.music_visible).clamp(0.0, 1.0);
        if alpha <= 0.01 {
            return;
        }
        let cy = height / 2.0;
        if let Some((percent, charging)) = scene.status.battery {
            self.draw_battery(bounds.left + 18.0, cy, 5.5, percent, charging, alpha);
        }
        if let Some(weather) = &scene.status.weather {
            if let Some(text) = self.text_layout(&weather.temperature, &self.small_left, 50.0) {
                let x = bounds.right - 13.0 - text.width;
                self.draw_layout(&text.layout, x, cy - text.height / 2.0, 0.75 * alpha);
                if let Some(sky) = self.text_layout(weather.description, &self.small_left, 90.0) {
                    let x = x - 7.0 - sky.width;
                    let opacity = 0.75 * alpha * peek * peek;
                    self.draw_layout(&sky.layout, x, cy - sky.height / 2.0, opacity);
                }
            }
        }
        // Widened: the day and date on the clock's left.
        if let Some(date) = self.text_layout(scene.status.date, &self.small_left, 120.0) {
            let x = cx - 32.0 - date.width;
            let opacity = 0.85 * alpha * peek * peek;
            self.draw_layout(&date.layout, x, cy - date.height / 2.0, opacity);
        }
    }

    unsafe fn draw_text(
        &self,
        text: &str,
        format: &IDWriteTextFormat,
        bounds: D2D_RECT_F,
        alpha: f32,
    ) {
        let text: Vec<u16> = text.encode_utf16().collect();
        self.light.SetOpacity(alpha);
        self.target.DrawText(
            &text,
            format,
            &bounds,
            &self.light,
            D2D1_DRAW_TEXT_OPTIONS_NONE,
            DWRITE_MEASURING_MODE_NATURAL,
        );
    }
}

fn rect(left: f32, top: f32, right: f32, bottom: f32) -> D2D_RECT_F {
    D2D_RECT_F {
        left,
        top,
        right,
        bottom,
    }
}

impl Drop for Renderer {
    fn drop(&mut self) {
        unsafe {
            SelectObject(self.dc, self.previous);
            let _ = DeleteObject(self.bitmap.into());
            let _ = DeleteDC(self.dc);
        }
    }
}

fn pt(x: f32, y: f32) -> Vector2 {
    Vector2 { X: x, Y: y }
}

fn color(r: f32, g: f32, b: f32, a: f32) -> D2D1_COLOR_F {
    D2D1_COLOR_F { r, g, b, a }
}

/// Quarter-circle corner, drawn clockwise.
fn arc(end: Vector2, radius: f32) -> D2D1_ARC_SEGMENT {
    D2D1_ARC_SEGMENT {
        point: end,
        size: D2D_SIZE_F {
            width: radius,
            height: radius,
        },
        rotationAngle: 0.0,
        sweepDirection: D2D1_SWEEP_DIRECTION_CLOCKWISE,
        arcSize: D2D1_ARC_SIZE_SMALL,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use windows::Win32::System::Com::{CoInitializeEx, COINIT_APARTMENTTHREADED};

    #[test]
    fn decodes_jpeg_album_art() {
        let Ok(bytes) = std::fs::read(r"C:\Windows\Web\Wallpaper\Windows\img0.jpg") else {
            return; // No sample image on this machine.
        };
        unsafe {
            let _ = CoInitializeEx(None, COINIT_APARTMENTTHREADED);
        }
        let mut renderer = Renderer::new(1.25).expect("renderer");
        renderer.set_album_art(Some(&bytes));
        let art = renderer.art.as_ref().expect("art decoded");
        let size = unsafe { art.GetSize() };
        assert_eq!(size.width, size.height, "cropped to a square");
        assert!(size.width <= ART_PIXELS as f32);
    }

    #[test]
    fn sun_path_starts_and_ends_low() {
        assert!(sun_arc(0.0).abs() < 0.001);
        assert!(sun_arc(4.0).abs() < 0.001);
        assert!(sun_arc(1.8) > 0.99);
    }

    #[test]
    fn page_icons_center_on_the_card() {
        let (first, _) = nav_item(0, 3);
        let (last, _) = nav_item(2, 3);
        assert_eq!((first + last) / 2.0, CANVAS.0 / 2.0);
    }

    #[test]
    fn greets_by_the_hour_of_the_clock() {
        assert_eq!(clock_hour("22:38"), 22);
        assert_eq!(clock_hour("07:05"), 7);
        assert_eq!(greeting(clock_hour("07:05")), "Selamat pagi");
        assert_eq!(greeting(clock_hour("13:00")), "Selamat siang");
        assert_eq!(greeting(clock_hour("16:30")), "Selamat sore");
        assert_eq!(greeting(clock_hour("22:38")), "Selamat malam");
        assert_eq!(greeting(clock_hour("02:00")), "Selamat malam");
    }

    #[test]
    fn tint_follows_the_vivid_pixels() {
        // Mostly grey with a little red, as BGRA.
        let mut pixels = [128, 128, 128, 255].repeat(50);
        pixels.extend([20, 20, 230, 255].repeat(10));
        let [r, g, b] = art_tint(&pixels).unwrap();
        assert!(r > g && r > b, "{r} {g} {b}");
        assert!((r - 0.6).abs() < 0.001);
        assert_eq!(art_tint(&[0, 0, 0, 255]), None);
    }

    #[test]
    fn next_up_puts_todays_event_before_tasks() {
        let task = |text: &str, done| Task {
            text: text.to_string(),
            done,
        };
        let event = |today| AgendaItem {
            day: String::new(),
            time: "09:00".to_string(),
            summary: "Rapat",
            today,
        };
        assert_eq!(next_up(&[], &[]), None);
        assert_eq!(next_up(&[event(false)], &[task("A", true)]), None);
        let tasks = [task("A", true), task("B", false), task("C", false)];
        assert_eq!(next_up(&[], &tasks), Some(("B".to_string(), 1)));
        assert_eq!(
            next_up(&[event(true)], &tasks),
            Some(("09:00 Rapat".to_string(), 2))
        );
    }

    #[test]
    fn formats_durations() {
        assert_eq!(format_duration(0.0), "0:00");
        assert_eq!(format_duration(68.4), "1:08");
        assert_eq!(format_duration(3725.0), "62:05");
    }
}
