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
    D2D1_FIGURE_BEGIN_HOLLOW, D2D1_FIGURE_END_CLOSED, D2D1_FIGURE_END_OPEN, D2D1_PIXEL_FORMAT,
    D2D_RECT_F, D2D_SIZE_F,
};
use windows::Win32::Graphics::Direct2D::{
    D2D1CreateFactory, ID2D1Bitmap, ID2D1DCRenderTarget, ID2D1Factory, ID2D1SolidColorBrush,
    ID2D1StrokeStyle, D2D1_ANTIALIAS_MODE_PER_PRIMITIVE, D2D1_ARC_SEGMENT, D2D1_ARC_SIZE_LARGE,
    D2D1_ARC_SIZE_SMALL, D2D1_BITMAP_INTERPOLATION_MODE_LINEAR, D2D1_CAP_STYLE_FLAT,
    D2D1_CAP_STYLE_ROUND, D2D1_DASH_STYLE_DASH, D2D1_DASH_STYLE_SOLID, D2D1_DRAW_TEXT_OPTIONS_NONE,
    D2D1_ELLIPSE, D2D1_FACTORY_TYPE_SINGLE_THREADED, D2D1_FEATURE_LEVEL_DEFAULT,
    D2D1_LINE_JOIN_ROUND, D2D1_QUADRATIC_BEZIER_SEGMENT, D2D1_RENDER_TARGET_PROPERTIES,
    D2D1_RENDER_TARGET_TYPE_SOFTWARE, D2D1_RENDER_TARGET_USAGE_NONE, D2D1_ROUNDED_RECT,
    D2D1_STROKE_STYLE_PROPERTIES, D2D1_SWEEP_DIRECTION_CLOCKWISE,
};
use windows::Win32::Graphics::DirectWrite::{
    DWriteCreateFactory, IDWriteFactory, IDWriteTextFormat, IDWriteTextLayout,
    DWRITE_FACTORY_TYPE_SHARED, DWRITE_FONT_STRETCH_NORMAL, DWRITE_FONT_STYLE_NORMAL,
    DWRITE_FONT_WEIGHT, DWRITE_FONT_WEIGHT_NORMAL, DWRITE_FONT_WEIGHT_SEMI_BOLD,
    DWRITE_FONT_WEIGHT_SEMI_LIGHT, DWRITE_MEASURING_MODE_NATURAL,
    DWRITE_PARAGRAPH_ALIGNMENT_CENTER, DWRITE_PARAGRAPH_ALIGNMENT_NEAR,
    DWRITE_TEXT_ALIGNMENT_CENTER, DWRITE_TEXT_ALIGNMENT_LEADING, DWRITE_TEXT_METRICS,
    DWRITE_TRIMMING, DWRITE_TRIMMING_GRANULARITY_CHARACTER, DWRITE_WORD_WRAPPING_NO_WRAP,
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
    Accent, Content, LevelKind, Notice, Trailing, CALENDAR_CARD, DROP_ZONE, IDLE, LEVEL,
    MUSIC_CARD, NOTICE_HEIGHT, NOTICE_WIDTH, SHELF_CARD_MAX, STATUS_CARD,
};
use crate::media::Command;
use crate::{calendar, shelf};

/// Width of the concave curves joining the notch to the top screen edge.
const EAR: f32 = 10.0;
/// The canvas fits the largest notch shape; everything outside it is transparent.
pub const CANVAS: (f32, f32) = (
    max_of(&[
        IDLE.0,
        LEVEL.0,
        NOTICE_WIDTH.1,
        MUSIC_CARD.0,
        STATUS_CARD.0,
        CALENDAR_CARD.0,
        DROP_ZONE.0,
        SHELF_CARD_MAX.0,
    ]) + 2.0 * EAR,
    max_of(&[
        IDLE.1,
        LEVEL.1,
        NOTICE_HEIGHT,
        MUSIC_CARD.1,
        STATUS_CARD.1,
        CALENDAR_CARD.1,
        DROP_ZONE.1,
        SHELF_CARD_MAX.1,
    ]),
);

/// Album art is decoded at most this many pixels square.
const ART_PIXELS: u32 = 160;
/// Click radius around each music control.
const HIT_RADIUS: f32 = 20.0;

/// Positions on the music card, measured from the top of the notch.
struct CardRects {
    art: D2D_RECT_F,
    text_left: f32,
    text_right: f32,
    progress_right: f32,
    previous: (f32, f32),
    play: (f32, f32),
    next: (f32, f32),
}

impl CardRects {
    fn new(width: f32) -> Self {
        let cx = CANVAS.0 / 2.0;
        let (x0, x1) = (cx - width / 2.0, cx + width / 2.0);
        let controls_y = 37.0;
        Self {
            art: rect(x0 + 18.0, 20.0, x0 + 90.0, 92.0),
            text_left: x0 + 104.0,
            text_right: x1 - 150.0,
            progress_right: x1 - 20.0,
            previous: (x1 - 122.0, controls_y),
            play: (x1 - 78.0, controls_y),
            next: (x1 - 34.0, controls_y),
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
const TILE: (f32, f32) = (64.0, 76.0);
const TILE_GAP: f32 = 11.0;

struct ShelfRects {
    left: f32,
    clear: D2D_RECT_F,
}

impl ShelfRects {
    fn new(x0: f32, x1: f32) -> Self {
        Self {
            left: x0 + 18.0,
            clear: rect(x1 - 112.0, 14.0, x1 - 18.0, 36.0),
        }
    }

    fn tile(&self, index: usize) -> D2D_RECT_F {
        let (column, row) = ((index % 5) as f32, (index / 5) as f32);
        let left = self.left + column * (TILE.0 + TILE_GAP);
        let top = 46.0 + row * (TILE.1 + 6.0);
        rect(left, top, left + TILE.0, top + TILE.1)
    }

    /// Center of the remove button on a tile.
    fn remove(&self, index: usize) -> (f32, f32) {
        let tile = self.tile(index);
        (tile.right - 6.0, tile.top + 6.0)
    }
}

// Calendar card layout.
const CELL: (f32, f32) = (28.0, 20.0);
const GRID_TOP: f32 = 60.0;

struct CalendarRects {
    grid_left: f32,
    previous_month: (f32, f32),
    next_month: (f32, f32),
    timer_left: f32,
    start_pause: (f32, f32),
    reset: (f32, f32),
}

impl CalendarRects {
    fn new(x0: f32, _x1: f32) -> Self {
        let grid_left = x0 + 20.0;
        let grid_right = grid_left + 7.0 * CELL.0;
        let timer_left = x0 + 250.0;
        Self {
            grid_left,
            previous_month: (grid_right - 42.0, 24.0),
            next_month: (grid_right - 12.0, 24.0),
            timer_left,
            start_pause: (timer_left + 18.0, 136.0),
            reset: (timer_left + 66.0, 136.0),
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
    pub const PREVIOUS: &str = "\u{E892}";
    pub const NEXT: &str = "\u{E893}";
    pub const CHEVRON_LEFT: &str = "\u{E76B}";
    pub const CHEVRON_RIGHT: &str = "\u{E76C}";
    pub const RESET: &str = "\u{E72C}";
    pub const TIMER: &str = "\u{E916}";
    pub const DOWNLOAD: &str = "\u{E896}";
    pub const CLOSE: &str = "\u{E711}";
    pub const FILE: &str = "\u{E8A5}";

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
    /// Which page the open card shows, and how many it has.
    pub page: Page,
    pub page_index: usize,
    pub pages: usize,
    pub calendar: CalendarScene,
    /// The Pomodoro timer; `timer_strip` puts it in the live strip.
    pub timer: Option<TimerScene>,
    pub timer_strip: bool,
    pub shelf: ShelfScene<'a>,
    /// Files are being dragged over the notch.
    pub drop_zone: bool,
    pub status: StatusScene<'a>,
}

#[derive(Clone, Copy, PartialEq)]
pub enum Page {
    Music,
    Status,
    Calendar,
    Shelf,
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

pub struct TimerScene {
    pub label: &'static str,
    /// "24:12"
    pub left: String,
    pub running: bool,
    pub on_break: bool,
    pub session: u32,
}

/// The month shown on the calendar page.
pub struct CalendarScene {
    pub year: i32,
    pub month: u32,
    /// Today's day number when the shown month is the current one.
    pub today: Option<u32>,
}

#[derive(Clone, Copy, PartialEq, Debug)]
pub enum CalendarControl {
    PreviousMonth,
    NextMonth,
    StartPause,
    Reset,
}

pub struct StatusScene<'a> {
    pub time: &'a str,
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
    /// Seconds.
    pub position: f64,
    pub duration: f64,
    /// Level bar heights, 0..1.
    pub bars: [f32; 4],
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
    /// Large clock on the status card.
    big_clock: IDWriteTextFormat,
    /// Small centered text: calendar days.
    tiny: IDWriteTextFormat,
    /// Centered single-line file names that end in an ellipsis.
    tile_text: IDWriteTextFormat,
    /// Small icon glyph for the remove buttons.
    close_glyph: IDWriteTextFormat,
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
            let big_clock = line(DWRITE_FONT_WEIGHT_SEMI_BOLD, 30.0)?;
            let tiny = format(font, DWRITE_FONT_WEIGHT_NORMAL, 12.0)?;
            let tile_text = line(DWRITE_FONT_WEIGHT_NORMAL, 11.0)?;
            tile_text.SetTextAlignment(DWRITE_TEXT_ALIGNMENT_CENTER)?;
            tile_text.SetParagraphAlignment(DWRITE_PARAGRAPH_ALIGNMENT_CENTER)?;
            let close_glyph = format(w!("Segoe Fluent Icons"), DWRITE_FONT_WEIGHT_NORMAL, 8.0)?;
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
                big_clock,
                tiny,
                tile_text,
                close_glyph,
                symbols,
                icons,
                black,
                light,
                accent,
                round_caps,
                art: None,
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
            self.draw_clock(scene);
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
        self.art = bytes.and_then(|bytes| unsafe { self.decode_art(bytes).ok() });
    }

    /// Center-crops to a square and scales to a small bitmap: the art is never
    /// drawn larger than the card shows it.
    unsafe fn decode_art(&self, bytes: &[u8]) -> Result<ID2D1Bitmap> {
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
        self.target.CreateBitmapFromWicBitmap(&converter, None)
    }

    /// Which music control sits at (`x`, `y`) on the open card.
    pub fn music_control_at(&self, x: f32, y: f32, width: f32) -> Option<Command> {
        let card = CardRects::new(width);
        [
            (card.previous, Command::Previous),
            (card.play, Command::TogglePlay),
            (card.next, Command::Next),
        ]
        .into_iter()
        .find(|((cx, cy), _)| (x - cx).powi(2) + (y - cy).powi(2) <= HIT_RADIUS.powi(2))
        .map(|(_, command)| command)
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
            match (&scene.timer, &scene.music) {
                (Some(timer), _) if scene.timer_strip => {
                    self.draw_timer_strip(timer, x0, x1, scene.height, alpha)
                }
                (_, Some(music)) => self.draw_music_strip(music, x0, x1, scene.height, alpha),
                _ => {}
            }
            self.target.PopAxisAlignedClip();
        }
        if scene.card_visible <= 0.01 {
            return;
        }
        let alpha = scene.card_visible.min(1.0);
        self.target
            .PushAxisAlignedClip(&notch, D2D1_ANTIALIAS_MODE_PER_PRIMITIVE);
        match (scene.page, &scene.music, &scene.timer) {
            _ if scene.drop_zone => self.draw_drop_zone(x0, x1, scene.height, alpha),
            (Page::Shelf, _, _) => self.draw_shelf_card(&scene.shelf, x0, x1, alpha),
            (Page::Music, Some(music), _) => self.draw_music_card(music, scene.width, alpha),
            (Page::Calendar, _, Some(timer)) => {
                self.draw_calendar_card(&scene.calendar, timer, x0, x1, alpha)
            }
            _ => self.draw_status_card(&scene.status, x0, x1, alpha),
        }
        if scene.pages > 1 && !scene.drop_zone {
            // Page dots: which of the card's pages is showing, at its foot.
            let y = scene.height - 8.0;
            for dot in 0..scene.pages {
                let x = cx + (dot as f32 - (scene.pages - 1) as f32 / 2.0) * 10.0;
                self.light
                    .SetOpacity(if dot == scene.page_index { 0.9 } else { 0.3 } * alpha);
                self.target.FillEllipse(
                    &D2D1_ELLIPSE {
                        point: pt(x, y),
                        radiusX: 2.5,
                        radiusY: 2.5,
                    },
                    &self.light,
                );
            }
        }
        self.target.PopAxisAlignedClip();
    }

    /// A phase-coloured dot and the time left.
    unsafe fn draw_timer_strip(&self, timer: &TimerScene, x0: f32, x1: f32, h: f32, alpha: f32) {
        let cy = h / 2.0;
        let color = accent_color(if timer.on_break {
            Accent::Teal
        } else {
            Accent::Green
        });
        self.accent.SetColor(&color);
        self.accent
            .SetOpacity(if timer.running { 1.0 } else { 0.45 } * alpha);
        self.target.FillEllipse(
            &D2D1_ELLIPSE {
                point: pt(x0 + 20.0, cy),
                radiusX: 4.0,
                radiusY: 4.0,
            },
            &self.accent,
        );
        let text = format!("{} {}", timer.label, timer.left);
        self.draw_text(
            &text,
            &self.text,
            rect(x0 + 28.0, 0.0, x1 - 12.0, h),
            0.95 * alpha,
        );
    }

    /// Month grid on the left, Pomodoro on the right.
    unsafe fn draw_calendar_card(
        &self,
        calendar: &CalendarScene,
        timer: &TimerScene,
        x0: f32,
        x1: f32,
        alpha: f32,
    ) {
        let layout = CalendarRects::new(x0, x1);
        let heading = format!(
            "{} {}",
            calendar::MONTHS[calendar.month as usize - 1],
            calendar.year
        );
        if let Some(text) = self.text_layout(&heading, &self.title_text, 150.0) {
            self.draw_layout(&text.layout, layout.grid_left, 14.0, alpha);
        }
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
            self.draw_text(name, &self.tiny, cell(column as u32, 40.0), 0.45 * alpha);
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
                        radiusX: 10.0,
                        radiusY: 10.0,
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
        }

        // Pomodoro.
        let left = layout.timer_left;
        if let Some(text) = self.text_layout("Pomodoro", &self.subtitle_text, 140.0) {
            self.draw_layout(&text.layout, left, 18.0, 0.55 * alpha);
        }
        if let Some(text) = self.text_layout(&timer.left, &self.big_clock, 140.0) {
            self.draw_layout(&text.layout, left, 38.0, alpha);
        }
        let caption = format!("{} · sesi {}", timer.label, timer.session);
        if let Some(text) = self.text_layout(&caption, &self.subtitle_text, 140.0) {
            self.draw_layout(&text.layout, left, 80.0, 0.6 * alpha);
        }
        let (px, py) = layout.start_pause;
        self.light.SetOpacity(alpha);
        self.target.FillEllipse(
            &D2D1_ELLIPSE {
                point: pt(px, py),
                radiusX: 17.0,
                radiusY: 17.0,
            },
            &self.light,
        );
        self.black.SetOpacity(alpha);
        let icon = if timer.running {
            glyph::PAUSE
        } else {
            glyph::PLAY
        };
        self.draw_glyph(icon, px, py, &self.black);
        self.black.SetOpacity(1.0);
        self.light.SetOpacity(0.85 * alpha);
        self.draw_glyph(glyph::RESET, layout.reset.0, layout.reset.1, &self.light);
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
        let heading = format!("Shelf · {} berkas", shelf.items.len());
        if let Some(text) = self.text_layout(&heading, &self.title_text, 220.0) {
            self.draw_layout(&text.layout, x0 + 20.0, 14.0, alpha);
        }
        self.draw_text("Hapus semua", &self.tiny, layout.clear, 0.6 * alpha);

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
                tile.left + 10.0,
                tile.top + 4.0,
                tile.left + 54.0,
                tile.top + 48.0,
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
                    let fit = (44.0 / size.width).min(44.0 / size.height);
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
                tile.top + 52.0,
                tile.right + 2.0,
                tile.top + 70.0,
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

    /// Which calendar or timer control sits at (`x`, `y`) on the open card.
    pub fn calendar_control_at(&self, x: f32, y: f32, width: f32) -> Option<CalendarControl> {
        let cx = CANVAS.0 / 2.0;
        let layout = CalendarRects::new(cx - width / 2.0, cx + width / 2.0);
        [
            (layout.previous_month, CalendarControl::PreviousMonth),
            (layout.next_month, CalendarControl::NextMonth),
            (layout.start_pause, CalendarControl::StartPause),
            (layout.reset, CalendarControl::Reset),
        ]
        .into_iter()
        .find(|((cx, cy), _)| (x - cx).powi(2) + (y - cy).powi(2) <= HIT_RADIUS.powi(2))
        .map(|(_, control)| control)
    }

    /// Clock and date, weather, and battery, left to right.
    unsafe fn draw_status_card(&self, status: &StatusScene, x0: f32, x1: f32, alpha: f32) {
        let left = x0 + 22.0;
        if let Some(time) = self.text_layout(status.time, &self.big_clock, 140.0) {
            self.draw_layout(&time.layout, left, 22.0, alpha);
        }
        if let Some(date) = self.text_layout(status.date, &self.subtitle_text, 140.0) {
            self.draw_layout(&date.layout, left, 64.0, 0.55 * alpha);
        }

        let column = x0 + 142.0;
        match &status.weather {
            Some(weather) => {
                if let Some(sky) = self.text_layout(weather.glyph, &self.symbols, 40.0) {
                    self.draw_layout(&sky.layout, column, 26.0, 0.95 * alpha);
                }
                if let Some(temp) =
                    self.text_layout(&weather.temperature, &self.trailing_text, 80.0)
                {
                    self.draw_layout(&temp.layout, column + 36.0, 22.0, alpha);
                }
                if let Some(text) =
                    self.text_layout(weather.description, &self.subtitle_text, 110.0)
                {
                    self.draw_layout(&text.layout, column, 58.0, 0.75 * alpha);
                }
                if let Some(text) = self.text_layout(weather.place, &self.small_left, 110.0) {
                    self.draw_layout(&text.layout, column, 76.0, 0.5 * alpha);
                }
            }
            None => {
                if let Some(text) = self.text_layout(status.weather_hint, &self.small_left, 110.0) {
                    self.draw_layout(&text.layout, column, 48.0, 0.5 * alpha);
                }
            }
        }

        if let Some((percent, charging)) = status.battery {
            let (cx, cy) = (x1 - 52.0, 46.0);
            let color = if charging {
                accent_color(Accent::Green)
            } else if percent <= 20 {
                accent_color(Accent::Red)
            } else {
                color(1.0, 1.0, 1.0, 1.0)
            };
            let _ = self.draw_ring(cx, cy, 21.0, percent as f32 / 100.0, color, alpha);
            self.draw_text(
                &format!("{percent}"),
                &self.small_text,
                rect(cx - 20.0, cy - 12.0, cx + 20.0, cy + 12.0),
                alpha,
            );
            let label = if charging { "Mengisi" } else { "Baterai" };
            self.draw_text(
                label,
                &self.small_text,
                rect(cx - 40.0, 72.0, cx + 40.0, 90.0),
                0.55 * alpha,
            );
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

    /// Art, title over artist, the three controls, and progress.
    unsafe fn draw_music_card(&self, music: &MusicScene, width: f32, alpha: f32) {
        let card = CardRects::new(width);
        self.draw_art(card.art, 12.0, alpha);

        let column = card.text_right - card.text_left;
        if let Some(title) = self.text_layout(music.title, &self.title_text, column) {
            self.draw_layout(&title.layout, card.text_left, 20.0, 0.97 * alpha);
        }
        if let Some(artist) = self.text_layout(music.artist, &self.subtitle_text, column) {
            self.draw_layout(&artist.layout, card.text_left, 41.0, 0.55 * alpha);
        }

        // Play/pause in a white disc, previous and next beside it.
        let (px, py) = card.play;
        self.light.SetOpacity(alpha);
        self.target.FillEllipse(
            &D2D1_ELLIPSE {
                point: pt(px, py),
                radiusX: 17.0,
                radiusY: 17.0,
            },
            &self.light,
        );
        let play_glyph = if music.playing {
            glyph::PAUSE
        } else {
            glyph::PLAY
        };
        self.black.SetOpacity(alpha);
        self.draw_glyph(play_glyph, px, py, &self.black);
        self.black.SetOpacity(1.0);
        self.light.SetOpacity(0.9 * alpha);
        for (cx, cy, icon) in [
            (card.previous.0, card.previous.1, glyph::PREVIOUS),
            (card.next.0, card.next.1, glyph::NEXT),
        ] {
            self.draw_glyph(icon, cx, cy, &self.light);
        }

        // Progress, when the player reports a length.
        if music.duration > 0.0 {
            let fraction = (music.position / music.duration).clamp(0.0, 1.0) as f32;
            let (left, right, y) = (card.text_left, card.progress_right, 76.0);
            let bar = |end: f32, opacity: f32| {
                self.light.SetOpacity(opacity * alpha);
                self.target.FillRoundedRectangle(
                    &D2D1_ROUNDED_RECT {
                        rect: rect(left, y - 2.0, end, y + 2.0),
                        radiusX: 2.0,
                        radiusY: 2.0,
                    },
                    &self.light,
                );
            };
            bar(right, 0.2);
            bar(left + (right - left) * fraction, 0.9);
            let elapsed = format_duration(music.position);
            let remaining = format!("-{}", format_duration(music.duration - music.position));
            if let Some(text) = self.text_layout(&elapsed, &self.small_left, 80.0) {
                self.draw_layout(&text.layout, left, 84.0, 0.55 * alpha);
            }
            if let Some(text) = self.text_layout(&remaining, &self.small_left, 80.0) {
                self.draw_layout(&text.layout, right - text.width, 84.0, 0.55 * alpha);
            }
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
        self.light.SetOpacity(0.14 * alpha);
        self.target
            .DrawEllipse(&ellipse, &self.light, RING_STROKE, None);

        self.accent.SetColor(&fill);
        self.accent.SetOpacity(alpha);
        let progress = progress.clamp(0.0, 1.0);
        if progress >= 0.999 {
            self.target
                .DrawEllipse(&ellipse, &self.accent, RING_STROKE, None);
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
                .DrawGeometry(&geometry, &self.accent, RING_STROKE, &self.round_caps);
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
        // The clock steps aside quickly so it never overlaps the eyes or level.
        let cover = scene
            .eyes_visible
            .max(scene.banner_visible)
            .max(scene.music_visible)
            .max(scene.card_visible);
        let alpha = (1.0 - cover * 1.6).clamp(0.0, 1.0);
        if alpha <= 0.0 {
            return;
        }
        let cx = CANVAS.0 / 2.0;
        let bounds = rect(
            cx - scene.width / 2.0,
            0.0,
            cx + scene.width / 2.0,
            scene.height,
        );
        self.draw_text(scene.clock, &self.text, bounds, 0.92 * alpha);
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
    fn formats_durations() {
        assert_eq!(format_duration(0.0), "0:00");
        assert_eq!(format_duration(68.4), "1:08");
        assert_eq!(format_duration(3725.0), "62:05");
    }
}
