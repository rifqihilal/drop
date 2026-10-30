#![windows_subsystem = "windows"]

//! Drop: a lightweight native notch for Windows.
//!
//! Prototype scope: a compact notch with a clock while the computer is in use,
//! growing into a pair of animated eyes once it has been idle for a while,
//! showing volume and brightness changes in place of Windows' own flyout, and
//! announcing Wi-Fi and Bluetooth changes and low battery.
//! Right-click the notch or the tray icon for the menu.

mod anim;
mod autostart;
mod bluetooth;
mod calendar;
mod config;
mod dragdrop;
mod frames;
mod geo;
mod keys;
mod media;
mod net;
mod osd;
mod prayer;
mod render;
mod settings;
mod shelf;
mod system;
mod tray;
mod weather;
mod wifi;

use std::cell::RefCell;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, Instant, SystemTime};

use windows::core::{w, Result, HSTRING};
use windows::Win32::Foundation::{
    GetLastError, ERROR_ALREADY_EXISTS, HWND, LPARAM, LRESULT, POINT, WPARAM,
};
use windows::Win32::Graphics::Gdi::ScreenToClient;
use windows::Win32::Media::Audio::{PlaySoundW, SND_ALIAS, SND_ASYNC, SND_NODEFAULT};
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::System::Ole::OleInitialize;
use windows::Win32::System::SystemInformation::{GetLocalTime, GetTickCount, GetTickCount64};
use windows::Win32::System::Threading::CreateMutexW;
use windows::Win32::UI::Controls::WM_MOUSELEAVE;
use windows::Win32::UI::HiDpi::{
    GetDpiForWindow, SetProcessDpiAwarenessContext, DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2,
};
use windows::Win32::UI::Input::KeyboardAndMouse::{
    GetLastInputInfo, ReleaseCapture, SetCapture, TrackMouseEvent, LASTINPUTINFO, TME_LEAVE,
    TRACKMOUSEEVENT,
};
use windows::Win32::UI::Shell::ShellExecuteW;
use windows::Win32::UI::WindowsAndMessaging::{
    AppendMenuW, CreatePopupMenu, CreateWindowExW, DefWindowProcW, DestroyMenu, DestroyWindow,
    DispatchMessageW, GetCursorPos, GetMessageW, GetSystemMetrics, LoadCursorW, PostMessageW,
    PostQuitMessage, RegisterClassExW, SetCursor, SetForegroundWindow, SetTimer, ShowWindow,
    TrackPopupMenu, TranslateMessage, HTCLIENT, IDC_ARROW, IDC_HAND, MF_CHECKED, MF_GRAYED,
    MF_POPUP, MF_SEPARATOR, MF_STRING, MSG, SM_CXSCREEN, SW_SHOWNOACTIVATE, SW_SHOWNORMAL,
    TPM_RETURNCMD, TPM_RIGHTBUTTON, WM_DESTROY, WM_DEVICECHANGE, WM_DISPLAYCHANGE, WM_DPICHANGED,
    WM_LBUTTONDOWN, WM_LBUTTONUP, WM_MOUSEMOVE, WM_MOUSEWHEEL, WM_NULL, WM_POWERBROADCAST,
    WM_RBUTTONUP, WM_SETCURSOR, WM_SETTINGCHANGE, WM_TIMER, WNDCLASSEXW, WS_EX_LAYERED,
    WS_EX_NOACTIVATE, WS_EX_TOOLWINDOW, WS_EX_TOPMOST, WS_POPUP,
};

use anim::{
    shelf_card, Accent, LevelKind, Notch, Notice, Trailing, CALENDAR_CARD, DROP_ZONE, MUSIC_CARD,
    MUSIC_STRIP, PRAYER_CARD, PRAYER_STRIP, STATUS_CARD, TIMER_STRIP,
};
use bluetooth::{Bluetooth, DeviceEvent};
use calendar::{Phase, Pomodoro};
use config::{PrayerSettings, Settings};
use frames::Pacer;
use media::{Command, Media, Track};
use render::{
    glyph, CalendarControl, CalendarScene, MusicScene, Page, PrayerScene, Renderer, Scene,
    ShelfHit, ShelfScene, StatusScene, Strip, TimerScene, WeatherScene,
};
use shelf::Shelf;
use system::{Audio, PowerEvent};
use weather::{Weather, WeatherWatch};
use wifi::Wifi;

const TIMER_ID: usize = 1;
/// Marks a request for animation frames (paced to the display, see `frames`).
const FRAME_MS: u32 = 16;
/// Wake-up interval while the eyes or a level are showing: catches the next
/// blink or glance, and the moment the level should hide.
const BUSY_MS: u32 = 100;
/// Wake-up interval while compact: only the idle check and the clock need it.
const COMPACT_MS: u32 = 500;
const MENU_QUIT: usize = 1;
const MENU_AUTOSTART: usize = 2;
/// A Wi-Fi drop is only announced if it does not come back within this time,
/// so roaming between access points stays quiet.
const WIFI_GRACE: Duration = Duration::from_secs(4);
/// Battery levels that trigger a low-battery notice on the way down.
const LOW_BATTERY: [u32; 2] = [20, 10];
/// Gap between sample notices in `--demo`.
const DEMO_STEP: Duration = Duration::from_secs(4);
/// Wait before reading a newly connected device's battery level.
const BATTERY_DELAY: Duration = Duration::from_millis(2500);
/// Frame interval for the music level bars (~15 fps keeps them light).
const BARS_MS: u32 = 66;
const BAR_SPEEDS: [f32; 4] = [7.0, 9.5, 6.0, 11.0];
/// How long the music card opens for a new song.
const SONG_CARD: Duration = Duration::from_secs(4);
/// The mouse rests this long on the notch before the music card opens.
const HOVER_DELAY: Duration = Duration::from_millis(150);
/// The card stays open this long after the mouse leaves.
const LEAVE_GRACE: Duration = Duration::from_millis(400);
/// How long the shelf card stays open after files are dropped.
const SHELF_SHOWN: Duration = Duration::from_secs(3);
/// Logical pixels a pressed shelf tile must move before it is dragged out.
const DRAG_THRESHOLD: f32 = 6.0;
/// Length of the cross-fade between card pages.
const PAGE_SWITCH: Duration = Duration::from_millis(260);
/// A prayer this many minutes away or closer shows in the closed notch.
const PRAYER_SOON_MINUTES: u32 = 30;
/// Prayer reminders stay up longer than other notices.
const PRAYER_NOTICE_DURATION: Duration = Duration::from_secs(10);
const MENU_SETTINGS: usize = 3;

struct App {
    hwnd: HWND,
    renderer: Renderer,
    notch: Notch,
    idle_after_ms: u32,
    last_tick: Instant,
    timer_ms: u32,
    clock: String,
    /// Kept alive so Core Audio keeps calling back.
    audio: Option<Audio>,
    /// Last known brightness; None until Windows reports the starting value.
    brightness: Option<u32>,
    /// Kept alive so the WLAN service keeps calling back.
    _wifi: Option<Wifi>,
    /// A Wi-Fi disconnect waiting out the grace period: (deadline, network).
    wifi_lost: Option<(Instant, String)>,
    bluetooth: Option<Bluetooth>,
    /// Last known power source and charge; None until Windows reports them.
    on_ac: Option<bool>,
    battery: Option<u32>,
    /// `--demo`: index of the next sample notice and when to show it.
    demo: Option<(usize, Instant)>,
    /// A connected Bluetooth device whose battery level is read at the deadline.
    battery_check: Option<(Instant, DeviceEvent)>,
    settings: Settings,
    /// `--idle-secs` was given, overriding the idle delay from the settings.
    idle_from_args: bool,
    /// Today's prayer times; None until a location is set.
    prayer: Option<PrayerDay>,
    /// Last seen modification time of the settings file.
    config_stamp: Option<SystemTime>,
    /// Media session watcher; None if Windows refused it.
    media: Option<Media>,
    /// What is playing, if anything.
    track: Option<Track>,
    /// A track was received at least once.
    tracks_seen: bool,
    /// When the mouse entered the notch, while it is inside.
    hover_since: Option<Instant>,
    /// The music card stays open until then (new song, mouse just left).
    card_until: Option<Instant>,
    bars: [f32; 4],
    bar_phase: f32,
    pomodoro: Pomodoro,
    /// Months away from the current one on the calendar page.
    month_offset: i32,
    /// A page switch in progress on the open card.
    page_switch: Option<PageSwitch>,
    /// Display-refresh frames while animating.
    pacer: Pacer,
    /// Files parked in the notch.
    shelf: Shelf,
    /// Files are being dragged over the notch.
    drag_over: bool,
    /// Shelf tile under the mouse.
    shelf_hover: Option<usize>,
    /// A shelf tile pressed and the logical point it was pressed at.
    press: Option<(usize, f32, f32)>,
    /// Which page the open card shows when there is music.
    page: Page,
    weather_watch: WeatherWatch,
    weather: Option<Weather>,
}

/// The card moving from one page to the next.
struct PageSwitch {
    from: Page,
    started: Instant,
    /// 1 toward later pages, -1 back.
    direction: f32,
}

/// One day's prayer schedule and which reminders already went off.
struct PrayerDay {
    settings: PrayerSettings,
    date: (u16, u16, u16),
    times: prayer::Schedule,
    reminded: [bool; 5],
}

impl PrayerDay {
    fn new(settings: PrayerSettings, date: (u16, u16, u16)) -> Self {
        let times = prayer::schedule(
            date.0 as i32,
            date.1 as u32,
            date.2 as u32,
            settings.location,
            prayer::utc_offset_hours(),
        );
        Self {
            settings,
            date,
            times,
            reminded: [false; 5],
        }
    }

    /// The next prayer after `now` (minutes after midnight), if any is left today.
    fn next(&self, now: u32) -> Option<usize> {
        (0..prayer::NAMES.len()).find(|&i| self.times[i] > now)
    }
}

thread_local! {
    static APP: RefCell<Option<App>> = const { RefCell::new(None) };
}

fn main() -> Result<()> {
    let idle_secs = parse_idle_secs();
    unsafe {
        // One notch per session: a second launch (say, from auto-start) just exits.
        // The handle stays open for the life of the process.
        let _instance_lock = CreateMutexW(None, false, w!("Local\\DropNotch"))?;
        if GetLastError() == ERROR_ALREADY_EXISTS {
            return Ok(());
        }

        let _ = SetProcessDpiAwarenessContext(DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2);
        // OLE, not just COM: dropping files onto the notch needs it.
        OleInitialize(None)?;

        let instance = GetModuleHandleW(None)?;
        let class = WNDCLASSEXW {
            cbSize: size_of::<WNDCLASSEXW>() as u32,
            lpfnWndProc: Some(wndproc),
            hInstance: instance.into(),
            // Without a class cursor, whatever cursor was showing (say, another
            // app's busy cursor) would stay over the notch.
            hCursor: LoadCursorW(None, IDC_ARROW)?,
            lpszClassName: w!("DropNotch"),
            ..Default::default()
        };
        RegisterClassExW(&class);

        // Topmost, never activated, no taskbar button; per-pixel alpha makes the
        // transparent parts of the canvas click-through.
        let hwnd = CreateWindowExW(
            WS_EX_LAYERED | WS_EX_TOPMOST | WS_EX_TOOLWINDOW | WS_EX_NOACTIVATE,
            w!("DropNotch"),
            w!("Drop"),
            WS_POPUP,
            0,
            0,
            1,
            1,
            None,
            None,
            Some(instance.into()),
            None,
        )?;

        let now = Instant::now();
        let mut app = App {
            hwnd,
            renderer: Renderer::new(window_scale(hwnd))?,
            notch: Notch::new(now, GetTickCount64()),
            idle_after_ms: idle_secs.unwrap_or(0) * 1000,
            last_tick: now,
            timer_ms: 0,
            clock: current_clock(),
            audio: Audio::new(hwnd).ok(),
            brightness: None,
            _wifi: Wifi::watch(hwnd),
            wifi_lost: None,
            bluetooth: Bluetooth::watch(hwnd),
            on_ac: None,
            battery: None,
            demo: has_flag("--demo").then(|| (0, now + Duration::from_secs(1))),
            battery_check: None,
            settings: Settings::default(),
            idle_from_args: idle_secs.is_some(),
            prayer: None,
            config_stamp: None,
            media: Some(Media::start(hwnd)),
            track: None,
            tracks_seen: false,
            hover_since: None,
            card_until: None,
            bars: [0.0; 4],
            bar_phase: 0.0,
            pomodoro: Pomodoro::default(),
            month_offset: 0,
            page_switch: None,
            pacer: Pacer::start(hwnd),
            shelf: Shelf::load(),
            drag_over: false,
            shelf_hover: None,
            press: None,
            page: Page::Music,
            weather_watch: WeatherWatch::start(hwnd),
            weather: None,
        };
        config::ensure_exists();
        // Load quietly: the "schedule ready" notice is for later edits only.
        app.config_stamp = config::modified();
        app.apply_settings(false);
        app.renderer.sync_thumbnails(app.shelf.items());
        app.draw();
        let _ = ShowWindow(hwnd, SW_SHOWNOACTIVATE);
        tray::add(hwnd);
        osd::set_suppressed(true);
        keys::install(hwnd);
        dragdrop::accept_drops(hwnd);
        app.schedule(COMPACT_MS);
        APP.with(|cell| *cell.borrow_mut() = Some(app));
        // After the app exists: Windows answers right away with the current values.
        system::watch_power(hwnd);
        if has_flag("--settings") {
            settings::open(hwnd);
        }

        let mut msg = MSG::default();
        while GetMessageW(&mut msg, None, 0, 0).as_bool() {
            if settings::handle_dialog_message(&msg) {
                continue;
            }
            let _ = TranslateMessage(&msg);
            DispatchMessageW(&msg);
        }
    }
    // Release the COM and Direct2D objects now. Left to the thread-local
    // destructor, they are released during process teardown, which stalls
    // the exit for several seconds.
    APP.with(|cell| drop(cell.borrow_mut().take()));
    Ok(())
}

impl App {
    fn tick(&mut self) {
        let now = Instant::now();
        let dt = (now - self.last_tick).as_secs_f32().min(0.05);
        self.last_tick = now;

        self.notch.set_idle(idle_ms() >= self.idle_after_ms, now);
        if self
            .wifi_lost
            .as_ref()
            .is_some_and(|(deadline, _)| now >= *deadline)
        {
            if let Some((_, ssid)) = self.wifi_lost.take() {
                if self.settings.notify_wifi {
                    self.notify(notices::wifi(&ssid, false));
                }
            }
        }
        if self
            .battery_check
            .as_ref()
            .is_some_and(|(deadline, _)| now >= *deadline)
        {
            if let Some((_, event)) = self.battery_check.take() {
                self.check_device_battery(event);
            }
        }
        if let Some((index, at)) = self.demo {
            if now >= at {
                self.demo = Some((index + 1, now + DEMO_STEP));
                self.notify(notices::demo(index));
            }
        }
        let clock = current_clock();
        let clock_changed = clock != self.clock;
        self.clock = clock;
        if clock_changed {
            self.update_prayer();
        }

        if let Some(finished) = self.pomodoro.advance(now) {
            self.on_phase_end(finished);
        }
        self.update_music(now);
        let switching = self
            .page_switch
            .as_ref()
            .is_some_and(|switch| now.duration_since(switch.started) < PAGE_SWITCH);
        if !switching {
            self.page_switch = None;
        }
        let moving = self.notch.step(now, dt) || switching;
        let bars_moving = self.step_bars(dt);
        // The open card shows a running progress bar.
        let card_open = self.notch.card_visible.value > 0.01;
        // A running timer counts down every second, in the strip or the card.
        let timer_running = self.pomodoro.running();

        if moving || clock_changed || bars_moving || card_open || timer_running {
            self.draw();
        }
        let waiting =
            self.wifi_lost.is_some() || self.battery_check.is_some() || self.demo.is_some();
        self.schedule(if moving {
            FRAME_MS
        } else if bars_moving {
            BARS_MS
        } else if self.notch.busy() || waiting || self.hover_since.is_some() {
            BUSY_MS
        } else {
            COMPACT_MS
        });
    }

    fn on_track(&mut self, track: Option<Track>) {
        let now = Instant::now();
        let art = |track: &Option<Track>| track.as_ref().and_then(|t| t.art.clone());
        let art_changed = match (art(&self.track), art(&track)) {
            (Some(old), Some(new)) => !Arc::ptr_eq(&old, &new),
            (old, new) => old.is_some() != new.is_some(),
        };
        if art_changed {
            let bytes = art(&track);
            self.renderer
                .set_album_art(bytes.as_deref().map(Vec::as_slice));
        }
        let new_song = match (&self.track, &track) {
            (Some(old), Some(new)) => !old.same_song(new),
            (None, Some(_)) => true,
            _ => false,
        };
        // A new song opens the card for a moment; the song already playing
        // when Drop starts does not.
        if new_song && self.tracks_seen && track.as_ref().is_some_and(|t| t.playing) {
            self.card_until = Some(now + SONG_CARD);
            self.page = Page::Music;
        }
        self.tracks_seen = true;
        self.track = track;
        self.tick();
    }

    /// Decides between the playing strip and the open card.
    fn update_music(&mut self, now: Instant) {
        let hovering = self
            .hover_since
            .is_some_and(|since| now.duration_since(since) >= HOVER_DELAY);
        self.card_until = self.card_until.filter(|until| now < *until);
        // Files dragged over the notch always open the drop zone.
        let open = hovering || self.card_until.is_some() || self.drag_over;
        if !open {
            // The next opening starts on the first page, this month.
            self.page = Page::Music;
            self.month_offset = 0;
            self.page_switch = None;
        }
        let card = open.then(|| match self.shown_page() {
            _ if self.drag_over => DROP_ZONE,
            Page::Shelf => shelf_card(self.shelf.items().len()),
            Page::Music => MUSIC_CARD,
            Page::Status => STATUS_CARD,
            Page::Calendar => CALENDAR_CARD,
            Page::Prayer => PRAYER_CARD,
        });
        let strip = match self.strip() {
            Strip::Timer => Some(TIMER_STRIP),
            Strip::Music => Some(MUSIC_STRIP),
            Strip::Prayer => Some(PRAYER_STRIP),
            Strip::None => None,
        };
        self.notch.set_live(strip, card);
    }

    /// What the closed notch shows beside the clock: a running or paused
    /// timer first, then music that is playing, then a prayer close at hand.
    fn strip(&self) -> Strip {
        if !self.pomodoro.idle() {
            Strip::Timer
        } else if self.track.as_ref().is_some_and(|track| track.playing) {
            Strip::Music
        } else if self.prayer_soon().is_some() {
            Strip::Prayer
        } else {
            Strip::None
        }
    }

    /// "Maghrib 12 mnt" while the next prayer is less than half an hour away.
    fn prayer_soon(&self) -> Option<String> {
        let day = self.prayer.as_ref()?;
        let now = minutes_now();
        let next = day.next(now)?;
        let wait = day.times[next] - now;
        (wait <= PRAYER_SOON_MINUTES)
            .then(|| format!("{} {}", prayer::NAMES[next], prayer::format_wait(wait)))
    }

    /// The card's pages, in wheel order; music only while there is some.
    /// The card's pages, in wheel order; music and shelf only when they have
    /// something to show.
    fn pages(&self) -> Vec<Page> {
        let mut pages = Vec::with_capacity(4);
        if self.track.is_some() {
            pages.push(Page::Music);
        }
        pages.push(Page::Status);
        if self.prayer.is_some() {
            pages.push(Page::Prayer);
        }
        pages.push(Page::Calendar);
        if !self.shelf.is_empty() {
            pages.push(Page::Shelf);
        }
        pages
    }

    fn on_drag(&mut self, over: bool) {
        self.drag_over = over;
        self.tick();
    }

    /// Files dropped on the notch go on the shelf, which opens to show them.
    fn on_drop(&mut self, paths: Vec<PathBuf>) {
        let added = self
            .shelf
            .add(paths.into_iter().filter(|path| path.exists()));
        if added == 0 {
            return;
        }
        self.renderer.sync_thumbnails(self.shelf.items());
        self.page = Page::Shelf;
        self.card_until = Some(Instant::now() + SHELF_SHOWN);
        self.tick();
    }

    /// A press on a shelf tile may become a drag out.
    fn on_press(&mut self, lparam: LPARAM) {
        if self.shown_page() != Page::Shelf || self.notch.card_visible.value < 0.5 {
            return;
        }
        let (x, y) = self.logical_point(lparam);
        if let Some(ShelfHit::Tile(index)) = self.shelf_hit(x, y) {
            self.press = Some((index, x, y));
            unsafe {
                SetCapture(self.hwnd);
            }
        }
    }

    /// Mouse movement over the card: tile highlight, and the start of a drag
    /// out once a pressed tile moves far enough. Returns the file to drag;
    /// the caller runs the drag after letting go of the app state.
    fn on_move(&mut self, lparam: LPARAM) -> Option<PathBuf> {
        let (x, y) = self.logical_point(lparam);
        if let Some((index, px, py)) = self.press {
            if (x - px).abs() + (y - py).abs() > DRAG_THRESHOLD {
                self.press = None;
                unsafe {
                    let _ = ReleaseCapture();
                }
                return self.shelf.items().get(index).cloned();
            }
        }
        let hover = match self.shelf_hit(x, y) {
            Some(ShelfHit::Tile(index) | ShelfHit::Remove(index)) => Some(index),
            _ => None,
        };
        if hover != self.shelf_hover {
            self.shelf_hover = hover;
            self.draw();
        }
        None
    }

    /// Whether (`x`, `y`) is over something clickable on the open card.
    fn clickable_at(&self, x: f32, y: f32) -> bool {
        if self.notch.card_visible.value < 0.5 || self.drag_over {
            return false;
        }
        let width = self.notch.width.value;
        match self.shown_page() {
            Page::Music => self.renderer.music_control_at(x, y, width).is_some(),
            Page::Calendar => self.renderer.calendar_control_at(x, y, width).is_some(),
            Page::Shelf => self.shelf_hit(x, y).is_some(),
            Page::Status | Page::Prayer => false,
        }
    }

    fn shelf_hit(&self, x: f32, y: f32) -> Option<ShelfHit> {
        if self.shown_page() != Page::Shelf || self.notch.card_visible.value < 0.5 {
            return None;
        }
        self.renderer
            .shelf_hit(x, y, self.notch.width.value, self.shelf.items().len())
    }

    /// Click on the shelf page: open, remove one, or clear all.
    fn on_shelf_click(&mut self, x: f32, y: f32) {
        let pressed = self.press.take();
        if pressed.is_some() {
            unsafe {
                let _ = ReleaseCapture();
            }
        }
        match self.shelf_hit(x, y) {
            Some(ShelfHit::Tile(index)) if pressed.is_some_and(|(i, ..)| i == index) => {
                if let Some(path) = self.shelf.items().get(index) {
                    open_file(path);
                }
            }
            Some(ShelfHit::Remove(index)) => self.shelf.remove(index),
            Some(ShelfHit::ClearAll) => self.shelf.clear(),
            _ => return,
        }
        self.shelf_hover = None;
        self.renderer.sync_thumbnails(self.shelf.items());
        self.tick();
    }

    fn logical_point(&self, lparam: LPARAM) -> (f32, f32) {
        let scale = window_scale(self.hwnd);
        (
            (lparam.0 & 0xffff) as i16 as f32 / scale,
            ((lparam.0 >> 16) & 0xffff) as i16 as f32 / scale,
        )
    }

    fn shown_page(&self) -> Page {
        let pages = self.pages();
        if pages.contains(&self.page) {
            self.page
        } else {
            pages[0]
        }
    }

    /// The wheel steps through the open card's pages, down for the next.
    fn on_wheel(&mut self, wparam: WPARAM) {
        if self.notch.card_visible.target == 0.0 {
            return;
        }
        let pages = self.pages();
        let current = pages
            .iter()
            .position(|page| *page == self.shown_page())
            .unwrap_or(0);
        let down = ((wparam.0 >> 16) as i16) < 0;
        let next = if down {
            (current + 1) % pages.len()
        } else {
            (current + pages.len() - 1) % pages.len()
        };
        if pages[next] != pages[current] {
            self.page_switch = Some(PageSwitch {
                from: pages[current],
                started: Instant::now(),
                direction: if down { 1.0 } else { -1.0 },
            });
        }
        self.page = pages[next];
        self.tick();
    }

    /// Announces the end of a Pomodoro phase.
    fn on_phase_end(&mut self, finished: Phase) {
        let notice = match finished {
            Phase::Focus => Notice {
                icon: glyph::TIMER,
                accent: Accent::Green,
                progress: 1.0,
                title: "Waktunya istirahat".to_string(),
                subtitle: format!("Sesi {} selesai · istirahat dimulai", self.pomodoro.session),
                trailing: Trailing::Text(calendar::format_left(self.pomodoro.left(Instant::now()))),
                dim: false,
            },
            Phase::Break => Notice {
                icon: glyph::TIMER,
                accent: Accent::Teal,
                progress: 1.0,
                title: "Istirahat selesai".to_string(),
                subtitle: "Mulai sesi fokus berikutnya".to_string(),
                trailing: Trailing::None,
                dim: false,
            },
        };
        self.notify_for(notice, PRAYER_NOTICE_DURATION);
    }

    fn on_weather(&mut self, weather: Weather) {
        self.weather = Some(weather);
        self.draw();
    }

    /// What the status page shows in place of the weather.
    fn weather_hint(&self) -> &'static str {
        if !self.settings.weather {
            "Cuaca dimatikan"
        } else if self.settings.prayer().is_none() {
            "Atur lokasi di Pengaturan"
        } else {
            "Memuat cuaca…"
        }
    }

    /// Moves the level bars toward the current output peak. Returns true
    /// while they are on screen and moving.
    fn step_bars(&mut self, dt: f32) -> bool {
        let playing = self.track.as_ref().is_some_and(|track| track.playing);
        let shown = self.notch.music_visible.value > 0.01;
        if !(shown && playing) {
            self.bars = [0.0; 4];
            return false;
        }
        self.bar_phase += dt;
        let peak = self.audio.as_ref().map_or(0.0, Audio::peak);
        let level = (peak * 1.8).min(1.0);
        for (i, bar) in self.bars.iter_mut().enumerate() {
            // Each bar sways at its own pace so they do not move in lockstep.
            let sway = 0.55 + 0.45 * (self.bar_phase * BAR_SPEEDS[i] + i as f32).sin();
            *bar += (level * sway - *bar) * 0.45;
        }
        true
    }

    fn on_hover(&mut self, inside: bool) {
        let now = Instant::now();
        if inside {
            if self.hover_since.is_none() {
                self.hover_since = Some(now);
                let mut track = TRACKMOUSEEVENT {
                    cbSize: size_of::<TRACKMOUSEEVENT>() as u32,
                    dwFlags: TME_LEAVE,
                    hwndTrack: self.hwnd,
                    dwHoverTime: 0,
                };
                unsafe {
                    let _ = TrackMouseEvent(&mut track);
                }
                self.schedule(BUSY_MS);
            }
        } else if self.hover_since.take().is_some() && self.notch.card_visible.target > 0.0 {
            // A short grace period, so brushing past the edge does not snap it shut.
            self.card_until = Some(now + LEAVE_GRACE);
        }
    }

    fn on_click(&mut self, lparam: LPARAM) {
        let (x, y) = self.logical_point(lparam);
        if self.shown_page() == Page::Shelf {
            self.on_shelf_click(x, y);
            return;
        }
        if self.notch.card_visible.value < 0.5 {
            return;
        }
        let width = self.notch.width.value;
        if self.shown_page() == Page::Calendar {
            match self.renderer.calendar_control_at(x, y, width) {
                Some(CalendarControl::PreviousMonth) => self.month_offset -= 1,
                Some(CalendarControl::NextMonth) => self.month_offset += 1,
                Some(CalendarControl::StartPause) => self.pomodoro.toggle(Instant::now()),
                Some(CalendarControl::Reset) => self.pomodoro.reset(),
                None => return,
            }
            self.tick();
            return;
        }
        if self.shown_page() != Page::Music {
            return;
        }
        let Some(command) = self.renderer.music_control_at(x, y, width) else {
            return;
        };
        if let Some(media) = &self.media {
            media.send(command);
        }
        // Show the new state right away; the session confirms it shortly.
        if let (Command::TogglePlay, Some(track)) = (command, self.track.as_mut()) {
            track.position = track.position_now();
            track.read_at = Instant::now();
            track.playing = !track.playing;
        }
        self.tick();
    }

    fn show_level(&mut self, kind: LevelKind, level: f32, muted: bool) {
        // Older shells host the flyout in a window that gets recreated; keep
        // it hidden. Newer Windows 11 draws it inside Explorer, where only
        // taking over the keys helps (see `keys`).
        osd::set_suppressed(true);
        self.notch.show_level(kind, level, muted, Instant::now());
        self.tick();
    }

    /// Shows a notice with the notification sound. Every notice uses the same
    /// sound so they are recognizable by ear.
    fn notify(&mut self, notice: Notice) {
        self.notify_for(notice, anim::NOTICE_DURATION);
    }

    fn notify_for(&mut self, notice: Notice, duration: Duration) {
        if self.settings.sound {
            chime();
        }
        let width = self.renderer.notice_width(&notice);
        self.notch
            .show_notice(notice, width, duration, Instant::now());
        // Called from `tick` too, which redraws right after.
        self.schedule(FRAME_MS);
    }

    /// Reads the settings file and puts it into effect. `announce` confirms
    /// a new prayer location with a notice.
    fn apply_settings(&mut self, announce: bool) {
        self.settings = Settings::load();
        let place = self.settings.latitude.zip(self.settings.longitude);
        let weather_place = place.filter(|_| self.settings.weather);
        if weather_place.is_none() {
            self.weather = None;
        }
        self.weather_watch.set_location(weather_place);
        if !self.idle_from_args {
            self.idle_after_ms = self.settings.idle_minutes * 60_000;
        }
        self.prayer = self
            .settings
            .prayer()
            .map(|settings| PrayerDay::new(settings, prayer::today()));
        if announce && self.settings.notify_prayer {
            if let Some(day) = &self.prayer {
                let notice = notices::prayer_ready(day);
                self.notify(notice);
            }
        }
        self.update_prayer();
    }

    /// Picks up edits to the settings file and the change of day, then
    /// raises any prayer reminder that is due. Runs once a minute.
    fn update_prayer(&mut self) {
        let stamp = config::modified();
        if stamp != self.config_stamp {
            self.config_stamp = stamp;
            self.apply_settings(true);
            return;
        }
        let today = prayer::today();
        if let Some(day) = self.prayer.as_mut().filter(|day| day.date != today) {
            *day = PrayerDay::new(day.settings.clone(), today);
        }

        let now = minutes_now();
        let Some(day) = self.prayer.as_mut().filter(|_| self.settings.notify_prayer) else {
            return;
        };
        let lead = day.settings.remind_minutes;
        // A reminder fires once, inside its window; a computer waking from
        // sleep after the prayer time skips it.
        let due = (0..prayer::NAMES.len())
            .find(|&i| !day.reminded[i] && now + lead >= day.times[i] && now < day.times[i]);
        if let Some(i) = due {
            day.reminded[i] = true;
            let notice = notices::prayer(i, day.times[i] - now, day);
            self.notify_for(notice, PRAYER_NOTICE_DURATION);
        }
    }

    /// The prayer page: the next prayer (tomorrow's Subuh after Isya), how
    /// long until it, today's times, and the qibla.
    fn prayer_scene(&self) -> Option<PrayerScene> {
        let day = self.prayer.as_ref()?;
        let now = minutes_now();
        let next_today = day.next(now);
        let (next, time) = match next_today {
            Some(index) => (index, day.times[index]),
            None => {
                let (y, m, d) = prayer::next_day(day.date.0, day.date.1, day.date.2);
                let tomorrow = prayer::schedule(
                    y as i32,
                    m as u32,
                    d as u32,
                    day.settings.location,
                    prayer::utc_offset_hours(),
                );
                (0, tomorrow[0] + 24 * 60)
            }
        };
        let qibla = prayer::qibla(day.settings.location);
        Some(PrayerScene {
            next_name: prayer::NAMES[next],
            next_time: prayer::format_time(time % (24 * 60)),
            wait: prayer::format_wait(time.saturating_sub(now)),
            times: day.times.map(prayer::format_time),
            next_today,
            qibla,
            direction: prayer::compass_point(qibla),
        })
    }

    /// Today's schedule for the tray menu: a heading and one line per prayer.
    fn prayer_menu(&self) -> (String, Vec<String>) {
        match &self.prayer {
            Some(day) => {
                let heading = if day.settings.city.is_empty() {
                    "Jadwal sholat".to_string()
                } else {
                    format!("Jadwal sholat · {}", day.settings.city)
                };
                let lines = prayer::NAMES
                    .iter()
                    .zip(day.times)
                    .map(|(name, time)| format!("{name}\t{}", prayer::format_time(time)))
                    .collect();
                (heading, lines)
            }
            None => (
                "Jadwal sholat".to_string(),
                vec!["Lokasi belum diatur".to_string()],
            ),
        }
    }

    fn on_volume(&mut self, wparam: WPARAM, lparam: LPARAM) {
        self.show_level(LevelKind::Volume, wparam.0 as f32 / 1000.0, lparam.0 != 0);
    }

    fn on_volume_key(&mut self, vk: u16) {
        let result = self.audio.as_ref().and_then(|audio| audio.press(vk));
        if let Some((level, muted)) = result {
            self.show_level(LevelKind::Volume, level, muted);
        }
    }

    // Windows reports each power value once right after registering; those
    // first reports are starting values, not changes.
    fn on_power(&mut self, event: PowerEvent) {
        match event {
            PowerEvent::Brightness(brightness) => {
                let previous = self.brightness.replace(brightness);
                if previous.is_some_and(|previous| previous != brightness) {
                    self.show_level(LevelKind::Brightness, brightness as f32 / 100.0, false);
                }
            }
            // Plugging the charger in or out stays quiet; the power source only
            // decides whether a falling charge needs a warning.
            PowerEvent::OnAc(on_ac) => self.on_ac = Some(on_ac),
            PowerEvent::Battery(percent) => {
                let Some(previous) = self.battery.replace(percent) else {
                    return;
                };
                let on_battery = self.on_ac == Some(false);
                if on_battery
                    && self.settings.notify_battery
                    && LOW_BATTERY
                        .iter()
                        .any(|&level| previous > level && percent <= level)
                {
                    self.notify(notices::low_battery(percent));
                }
            }
        }
    }

    fn on_wifi(&mut self, connected: bool, ssid: String) {
        if connected {
            // Back on the same network within the grace period: nothing to say.
            if let Some((_, lost)) = self.wifi_lost.take() {
                if lost == ssid {
                    return;
                }
            }
            if self.settings.notify_wifi {
                self.notify(notices::wifi(&ssid, true));
            }
        } else {
            self.wifi_lost = Some((Instant::now() + WIFI_GRACE, ssid));
            self.schedule(BUSY_MS);
        }
    }

    fn on_device_change(&mut self, wparam: WPARAM, lparam: LPARAM) {
        let event = self
            .bluetooth
            .as_mut()
            .and_then(|bluetooth| bluetooth.event(wparam, lparam))
            .filter(|_| self.settings.notify_bluetooth);
        if let Some(event) = event {
            self.notify(notices::bluetooth(
                event.name.clone(),
                event.connected,
                event.audio,
                None,
            ));
            // Devices report their charge a moment after connecting.
            if event.connected {
                self.battery_check = Some((Instant::now() + BATTERY_DELAY, event));
                self.schedule(BUSY_MS);
            }
        }
    }

    /// Adds the device's battery level to its card once it is known.
    fn check_device_battery(&mut self, event: DeviceEvent) {
        let Some(percent) = bluetooth::battery_level(event.address) else {
            return;
        };
        let notice = notices::bluetooth(event.name, true, event.audio, Some(percent));
        let width = self.renderer.notice_width(&notice);
        self.notch.refresh_notice(notice, width, Instant::now());
    }

    fn draw(&self) {
        let n = &self.notch;
        let date = date_text();
        let soon = self.prayer_soon();
        let scene = Scene {
            width: n.width.value,
            height: n.height.value,
            eyes_visible: n.eyes_visible.value,
            blink: n.eyes.blink,
            look: n.eyes.look.value,
            clock: &self.clock,
            banner_visible: n.banner_visible.value,
            banner: n.banner.as_ref().map(|banner| &banner.content),
            music: self.track.as_ref().map(|track| MusicScene {
                title: &track.title,
                artist: &track.artist,
                playing: track.playing,
                position: track.position_now(),
                duration: track.duration,
                bars: self.bars,
            }),
            music_visible: n.music_visible.value,
            card_visible: n.card_visible.value,
            page: self.shown_page(),
            page_index: self
                .pages()
                .iter()
                .position(|page| *page == self.shown_page())
                .unwrap_or(0),
            pages: self.pages().len(),
            calendar: {
                let (year, month, day) = prayer::today();
                let (shown_year, shown_month) =
                    calendar::shift(year as i32, month as u32, self.month_offset);
                CalendarScene {
                    year: shown_year,
                    month: shown_month,
                    today: (self.month_offset == 0).then_some(day as u32),
                }
            },
            timer: Some(TimerScene {
                label: self.pomodoro.label(),
                left: calendar::format_left(self.pomodoro.left(Instant::now())),
                running: self.pomodoro.running(),
                on_break: self.pomodoro.phase == Phase::Break,
                session: self.pomodoro.session,
            }),
            strip: self.strip(),
            prayer_soon: soon.as_deref().unwrap_or(""),
            prayer: self.prayer_scene(),
            shelf: ShelfScene {
                items: self.shelf.items(),
                hover: self.shelf_hover,
            },
            drop_zone: self.drag_over,
            previous_page: self.page_switch.as_ref().map(|switch| switch.from),
            page_mix: self.page_switch.as_ref().map_or(1.0, |switch| {
                let t =
                    (switch.started.elapsed().as_secs_f32() / PAGE_SWITCH.as_secs_f32()).min(1.0);
                // Ease out: quick start, gentle landing.
                1.0 - (1.0 - t).powi(3)
            }),
            page_direction: self
                .page_switch
                .as_ref()
                .map_or(1.0, |switch| switch.direction),
            status: StatusScene {
                time: &self.clock,
                date: &date,
                weather: self.weather.as_ref().map(|weather| WeatherScene {
                    glyph: weather.glyph(),
                    temperature: format!("{}°", weather.temperature.round()),
                    description: weather.description(),
                    place: &self.settings.city,
                }),
                weather_hint: self.weather_hint(),
                // Read only while a card is showing; it is cheap anyway.
                battery: if n.card_visible.value > 0.01 {
                    system::battery_status()
                } else {
                    None
                },
            },
        };
        let _ = self.renderer.present(self.hwnd, self.origin(), &scene);
    }

    /// Top-center of the primary monitor.
    fn origin(&self) -> POINT {
        let screen_w = unsafe { GetSystemMetrics(SM_CXSCREEN) };
        POINT {
            x: (screen_w - self.renderer.size().cx) / 2,
            y: 0,
        }
    }

    /// Sets how soon the next tick comes. Animation frames follow the display
    /// refresh through the pacer; the timer then only backs it up.
    fn schedule(&mut self, ms: u32) {
        let animating = ms == FRAME_MS;
        self.pacer.set_active(animating);
        let ms = if animating { BUSY_MS } else { ms };
        if ms != self.timer_ms {
            self.timer_ms = ms;
            unsafe { SetTimer(Some(self.hwnd), TIMER_ID, ms, None) };
        }
    }

    /// Rebuilds the surface after a DPI or resolution change.
    fn relayout(&mut self) {
        if let Ok(renderer) = Renderer::new(window_scale(self.hwnd)) {
            self.renderer = renderer;
            self.renderer.sync_thumbnails(self.shelf.items());
            // Bitmaps belong to the old surface; decode the art again.
            let art = self.track.as_ref().and_then(|track| track.art.clone());
            self.renderer
                .set_album_art(art.as_deref().map(Vec::as_slice));
        }
        self.draw();
    }
}

unsafe extern "system" fn wndproc(hwnd: HWND, msg: u32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
    match msg {
        WM_TIMER => {
            with_app(App::tick);
            LRESULT(0)
        }
        frames::FRAME_MSG => {
            with_app(|app| {
                app.pacer.frame_taken();
                app.tick();
            });
            LRESULT(0)
        }
        WM_DPICHANGED | WM_DISPLAYCHANGE => {
            with_app(App::relayout);
            LRESULT(0)
        }
        WM_RBUTTONUP => {
            handle_menu(hwnd);
            LRESULT(0)
        }
        WM_LBUTTONUP => {
            with_app(|app| app.on_click(lparam));
            LRESULT(0)
        }
        WM_LBUTTONDOWN => {
            with_app(|app| app.on_press(lparam));
            LRESULT(0)
        }
        dragdrop::DRAG_MSG => {
            with_app(|app| app.on_drag(wparam.0 == 1));
            LRESULT(0)
        }
        dragdrop::DROP_MSG => {
            let paths = dragdrop::take_paths(lparam);
            with_app(|app| app.on_drop(paths));
            LRESULT(0)
        }
        WM_MOUSEMOVE => {
            let drag = with_app(|app| {
                app.on_hover(true);
                app.on_move(lparam)
            });
            // Outside the app borrow: dragging runs its own message loop.
            if let Some(path) = drag.flatten() {
                dragdrop::drag_out(&path);
            }
            LRESULT(0)
        }
        WM_SETCURSOR if (lparam.0 & 0xffff) as u32 == HTCLIENT => {
            let mut point = POINT::default();
            let _ = GetCursorPos(&mut point);
            let _ = ScreenToClient(hwnd, &mut point);
            let hand = with_app(|app| {
                let scale = window_scale(hwnd);
                app.clickable_at(point.x as f32 / scale, point.y as f32 / scale)
            });
            let shape = if hand == Some(true) {
                IDC_HAND
            } else {
                IDC_ARROW
            };
            if let Ok(cursor) = LoadCursorW(None, shape) {
                SetCursor(Some(cursor));
            }
            LRESULT(1)
        }
        WM_MOUSEWHEEL => {
            with_app(|app| app.on_wheel(wparam));
            LRESULT(0)
        }
        weather::WEATHER_MSG => {
            let weather = weather::take_weather(lparam);
            with_app(|app| app.on_weather(weather));
            LRESULT(0)
        }
        WM_MOUSELEAVE => {
            with_app(|app| app.on_hover(false));
            LRESULT(0)
        }
        media::MEDIA_MSG => {
            let track = media::take_track(lparam);
            with_app(|app| app.on_track(track));
            LRESULT(0)
        }
        system::VOLUME_MSG => {
            with_app(|app| app.on_volume(wparam, lparam));
            LRESULT(0)
        }
        keys::VOLUME_KEY_MSG => {
            with_app(|app| app.on_volume_key(wparam.0 as u16));
            LRESULT(0)
        }
        settings::SAVED_MSG => {
            with_app(App::update_prayer);
            LRESULT(0)
        }
        system::DEVICE_MSG => {
            with_app(|app| {
                if let Some(audio) = app.audio.as_mut() {
                    audio.rebind();
                }
            });
            LRESULT(0)
        }
        WM_POWERBROADCAST => {
            if let Some(event) = system::power_event(wparam, lparam) {
                with_app(|app| app.on_power(event));
            }
            LRESULT(1)
        }
        wifi::WIFI_MSG => {
            let ssid = wifi::take_ssid(lparam);
            with_app(|app| app.on_wifi(wparam.0 != 0, ssid));
            LRESULT(0)
        }
        WM_DEVICECHANGE => {
            with_app(|app| app.on_device_change(wparam, lparam));
            LRESULT(1)
        }
        tray::CALLBACK_MSG => {
            let event = lparam.0 as u32;
            if event == WM_RBUTTONUP || event == WM_LBUTTONUP {
                handle_menu(hwnd);
            }
            LRESULT(0)
        }
        WM_SETTINGCHANGE => {
            // Covers switching between light and dark taskbars.
            tray::refresh(hwnd);
            LRESULT(0)
        }
        WM_DESTROY => {
            tray::remove(hwnd);
            osd::set_suppressed(false);
            PostQuitMessage(0);
            LRESULT(0)
        }
        _ if msg == tray::taskbar_created_msg() => {
            tray::add(hwnd);
            LRESULT(0)
        }
        _ => DefWindowProcW(hwnd, msg, wparam, lparam),
    }
}

fn handle_menu(hwnd: HWND) {
    match show_menu(hwnd) {
        MENU_AUTOSTART => autostart::set_enabled(!autostart::is_enabled()),
        MENU_SETTINGS => settings::open(hwnd),
        MENU_QUIT => unsafe {
            let _ = DestroyWindow(hwnd);
        },
        _ => {}
    }
}

/// Runs `f` on the app unless it is already borrowed (window messages can
/// re-enter while a frame is being presented).
fn with_app<R>(f: impl FnOnce(&mut App) -> R) -> Option<R> {
    APP.with(|cell| {
        let mut app = cell.try_borrow_mut().ok()?;
        app.as_mut().map(f)
    })
}

fn show_menu(hwnd: HWND) -> usize {
    unsafe {
        let Ok(menu) = CreatePopupMenu() else {
            return 0;
        };

        // Today's schedule as a read-only submenu.
        let (heading, lines) = APP.with(|cell| {
            cell.try_borrow()
                .ok()
                .and_then(|app| app.as_ref().map(App::prayer_menu))
                .unwrap_or_default()
        });
        if let Ok(schedule) = CreatePopupMenu() {
            for line in &lines {
                let _ = AppendMenuW(schedule, MF_STRING | MF_GRAYED, 0, &HSTRING::from(line));
            }
            let _ = AppendMenuW(menu, MF_POPUP, schedule.0 as usize, &HSTRING::from(heading));
        }
        let _ = AppendMenuW(menu, MF_STRING, MENU_SETTINGS, w!("Pengaturan…"));
        let _ = AppendMenuW(menu, MF_SEPARATOR, 0, None);

        let autostart_flags = if autostart::is_enabled() {
            MF_STRING | MF_CHECKED
        } else {
            MF_STRING
        };
        let _ = AppendMenuW(
            menu,
            autostart_flags,
            MENU_AUTOSTART,
            w!("Jalankan saat Windows menyala"),
        );
        let _ = AppendMenuW(menu, MF_SEPARATOR, 0, None);
        let _ = AppendMenuW(menu, MF_STRING, MENU_QUIT, w!("Keluar Drop"));
        let mut cursor = POINT::default();
        let _ = GetCursorPos(&mut cursor);
        // Required so the menu closes when clicking elsewhere.
        let _ = SetForegroundWindow(hwnd);
        let chosen = TrackPopupMenu(
            menu,
            TPM_RETURNCMD | TPM_RIGHTBUTTON,
            cursor.x,
            cursor.y,
            None,
            hwnd,
            None,
        );
        // Lets the menu close properly when it was opened from the tray.
        let _ = PostMessageW(Some(hwnd), WM_NULL, WPARAM(0), LPARAM(0));
        let _ = DestroyMenu(menu);
        chosen.0 as usize
    }
}

/// Milliseconds since the last keyboard or mouse input anywhere in the session.
fn idle_ms() -> u32 {
    let mut info = LASTINPUTINFO {
        cbSize: size_of::<LASTINPUTINFO>() as u32,
        dwTime: 0,
    };
    unsafe {
        if GetLastInputInfo(&mut info).as_bool() {
            GetTickCount().wrapping_sub(info.dwTime)
        } else {
            0
        }
    }
}

/// "Jumat, 30 Okt"
fn date_text() -> String {
    const DAYS: [&str; 7] = [
        "Minggu", "Senin", "Selasa", "Rabu", "Kamis", "Jumat", "Sabtu",
    ];
    const MONTHS: [&str; 12] = [
        "Jan", "Feb", "Mar", "Apr", "Mei", "Jun", "Jul", "Agu", "Sep", "Okt", "Nov", "Des",
    ];
    let time = unsafe { GetLocalTime() };
    format!(
        "{}, {} {}",
        DAYS[time.wDayOfWeek as usize % 7],
        time.wDay,
        MONTHS[(time.wMonth as usize).clamp(1, 12) - 1]
    )
}

fn current_clock() -> String {
    let time = unsafe { GetLocalTime() };
    format!("{:02}:{:02}", time.wHour, time.wMinute)
}

/// Local minutes after midnight.
fn minutes_now() -> u32 {
    let time = unsafe { GetLocalTime() };
    time.wHour as u32 * 60 + time.wMinute as u32
}

fn window_scale(hwnd: HWND) -> f32 {
    let dpi = unsafe { GetDpiForWindow(hwnd) };
    if dpi == 0 {
        1.0
    } else {
        dpi as f32 / 96.0
    }
}

/// `--idle-secs N` shortens the idle delay, handy for trying the eyes out.
fn parse_idle_secs() -> Option<u32> {
    let mut args = std::env::args().skip(1);
    while let Some(arg) = args.next() {
        if arg == "--idle-secs" {
            return args.next()?.parse().ok();
        }
    }
    None
}

fn has_flag(flag: &str) -> bool {
    std::env::args().skip(1).any(|arg| arg == flag)
}

/// The system notification sound, played asynchronously.
fn chime() {
    unsafe {
        let _ = PlaySoundW(
            w!("Notification.Default"),
            None,
            SND_ALIAS | SND_ASYNC | SND_NODEFAULT,
        );
    }
}

/// The notice cards Drop shows.
mod notices {
    use crate::anim::{Accent, Notice, Trailing};
    use crate::prayer::{format_time, NAMES};
    use crate::render::glyph;
    use crate::{minutes_now, PrayerDay};

    pub fn wifi(ssid: &str, connected: bool) -> Notice {
        Notice {
            icon: glyph::WIFI,
            accent: Accent::Teal,
            progress: 1.0,
            title: if connected {
                ssid.to_string()
            } else {
                "Wi-Fi terputus".to_string()
            },
            subtitle: if connected {
                "Wi-Fi terhubung".to_string()
            } else {
                ssid.to_string()
            },
            trailing: Trailing::None,
            dim: !connected,
        }
    }

    /// `battery` fills the ring and joins the subtitle when the device reports it.
    pub fn bluetooth(name: String, connected: bool, audio: bool, battery: Option<u32>) -> Notice {
        let state = if connected { "Terhubung" } else { "Terputus" };
        Notice {
            icon: glyph::BLUETOOTH,
            accent: Accent::Blue,
            progress: battery.map_or(1.0, |percent| percent as f32 / 100.0),
            title: name,
            subtitle: match battery {
                Some(percent) => format!("{state} · {percent}%"),
                None => state.to_string(),
            },
            trailing: if audio && connected {
                Trailing::Icon(glyph::HEADPHONES)
            } else {
                Trailing::None
            },
            dim: !connected,
        }
    }

    pub fn low_battery(percent: u32) -> Notice {
        Notice {
            icon: glyph::battery(percent),
            accent: Accent::Red,
            progress: percent as f32 / 100.0,
            title: "Baterai lemah".to_string(),
            subtitle: "Colokkan charger".to_string(),
            trailing: Trailing::Text(format!("{percent}%")),
            dim: false,
        }
    }

    /// "Dzuhur 5 menit lagi", with the prayer time large on the right.
    pub fn prayer(index: usize, minutes_left: u32, day: &PrayerDay) -> Notice {
        prayer_reminder(
            NAMES[index],
            minutes_left,
            &format_time(day.times[index]),
            &day.settings.city,
        )
    }

    fn prayer_reminder(name: &str, minutes_left: u32, time: &str, city: &str) -> Notice {
        Notice {
            icon: glyph::PRAYER,
            accent: Accent::Green,
            progress: 1.0,
            title: format!("{name} {minutes_left} menit lagi"),
            subtitle: if city.is_empty() {
                "Waktu sholat".to_string()
            } else {
                format!("Waktu sholat · {city}")
            },
            trailing: Trailing::Text(time.to_string()),
            dim: false,
        }
    }

    /// Confirms a new location: names the place and the next prayer.
    pub fn prayer_ready(day: &PrayerDay) -> Notice {
        let city = if day.settings.city.is_empty() {
            "Lokasi tersimpan".to_string()
        } else {
            day.settings.city.clone()
        };
        // After Isya the next prayer is tomorrow's Subuh, a minute or so off
        // today's; today's time is close enough for a confirmation.
        let next = day.next(minutes_now()).unwrap_or(0);
        Notice {
            icon: glyph::PRAYER,
            accent: Accent::Green,
            progress: 1.0,
            title: "Jadwal sholat aktif".to_string(),
            subtitle: format!("{city} · berikutnya {}", NAMES[next]),
            trailing: Trailing::Text(format_time(day.times[next])),
            dim: false,
        }
    }

    /// `--demo` cycles through sample notices to preview their look.
    pub fn demo(index: usize) -> Notice {
        match index % 7 {
            6 => prayer_reminder("Dzuhur", 5, "11:40", "Jakarta"),
            0 => wifi("Rumah 5G", true),
            1 => bluetooth("Galaxy Buds2 Pro".to_string(), true, true, Some(70)),
            2 => low_battery(20),
            3 => wifi("Jaringan Kantor Lantai 3 dengan nama sangat panjang", true),
            4 => bluetooth("Galaxy Buds2 Pro".to_string(), false, true, None),
            _ => wifi("Rumah 5G", false),
        }
    }
}

/// Opens a file with its default app, as a double click in Explorer would.
fn open_file(path: &Path) {
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
