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
mod clipboard;
mod config;
mod downloads;
mod dragdrop;
mod frames;
mod geo;
mod health;
mod ical;
mod keys;
mod lyrics;
mod media;
mod net;
mod notifications;
mod osd;
mod prayer;
mod privacy;
mod render;
mod screenshots;
mod settings;
mod shelf;
mod sound;
mod system;
mod timers;
mod todo;
mod tray;
mod usage;
mod watch;
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
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::System::Ole::OleInitialize;
use windows::Win32::System::SystemInformation::{GetLocalTime, GetTickCount, GetTickCount64};
use windows::Win32::System::Threading::CreateMutexW;
use windows::Win32::UI::Controls::WM_MOUSELEAVE;
use windows::Win32::UI::HiDpi::{
    GetDpiForWindow, SetProcessDpiAwarenessContext, DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2,
};
use windows::Win32::UI::Input::KeyboardAndMouse::{
    GetLastInputInfo, RegisterHotKey, ReleaseCapture, SetCapture, SetFocus, TrackMouseEvent,
    LASTINPUTINFO, MOD_ALT, MOD_NOREPEAT, TME_LEAVE, TRACKMOUSEEVENT,
};
use windows::Win32::UI::Shell::ShellExecuteW;
use windows::Win32::UI::WindowsAndMessaging::{
    AppendMenuW, CreatePopupMenu, CreateWindowExW, DefWindowProcW, DestroyMenu, DestroyWindow,
    DispatchMessageW, GetCursorPos, GetForegroundWindow, GetMessageW, GetSystemMetrics, IsWindow,
    LoadCursorW, PostMessageW, PostQuitMessage, RegisterClassExW, SetCursor, SetForegroundWindow,
    SetTimer, ShowWindow, TrackPopupMenu, TranslateMessage, HTCLIENT, IDC_ARROW, IDC_HAND,
    MF_CHECKED, MF_SEPARATOR, MF_STRING, MSG, SM_CXSCREEN, SW_HIDE, SW_SHOWNOACTIVATE,
    SW_SHOWNORMAL, TPM_RETURNCMD, TPM_RIGHTBUTTON, WM_CHAR, WM_CLIPBOARDUPDATE, WM_DESTROY,
    WM_DEVICECHANGE, WM_DISPLAYCHANGE, WM_DPICHANGED, WM_ENDSESSION, WM_HOTKEY, WM_KILLFOCUS,
    WM_LBUTTONDOWN, WM_LBUTTONUP, WM_MOUSEMOVE, WM_MOUSEWHEEL, WM_NULL, WM_POWERBROADCAST,
    WM_RBUTTONUP, WM_SETCURSOR, WM_SETTINGCHANGE, WM_TIMER, WNDCLASSEXW, WS_EX_LAYERED,
    WS_EX_NOACTIVATE, WS_EX_TOOLWINDOW, WS_EX_TOPMOST, WS_POPUP,
};

use anim::{
    Accent, Bell, LevelKind, Motion, Notch, Notice, Spring, Trailing, CARD, CARD_NAV, COMPACT,
    COMPACT_LARGE, COMPACT_SMALL, DOWNLOAD_STRIP, DROP_ZONE, LYRIC_CARD_EXTRA, LYRIC_STRIP,
    LYRIC_STRIP_MAX, MUSIC_STRIP, TIMER_MUSIC_STRIP, TIMER_STRIP,
};
use bluetooth::{Bluetooth, DeviceEvent};
use calendar::{Phase, Pomodoro};
use config::{NotchSize, PrayerSettings, Settings};
use frames::Pacer;
use health::Health;
use ical::{Event, IcalWatch};
use lyrics::{Lyrics, LyricsWatch};
use media::{Command, Media, Track};
use notifications::{AppNotice, Inbox};
use render::{
    glyph, zoom, AgendaItem, CalendarControl, CalendarScene, DownloadScene, InboxRow, InboxScene,
    MusicScene, Page, PrayerScene, Renderer, Scene, ShelfHit, ShelfScene, StatusScene, Strip,
    TimerControl, TimerKind, TimerScene, TodoHit, TodoScene, UsageScene, WeatherScene,
};
use shelf::Shelf;
use system::{Audio, PowerEvent};
use timers::{Countdown, Stopwatch};
use todo::Todo;
use usage::Usage;
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
/// Logical pixels between the floating notch and the screen's top.
const FLOAT_GAP: f32 = 8.0;
const MENU_QUIT: usize = 1;
const MENU_AUTOSTART: usize = 2;
const MENU_MINIMAL: usize = 4;
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
const BARS_MS: u32 = 40;
const BAR_SPEEDS: [f32; 4] = [9.0, 13.0, 7.5, 15.0];
/// How much of each new peak joins the running loudness of the song.
const BAR_AVERAGING: f32 = 0.03;
/// Bar height for a peak as loud as that running loudness, 0..1.
const BAR_GAIN: f32 = 0.55;
/// Peaks below this count as silence.
const BAR_SILENCE: f32 = 0.002;
/// How long a new song is announced.
const SONG_SHOWN: Duration = Duration::from_secs(4);
/// How fast a control zooms in and out under the mouse.
const ZOOM_SPEED: f32 = 18.0;
/// How long a new lyric line takes to roll into place.
const LYRIC_ROLL: Duration = Duration::from_millis(350);
/// The mouse rests this long on the notch before the music card opens.
const HOVER_DELAY: Duration = Duration::from_millis(150);
/// The card stays open this long after the mouse leaves.
const LEAVE_GRACE: Duration = Duration::from_millis(400);
/// How long the shelf card stays open after files are dropped.
const SHELF_SHOWN: Duration = Duration::from_secs(3);
/// A picture copied this close to a screenshot is the same capture.
const SHOT_WINDOW: Duration = Duration::from_secs(5);
/// How long a shelf tile says its file was copied.
/// The quick timer's length until another is chosen.
const QUICK_DEFAULT: u64 = 5;
const COPIED_SHOWN: Duration = Duration::from_millis(1500);
/// How long a notification from another app stays up.
const APP_BANNER_SHOWN: Duration = Duration::from_secs(4);
/// Logical pixels a pressed shelf tile must move before it is dragged out.
const DRAG_THRESHOLD: f32 = 6.0;
/// Length of the cross-fade between card pages.
const PAGE_SWITCH: Duration = Duration::from_millis(260);
/// A prayer's arrival is still announced this many minutes after its time,
/// so a minute's tick that runs late does not miss it.
const PRAYER_ARRIVAL_MINUTES: u32 = 2;
/// How long the card stays open after Alt+N.
const KEY_CARD: Duration = Duration::from_secs(8);
const HOTKEY_ID: i32 = 1;
/// Prayer reminders stay up longer than other notices.
const PRAYER_NOTICE_DURATION: Duration = Duration::from_secs(10);
const MENU_SETTINGS: usize = 3;
/// Agenda events are announced this many minutes ahead.
const EVENT_LEAD: u32 = 10;
/// The agenda looks this many days ahead.
const AGENDA_DAYS: i64 = 14;
/// A timed event stays on the agenda this long after it starts.
const AGENDA_GRACE: u32 = 30;
/// Half a blink of the text cursor, in milliseconds.
const CARET_BLINK: u128 = 530;

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
    /// Running loudness of the song, which the bars are measured against.
    peak_average: f32,
    pomodoro: Pomodoro,
    quick: Countdown,
    stopwatch: Stopwatch,
    /// The timer the card shows.
    timer_view: TimerKind,
    /// Months away from the current one on the calendar page.
    month_offset: i32,
    /// The notch is showing (it hides while another app is full screen).
    shown: bool,
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
    /// Shelf tile just copied, and when.
    shelf_copied: Option<(usize, Instant)>,
    /// Shelf items picked out to remove together.
    shelf_picked: Vec<PathBuf>,
    /// The camera and microphone bits in use, see `privacy`.
    privacy: usize,
    /// Files still arriving in the Downloads folder: name and bytes so far.
    downloads: Vec<(String, u64)>,
    /// Whether the default microphone is muted; None without one.
    mic_muted: Option<bool>,
    /// When a picture was last copied, and a screenshot last saved: one
    /// capture brings both.
    clip_at: Option<Instant>,
    shot_at: Option<Instant>,
    /// What the Windows notification center holds.
    inbox: Inbox,
    /// A notification came in and its page has not been seen since.
    unread: bool,
    bell: Bell,
    /// Notification row under the mouse.
    inbox_hover: Option<usize>,
    /// A shelf tile pressed and the logical point it was pressed at.
    press: Option<(usize, f32, f32)>,
    /// Which page the open card shows when there is music.
    page: Page,
    /// The page dots have opened into page icons.
    nav_open: bool,
    /// What the mouse is on that zooms in, see [`render::zoom`].
    hover_zoom: Option<u16>,
    /// What is zoomed in and how far, 0..1, easing in and out.
    zooms: Vec<(u16, f32)>,
    /// The mouse is on an app notification.
    banner_hover: bool,
    /// The volume slider is being dragged.
    volume_drag: bool,
    /// The volume slider is open under the mouse.
    volume_open: bool,
    /// The lyrics button on the music card is on.
    show_lyrics: bool,
    /// The search found no lyrics for this song.
    lyrics_missing: bool,
    /// Strip width for this song's longest lyric line.
    lyric_strip: f32,
    /// The lyric line sung, and when it took over from the last one.
    lyric_line: Option<usize>,
    lyric_moved: Option<Instant>,
    /// Click-to-open mode: the card was clicked open and stays while hovered.
    card_clicked: bool,
    weather_watch: WeatherWatch,
    weather: Option<Weather>,
    ical_watch: IcalWatch,
    lyrics_watch: LyricsWatch,
    /// Synced lyrics of the song playing, once found: (artist, title, lines).
    lyrics: Option<(String, String, Lyrics)>,
    /// Events around today from the calendar feed.
    events: Vec<Event>,
    /// Events already announced: (day, minute, title).
    event_reminded: Vec<(i64, u32, String)>,
    health: Health,
    usage: Usage,
    /// The Screen Time switch's thumb, from today (0) to the week (1).
    usage_switch: Spring,
    todo: Todo,
    /// Task under the mouse.
    todo_hover: Option<usize>,
    /// A new task being typed.
    editing: Option<Editing>,
}

/// A new task being typed in the notch, which holds the keyboard meanwhile.
struct Editing {
    text: String,
    /// The window that had the keyboard before; it gets it back afterwards.
    previous: HWND,
    /// First half of a character outside the Basic Multilingual Plane,
    /// waiting for the second.
    surrogate: Option<u16>,
    /// The cursor blinks from here, starting over with each key.
    since: Instant,
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
    arrived: [bool; 5],
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
            arrived: [false; 5],
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
            peak_average: 0.0,
            pomodoro: Pomodoro::default(),
            quick: Countdown::new(QUICK_DEFAULT),
            stopwatch: Stopwatch::default(),
            timer_view: TimerKind::Pomodoro,
            month_offset: 0,
            page_switch: None,
            shown: true,
            pacer: Pacer::start(hwnd),
            shelf: Shelf::load(),
            drag_over: false,
            shelf_hover: None,
            shelf_copied: None,
            shelf_picked: Vec::new(),
            privacy: 0,
            downloads: Vec::new(),
            mic_muted: None,
            clip_at: None,
            shot_at: None,
            inbox: Inbox::default(),
            unread: false,
            bell: Bell::new(Instant::now()),
            inbox_hover: None,
            todo: Todo::load(),
            todo_hover: None,
            editing: None,
            press: None,
            page: Page::Music,
            nav_open: false,
            hover_zoom: None,
            zooms: Vec::new(),
            banner_hover: false,
            volume_drag: false,
            volume_open: false,
            show_lyrics: false,
            lyrics_missing: false,
            lyric_strip: 0.0,
            lyric_line: None,
            lyric_moved: None,
            card_clicked: false,
            weather_watch: WeatherWatch::start(hwnd),
            ical_watch: IcalWatch::start(hwnd),
            lyrics_watch: LyricsWatch::start(hwnd),
            lyrics: None,
            events: Vec::new(),
            event_reminded: Vec::new(),
            weather: None,
            health: Health::default(),
            usage: Usage::load(prayer::today(), now),
            usage_switch: Spring::new(0.0, 260.0, 32.0, 0.002),
        };
        config::ensure_exists();
        // Load quietly: the "schedule ready" notice is for later edits only.
        app.config_stamp = config::modified();
        app.apply_settings(false);
        app.usage_switch.value = app.usage_switch.target;
        app.mic_muted = app.audio.as_ref().and_then(Audio::mic_muted);
        app.update_usage();
        app.sync_usage_icons();
        app.renderer.sync_thumbnails(app.shelf.items());
        app.draw();
        let _ = ShowWindow(hwnd, SW_SHOWNOACTIVATE);
        tray::add(hwnd);
        osd::set_suppressed(true);
        keys::install(hwnd);
        dragdrop::accept_drops(hwnd);
        clipboard::listen(hwnd);
        sound::prepare();
        // Alt+N opens the card and steps through its pages without the mouse.
        let _ = RegisterHotKey(
            Some(hwnd),
            HOTKEY_ID,
            MOD_ALT | MOD_NOREPEAT,
            u32::from(b'N'),
        );
        app.schedule(COMPACT_MS);
        APP.with(|cell| *cell.borrow_mut() = Some(app));
        // After the app exists: Windows answers right away with the current values.
        system::watch_power(hwnd);
        system::stay_on_top(hwnd);
        system::watch_foreground(hwnd);
        screenshots::watch(hwnd);
        notifications::watch(hwnd);
        privacy::watch(hwnd);
        downloads::watch(hwnd);
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

        if !self.shelf_picked.is_empty() {
            if self.shown_page() == Page::Shelf && self.notch.card_visible.value >= 0.5 {
                let items = self.shelf.items();
                self.shelf_picked.retain(|path| items.contains(path));
            } else {
                self.shelf_picked.clear();
            }
        }

        let idle = self.settings.module_eyes
            && !self.settings.notch_minimal
            && idle_ms() >= self.idle_after_ms;
        self.notch.set_idle(idle, now);
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
                match index % 17 {
                    12 => {
                        self.notify_prayer("Dzuhur 12:02".to_string(), "5 Menit lagi".to_string())
                    }
                    13 => self
                        .notify_prayer("Dzuhur 12:02".to_string(), "Waktunya sholat".to_string()),
                    15 => self.notify(notices::mic(true)),
                    16 => self.notify(notices::mic(false)),
                    14 => self.notch.show_song(
                        "Judul lagu \u{b7} Nama penyanyi".to_string(),
                        220.0,
                        SONG_SHOWN,
                        now,
                    ),
                    _ => self.notify(notices::demo(index)),
                }
            }
        }
        let clock = current_clock();
        let clock_changed = clock != self.clock;
        self.clock = clock;
        if clock_changed {
            self.update_prayer();
            self.update_health();
            self.update_usage();
            self.update_agenda();
        }

        if let Some(finished) = self.pomodoro.advance(now) {
            self.on_phase_end(finished);
        }
        let minutes = self.quick.minutes();
        if self.quick.finish(now) {
            self.on_quick_end(minutes);
        }
        self.update_music(now);
        self.step_aside_for_fullscreen();
        let switching = self
            .page_switch
            .as_ref()
            .is_some_and(|switch| now.duration_since(switch.started) < PAGE_SWITCH);
        if !switching {
            self.page_switch = None;
        }
        let rolling = self.update_lyric_roll(now);
        let zooming = self.step_zooms(dt);
        let sliding = self.usage_switch.step(dt);
        let moving = self.notch.step(now, dt) || switching || rolling || zooming || sliding;
        let moving = self.step_bell(now) || moving;
        let bars_moving = self.step_bars(dt);
        // The open card shows a running progress bar.
        let card_open = self.notch.card_visible.value > 0.01;
        // A running timer counts down every second, in the strip or the card.
        let timer_running =
            self.pomodoro.running() || self.quick.running() || self.stopwatch.running();

        // In lyric mode the strip's line moves on with the song.
        let lyric_strip = self.strip() == Strip::Music && self.shown_lyric().is_some();
        if moving || clock_changed || bars_moving || card_open || timer_running || lyric_strip {
            self.draw();
        }
        // Typing needs the cursor to blink on time.
        let waiting = self.wifi_lost.is_some()
            || self.battery_check.is_some()
            || self.demo.is_some()
            || self.editing.is_some()
            || lyric_strip
            // Lyrics move on with the song.
            || (card_open && self.shown_page() == Page::Music && self.shown_lyric().is_some());
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
        // A new song is announced for a moment; the song already playing when
        // Drop starts is not.
        if let Some(track) = track.as_ref().filter(|track| {
            new_song && self.tracks_seen && self.settings.module_music && track.playing
        }) {
            let song = if track.artist.is_empty() {
                track.title.clone()
            } else {
                format!("{} \u{2014} {}", track.title, track.artist)
            };
            let width = self.renderer.song_banner_width(&song);
            self.notch.show_song(song, width, SONG_SHOWN, now);
        }
        if new_song {
            self.lyrics = None;
            self.lyrics_missing = false;
            self.lyric_strip = 0.0;
            if let Some(track) = track.as_ref().filter(|_| self.settings.module_lyrics) {
                self.lyrics_watch
                    .request(&track.artist, &track.title, track.duration);
            }
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
        if self.editing.is_some() && self.shown_page() != Page::Todo {
            self.finish_editing(true, true);
        }
        let by_click = self.settings.open_on_click;
        let open = (hovering && (!by_click || self.card_clicked))
            || self.card_until.is_some()
            || self.drag_over
            || self.editing.is_some();
        self.notch.set_peek(hovering && by_click && !open);
        if !open {
            // The next opening returns to the last page, on this month, unless
            // notifications are waiting to be seen.
            if self.unread {
                self.page = Page::Notifications;
            }
            self.month_offset = 0;
            self.page_switch = None;
            self.nav_open = false;
            self.volume_open = false;
        } else if self.notch.card_visible.value >= 0.5 && self.shown_page() == Page::Notifications {
            self.unread = false;
        }
        self.notch.set_nav(self.nav_open);
        self.notch.set_volume(self.volume_open);
        let lyric_mode = self.shown_lyric().is_some();
        let extra = if lyric_mode && self.shown_page() == Page::Music {
            LYRIC_CARD_EXTRA
        } else {
            0.0
        };
        let card = open.then_some(if self.drag_over {
            DROP_ZONE
        } else if self.nav_open {
            (CARD_NAV.0 + extra, CARD_NAV.1)
        } else {
            (CARD.0 + extra, CARD.1)
        });
        let strip = match self.strip() {
            Strip::Timer if self.song_playing() => Some(TIMER_MUSIC_STRIP),
            Strip::Timer => Some(TIMER_STRIP),
            Strip::Download => Some(DOWNLOAD_STRIP),
            Strip::Music if lyric_mode => Some((
                self.lyric_strip.clamp(LYRIC_STRIP.0, LYRIC_STRIP_MAX),
                LYRIC_STRIP.1,
            )),
            Strip::Music => Some(MUSIC_STRIP),
            Strip::None => None,
        };
        self.notch.set_live(strip, card);
    }

    /// What the closed notch shows beside the clock: a running or paused
    /// timer first, then music that is playing.
    fn strip(&self) -> Strip {
        let modules = &self.settings;
        if modules.notch_minimal {
            Strip::None
        } else if modules.module_calendar && self.live_timer().is_some() {
            Strip::Timer
        } else if !self.downloads.is_empty() {
            Strip::Download
        } else if self.music().is_some_and(|track| track.playing) {
            Strip::Music
        } else {
            Strip::None
        }
    }

    fn song_playing(&self) -> bool {
        self.music().is_some_and(|track| track.playing)
    }

    /// The track playing, unless the music module is off.
    fn music(&self) -> Option<&Track> {
        self.track.as_ref().filter(|_| self.settings.module_music)
    }

    /// The card's pages, in wheel order: those whose module is on, and of
    /// those, music only when something plays.
    fn pages(&self) -> Vec<Page> {
        let modules = &self.settings;
        let mut pages = Vec::with_capacity(9);
        if self.music().is_some() {
            pages.push(Page::Music);
        }
        pages.push(Page::Status);
        if modules.notify_apps {
            pages.push(Page::Notifications);
        }
        if modules.module_prayer && self.prayer.is_some() {
            pages.push(Page::Prayer);
        }
        if modules.module_calendar || modules.ical_urls.iter().any(|url| !url.is_empty()) {
            pages.push(Page::Calendar);
        }
        if modules.module_calendar {
            pages.push(Page::Pomodoro);
        }
        if modules.module_todo {
            pages.push(Page::Todo);
        }
        if modules.module_usage {
            pages.push(Page::Usage);
        }
        if modules.module_shelf {
            pages.push(Page::Shelf);
        }
        pages
    }

    fn on_drag(&mut self, over: bool) {
        self.drag_over = over;
        self.notch.set_liquid(over);
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
        self.notch.ripple_at = self.cursor_on_canvas();
        self.notch.settle_liquid();
        self.page = Page::Shelf;
        self.card_until = Some(Instant::now() + SHELF_SHOWN);
        self.tick();
    }

    /// A new screenshot goes on the shelf with a notice, not the whole card,
    /// so it stays out of the way of whatever was captured.
    fn on_screenshot(&mut self, path: PathBuf) {
        if !self.settings.module_shelf || !self.settings.shelf_screenshots {
            return;
        }
        // The capture reached the clipboard too; keep only the saved file.
        let copied = self.clip_at.is_some_and(|at| at.elapsed() < SHOT_WINDOW);
        let last = self.shelf.items().len().checked_sub(1);
        if let Some(last) = last.filter(|_| copied) {
            if shelf::is_clip(&self.shelf.items()[last]) {
                self.shelf.remove(last);
            }
        }
        self.shot_at = Some(Instant::now());
        if self.shelf.add([path]) == 0 {
            return;
        }
        self.renderer.sync_thumbnails(self.shelf.items());
        self.notify(notices::screenshot(system::clipboard_has_image()));
        self.tick();
    }

    /// The clipboard changed: what was copied is saved off this thread.
    fn on_clipboard(&self) {
        if self.settings.module_shelf && self.settings.shelf_clipboard {
            clipboard::capture(self.hwnd);
        }
    }

    /// Something copied was saved: it joins the shelf quietly. A picture
    /// right after a screenshot is that screenshot again.
    fn on_clip(&mut self, path: PathBuf) {
        let repeat = !shelf::is_text_clip(&path)
            && self.shot_at.is_some_and(|at| at.elapsed() < SHOT_WINDOW);
        if repeat {
            let _ = std::fs::remove_file(&path);
            return;
        }
        self.clip_at = (!shelf::is_text_clip(&path)).then(Instant::now);
        self.shelf.add([path]);
        self.renderer.sync_thumbnails(self.shelf.items());
        self.tick();
    }

    /// Copies a shelf item: a text clip as its text, the rest as files.
    fn copy_shelf_item(&mut self, index: usize) {
        let Some(path) = self.shelf.items().get(index) else {
            return;
        };
        let copied = if shelf::is_text_clip(path) {
            std::fs::read_to_string(path).is_ok_and(|text| system::copy_text(self.hwnd, &text))
        } else {
            system::copy_file(self.hwnd, path)
        };
        if copied {
            self.shelf_copied = Some((index, Instant::now()));
            self.draw();
        }
    }

    /// The notification center changed. What just arrived shows as a notice,
    /// and stays marked until its page has been seen.
    fn on_inbox(&mut self, inbox: Inbox) {
        self.inbox = inbox;
        self.renderer.sync_logos(&self.inbox.items);
        if self.inbox.items.is_empty() || !self.settings.notify_apps {
            self.unread = false;
        } else if self.inbox.fresh && !self.quiet() {
            self.unread = true;
            if self.settings.sound {
                sound::play_app();
            }
            let notice = self.inbox.items[0].clone();
            self.notch
                .show_app(notice, APP_BANNER_SHOWN, Instant::now());
            self.schedule(FRAME_MS);
        }
        self.tick();
    }

    /// Opens the app a notification came from and takes the notification
    /// away, as clicking a Windows toast does. Windows keeps a toast's own
    /// action to itself, so this reaches the app, not the message.
    fn open_notice(&mut self, notice: &AppNotice) {
        if !notice.app_id.is_empty() {
            open_file(Path::new(&format!("shell:AppsFolder\\{}", notice.app_id)));
        }
        notifications::remove(notice.id);
    }

    fn inbox_scene(&self) -> InboxScene<'_> {
        let now = notifications::now_seconds();
        let rows = self.inbox.items.iter().map(|item| InboxRow {
            title: &item.title,
            app_id: &item.app_id,
            detail: if item.app.is_empty() || item.app == item.title {
                item.body.clone()
            } else {
                format!("{} \u{b7} {}", item.app, item.body)
            },
            age: notifications::age_label(now - item.created),
        });
        InboxScene {
            rows: rows.collect(),
            total: self.inbox.total,
            unread: self.unread,
            hover: self.inbox_hover,
        }
    }

    /// Whether the music card's volume slider is at (`x`, `y`).
    fn volume_at(&self, x: f32, y: f32) -> bool {
        self.volume_open
            && self.shown_page() == Page::Music
            && self.notch.card_visible.value >= 0.5
            && self
                .renderer
                .volume_at(x, y, self.notch.width.value, self.shown_lyric().is_some())
    }

    /// The speaker opens the slider, which stays while the mouse is on it.
    fn update_volume(&mut self, x: f32, y: f32) {
        let open = self.shown_page() == Page::Music
            && self.notch.card_visible.value >= 0.5
            && self.renderer.volume_zone_at(
                x,
                y,
                self.notch.width.value,
                self.shown_lyric().is_some(),
                self.volume_open,
            );
        if open != self.volume_open {
            self.volume_open = open;
            self.tick();
        }
    }

    fn set_volume_at(&mut self, x: f32) {
        let level =
            self.renderer
                .volume_level(x, self.notch.width.value, self.shown_lyric().is_some());
        if let Some(audio) = &self.audio {
            audio.set_volume(level);
        }
        self.draw();
    }

    /// A press on a shelf tile may become a drag out.
    fn on_press(&mut self, lparam: LPARAM) {
        let (x, y) = self.logical_point(lparam);
        if self.volume_at(x, y) {
            self.volume_drag = true;
            unsafe {
                SetCapture(self.hwnd);
            }
            self.set_volume_at(x);
            return;
        }
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
        if self.volume_drag {
            self.set_volume_at(x);
            return None;
        }
        self.update_nav(x, y);
        self.update_volume(x, y);
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
            Some(ShelfHit::Tile(index) | ShelfHit::Remove(index) | ShelfHit::Copy(index)) => {
                Some(index)
            }
            _ => None,
        };
        let todo_hover = match self.todo_hit(x, y) {
            Some(TodoHit::Toggle(index) | TodoHit::Remove(index)) => Some(index),
            _ => None,
        };
        let inbox_hover = self.inbox_row(x, y);
        let hover_zoom = self.zoom_at(x, y);
        let banner_hover = self.notch.app_banner().is_some();
        if hover != self.shelf_hover
            || todo_hover != self.todo_hover
            || inbox_hover != self.inbox_hover
            || hover_zoom != self.hover_zoom
            || banner_hover != self.banner_hover
        {
            self.hover_zoom = hover_zoom;
            self.banner_hover = banner_hover;
            self.shelf_hover = hover;
            self.todo_hover = todo_hover;
            self.inbox_hover = inbox_hover;
            self.draw();
        }
        None
    }

    fn todo_scene(&self) -> TodoScene<'_> {
        TodoScene {
            tasks: self.todo.tasks(),
            left: self.todo.left(),
            hover: self.todo_hover,
            editing: self.editing.as_ref().map(|editing| editing.text.as_str()),
            caret: self.editing.as_ref().is_some_and(|editing| {
                (editing.since.elapsed().as_millis() / CARET_BLINK).is_multiple_of(2)
            }),
        }
    }

    /// The side arrow at (`x`, `y`), when the card has pages to switch to.
    fn arrow_at(&self, x: f32, y: f32) -> Option<f32> {
        if !self.settings.card_arrows || self.pages().len() < 2 {
            return None;
        }
        let n = &self.notch;
        self.renderer
            .card_arrow_at(x, y, n.width.value, n.height.value)
    }

    /// The icon at (`x`, `y`) while the page icons show: an index into the
    /// pages, or one past them for Settings.
    fn nav_item_at(&self, x: f32, y: f32) -> Option<usize> {
        if !self.nav_open {
            return None;
        }
        self.renderer.nav_item_at(x, y, self.pages().len() + 1)
    }

    /// The page dots open into the icons under the mouse, and the card grows
    /// to hold them until the mouse leaves that row.
    fn update_nav(&mut self, x: f32, y: f32) {
        let open = self.notch.card_visible.value >= 0.5
            && !self.drag_over
            && self
                .renderer
                .nav_zone_at(x, y, self.nav_open, self.pages().len());
        if open != self.nav_open {
            self.nav_open = open;
            self.tick();
        }
    }

    /// Goes to the page at `index`, or opens Settings just past the last one.
    fn go_to(&mut self, index: usize) {
        self.finish_editing(true, true);
        let pages = self.pages();
        let Some(&target) = pages.get(index) else {
            settings::open(self.hwnd);
            self.tick();
            return;
        };
        let shown = self.shown_page();
        let current = pages.iter().position(|page| *page == shown).unwrap_or(0);
        if target != shown {
            self.page_switch = Some(PageSwitch {
                from: shown,
                started: Instant::now(),
                direction: if index > current { 1.0 } else { -1.0 },
            });
        }
        self.page = target;
        self.tick();
    }

    fn todo_hit(&self, x: f32, y: f32) -> Option<TodoHit> {
        if self.shown_page() != Page::Todo || self.notch.card_visible.value < 0.5 {
            return None;
        }
        self.renderer
            .todo_hit(x, y, self.notch.width.value, &self.todo_scene())
    }

    /// Click on the task page: add (or save what is typed), tick a task off,
    /// remove one, or clear the finished ones.
    fn on_todo_click(&mut self, x: f32, y: f32) {
        match self.todo_hit(x, y) {
            Some(TodoHit::Add) if self.editing.is_some() => self.finish_editing(true, true),
            Some(TodoHit::Add) if !self.todo.is_full() => self.start_editing(),
            Some(TodoHit::Toggle(index)) => self.todo.toggle(index),
            Some(TodoHit::Remove(index)) => {
                self.todo.remove(index);
                self.todo_hover = None;
            }
            Some(TodoHit::ClearDone) => self.todo.clear_done(),
            _ => return,
        }
        self.tick();
    }

    /// Takes the keyboard so a new task can be typed in the notch.
    fn start_editing(&mut self) {
        let previous = unsafe { GetForegroundWindow() };
        self.editing = Some(Editing {
            text: String::new(),
            previous,
            surrogate: None,
            since: Instant::now(),
        });
        unsafe {
            let _ = SetForegroundWindow(self.hwnd);
            let _ = SetFocus(Some(self.hwnd));
        }
    }

    /// Ends typing, adding the task when `save`, and hands the keyboard back
    /// when `give_back` (not when another window already took it).
    fn finish_editing(&mut self, save: bool, give_back: bool) {
        let Some(editing) = self.editing.take() else {
            return;
        };
        if save {
            self.todo.add(&editing.text);
        }
        unsafe {
            if give_back
                && GetForegroundWindow() == self.hwnd
                && IsWindow(Some(editing.previous)).as_bool()
            {
                let _ = SetForegroundWindow(editing.previous);
            }
        }
        // Stay open a moment, so the card does not snap shut under the mouse.
        self.card_until = Some(Instant::now() + LEAVE_GRACE);
    }

    /// A typed character while a new task is being written. Enter adds the
    /// task and starts the next one, Escape stops.
    fn on_char(&mut self, unit: u16) {
        let Some(editing) = self.editing.as_mut() else {
            return;
        };
        editing.since = Instant::now();
        match unit {
            0x0D if !editing.text.trim().is_empty() && !self.todo.is_full() => {
                let text = std::mem::take(&mut editing.text);
                self.todo.add(&text);
                if self.todo.is_full() {
                    self.finish_editing(false, true);
                }
            }
            0x0D | 0x1B => self.finish_editing(false, true),
            0x08 => {
                editing.text.pop();
            }
            // Ctrl+V
            0x16 => {
                if let Some(pasted) = system::clipboard_text() {
                    let room = todo::MAX_CHARS - editing.text.chars().count();
                    editing.text.extend(todo::clean(&pasted).chars().take(room));
                }
            }
            0xD800..=0xDBFF => editing.surrogate = Some(unit),
            _ if unit < 0x20 => {}
            _ => {
                let units = match editing.surrogate.take() {
                    Some(high) if (0xDC00..=0xDFFF).contains(&unit) => vec![high, unit],
                    _ => vec![unit],
                };
                if editing.text.chars().count() < todo::MAX_CHARS {
                    editing
                        .text
                        .extend(char::decode_utf16(units).filter_map(|c| c.ok()));
                }
            }
        }
        self.tick();
    }

    /// Whether (`x`, `y`) is over something clickable on the open card.
    fn clickable_at(&self, x: f32, y: f32) -> bool {
        if self.notch.app_banner().is_some() {
            return true;
        }
        if self.settings.open_on_click && self.notch.card_visible.target == 0.0 {
            return true;
        }
        if self.notch.card_visible.value < 0.5 || self.drag_over {
            return false;
        }
        let width = self.notch.width.value;
        if self.arrow_at(x, y).is_some() || self.nav_item_at(x, y).is_some() || self.volume_at(x, y)
        {
            return true;
        }
        match self.shown_page() {
            Page::Music => {
                let lyrics = self.shown_lyric().is_some();
                self.renderer
                    .music_control_at(x, y, width, lyrics)
                    .is_some()
                    || self.renderer.lyrics_button_at(x, y, width, lyrics)
                    || self.renderer.progress_at(x, y, width, lyrics).is_some()
                    || (lyrics && self.renderer.lyric_line_at(x, y, width).is_some())
            }
            Page::Calendar => self.renderer.calendar_control_at(x, y, width).is_some(),
            Page::Pomodoro => self
                .renderer
                .timer_control_at(x, y, width, self.picking_minutes())
                .is_some(),
            Page::Shelf => self.shelf_hit(x, y).is_some(),
            Page::Notifications => {
                self.inbox.total > 0 && self.renderer.inbox_clear_at(x, y, width)
                    || self
                        .renderer
                        .inbox_row_at(x, y, width, self.inbox.items.len())
                        .is_some()
            }
            Page::Todo => self.todo_hit(x, y).is_some(),
            Page::Usage => self.renderer.usage_switch_at(x, y, width).is_some(),
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

    /// Picks a shelf tile, or lets it go again.
    fn toggle_shelf_pick(&mut self, index: usize) {
        let Some(path) = self.shelf.items().get(index).cloned() else {
            return;
        };
        match self.shelf_picked.iter().position(|item| *item == path) {
            Some(place) => {
                self.shelf_picked.remove(place);
            }
            None => self.shelf_picked.push(path),
        }
    }

    /// Click on the shelf page: open, copy, pick, remove, or clear all. Once
    /// something is picked, every tile click picks too.
    fn on_shelf_click(&mut self, x: f32, y: f32) {
        let pressed = self.press.take();
        if pressed.is_some() {
            unsafe {
                let _ = ReleaseCapture();
            }
        }
        let picking = !self.shelf_picked.is_empty() || system::ctrl_down();
        match self.shelf_hit(x, y) {
            Some(ShelfHit::Tile(index) | ShelfHit::Remove(index) | ShelfHit::Copy(index))
                if picking && pressed.is_none_or(|(i, ..)| i == index) =>
            {
                self.toggle_shelf_pick(index);
                self.tick();
                self.draw();
                return;
            }
            Some(ShelfHit::Tile(index)) if pressed.is_some_and(|(i, ..)| i == index) => {
                // A clip is there to be pasted back.
                let clip = self.shelf.items().get(index).filter(|p| shelf::is_clip(p));
                if clip.is_some() {
                    self.copy_shelf_item(index);
                    return;
                }
                if let Some(path) = self.shelf.items().get(index) {
                    open_file(path);
                }
            }
            Some(ShelfHit::Copy(index)) => {
                self.copy_shelf_item(index);
                return;
            }
            Some(ShelfHit::Remove(index)) => self.shelf.remove(index),
            Some(ShelfHit::ClearAll) if !self.shelf_picked.is_empty() => {
                for path in std::mem::take(&mut self.shelf_picked) {
                    let place = self.shelf.items().iter().position(|item| *item == path);
                    if let Some(index) = place {
                        self.shelf.remove(index);
                    }
                }
            }
            Some(ShelfHit::ClearAll) => self.shelf.clear(),
            _ => return,
        }
        self.shelf_copied = None;
        self.shelf_hover = None;
        self.renderer.sync_thumbnails(self.shelf.items());
        self.tick();
    }

    /// The mouse in the canvas' logical pixels.
    fn cursor_on_canvas(&self) -> (f32, f32) {
        let mut point = POINT::default();
        let scale = window_scale(self.hwnd);
        unsafe {
            let _ = GetCursorPos(&mut point);
            let _ = ScreenToClient(self.hwnd, &mut point);
        }
        (point.x as f32 / scale, point.y as f32 / scale)
    }

    fn logical_point(&self, lparam: LPARAM) -> (f32, f32) {
        let scale = window_scale(self.hwnd);
        (
            (lparam.0 & 0xffff) as i16 as f32 / scale,
            ((lparam.0 >> 16) & 0xffff) as i16 as f32 / scale,
        )
    }

    /// What zooms in for the mouse at (`x`, `y`): a page icon, a side arrow, or a media
    /// control on an app notification, the widened notch or the music card.
    fn zoom_at(&self, x: f32, y: f32) -> Option<u16> {
        if let Some(index) = self.nav_item_at(x, y) {
            return Some(zoom::NAV + index as u16);
        }
        if self.notch.card_visible.value >= 0.5 {
            if let Some(direction) = self.arrow_at(x, y) {
                return Some(if direction < 0.0 {
                    zoom::ARROW_BACK
                } else {
                    zoom::ARROW_ON
                });
            }
        }
        self.track.as_ref()?;
        let width = self.notch.width.value;
        if self.notch.app_banner().is_some() {
            self.renderer.peek_zoom_at(x, y, width, 1.0)
        } else if self.notch.card_visible.value >= 0.5 {
            if self.shown_page() != Page::Music {
                return None;
            }
            let lyrics = self.shown_lyric().is_some();
            let open = self.volume_open;
            if self.renderer.volume_zone_at(x, y, width, lyrics, open) {
                return Some(zoom::VOLUME);
            }
            self.renderer.music_zoom_at(x, y, width, lyrics)
        } else if self.notch.peek_visible.value >= 0.5 {
            self.renderer.peek_zoom_at(x, y, width, self.notch.scale)
        } else {
            None
        }
    }

    /// Shakes the bell while the closed notch shows it. Returns true while it
    /// moves.
    fn step_bell(&mut self, now: Instant) -> bool {
        let n = &self.notch;
        let shown = self.unread
            && !self.settings.notch_minimal
            && n.banner_visible.value < 0.01
            && n.card_visible.value < 0.01
            && n.music_visible.value < 0.01
            && n.eyes_visible.value < 0.01;
        self.bell.step(now, shown)
    }

    /// Eases what is under the mouse in and the rest out. Returns true while
    /// anything still moves.
    fn step_zooms(&mut self, dt: f32) -> bool {
        if let Some(id) = self.hover_zoom {
            if !self.zooms.iter().any(|(known, _)| *known == id) {
                self.zooms.push((id, 0.0));
            }
        }
        let share = 1.0 - (-dt * ZOOM_SPEED).exp();
        let mut moving = false;
        for (id, zoom) in &mut self.zooms {
            let target = if self.hover_zoom == Some(*id) {
                1.0
            } else {
                0.0
            };
            *zoom += (target - *zoom) * share;
            if (target - *zoom).abs() < 0.01 {
                *zoom = target;
            } else {
                moving = true;
            }
        }
        self.zooms.retain(|(_, zoom)| *zoom > 0.0);
        moving
    }

    fn inbox_row(&self, x: f32, y: f32) -> Option<usize> {
        if self.shown_page() != Page::Notifications || self.notch.card_visible.value < 0.5 {
            return None;
        }
        let (width, count) = (self.notch.width.value, self.inbox.items.len());
        self.renderer.inbox_row_at(x, y, width, count)
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
    /// Hides the notch while another app is full screen, except while a banner
    /// (a reminder, a level) needs to be seen.
    fn step_aside_for_fullscreen(&mut self) {
        let banner =
            self.notch.banner_visible.target > 0.0 || self.notch.banner_visible.value > 0.01;
        let show = banner || !system::fullscreen_app(self.hwnd);
        if show != self.shown {
            self.shown = show;
            unsafe {
                let _ = ShowWindow(self.hwnd, if show { SW_SHOWNOACTIVATE } else { SW_HIDE });
            }
            if show {
                system::stay_on_top(self.hwnd);
            }
        }
    }

    /// Alt+N: opens the card, then moves through its pages, then closes it.
    fn on_hotkey(&mut self) {
        self.finish_editing(true, true);
        let now = Instant::now();
        let pages = self.pages();
        let current = pages
            .iter()
            .position(|page| *page == self.shown_page())
            .unwrap_or(0);
        if self.notch.card_visible.target == 0.0 {
            if self.unread {
                self.page = Page::Notifications;
            }
            self.card_until = Some(now + KEY_CARD);
        } else if current + 1 < pages.len() {
            self.page_switch = Some(PageSwitch {
                from: pages[current],
                started: now,
                direction: 1.0,
            });
            self.page = pages[current + 1];
            self.card_until = Some(now + KEY_CARD);
        } else {
            self.card_until = None;
        }
        self.tick();
    }

    fn on_wheel(&mut self, wparam: WPARAM) {
        if self.notch.card_visible.target == 0.0 {
            return;
        }
        let down = ((wparam.0 >> 16) as i16) < 0;
        self.step_page(down);
    }

    /// Moves the open card to the next page (`forward`) or the one before,
    /// wrapping around.
    fn step_page(&mut self, forward: bool) {
        self.finish_editing(true, true);
        let pages = self.pages();
        let current = pages
            .iter()
            .position(|page| *page == self.shown_page())
            .unwrap_or(0);
        let next = if forward {
            (current + 1) % pages.len()
        } else {
            (current + pages.len() - 1) % pages.len()
        };
        if pages[next] != pages[current] {
            self.page_switch = Some(PageSwitch {
                from: pages[current],
                started: Instant::now(),
                direction: if forward { 1.0 } else { -1.0 },
            });
        }
        self.page = pages[next];
        self.tick();
    }

    /// Whether the timer of `kind` has started and not been reset.
    fn timer_live(&self, kind: TimerKind) -> bool {
        match kind {
            TimerKind::Pomodoro => !self.pomodoro.idle(),
            TimerKind::Quick(_) => !self.quick.idle(),
            TimerKind::Stopwatch => !self.stopwatch.idle(),
        }
    }

    /// The timer the live strip shows: the one on the card when it has
    /// started, else the first that has.
    fn live_timer(&self) -> Option<TimerKind> {
        let all = [
            TimerKind::Pomodoro,
            TimerKind::Quick(self.quick.minutes()),
            TimerKind::Stopwatch,
        ];
        std::iter::once(self.timer_view)
            .chain(all)
            .find(|&kind| self.timer_live(kind))
    }

    /// The first download, or how many there are when several arrive.
    fn download_scene(&self) -> Option<DownloadScene> {
        let (name, bytes) = self.downloads.first()?;
        Some(DownloadScene {
            label: if self.downloads.len() > 1 {
                format!("{} unduhan", self.downloads.len())
            } else {
                name.clone()
            },
            size: downloads::size_label(*bytes),
        })
    }

    fn timer_scene(&self, kind: TimerKind) -> TimerScene {
        let now = Instant::now();
        let with_music = self.song_playing();
        // "Berjalan" or "Dijeda", once a timer has started.
        let state = |running: bool| if running { "Berjalan" } else { "Dijeda" };
        match kind {
            TimerKind::Pomodoro => TimerScene {
                kind,
                label: self.pomodoro.label(),
                left: calendar::format_left(self.pomodoro.left(now)),
                running: self.pomodoro.running(),
                on_break: self.pomodoro.phase == Phase::Break,
                detail: format!("Sesi {}", self.pomodoro.session),
                picking: false,
                with_music,
                progress: self.pomodoro.progress(now),
            },
            TimerKind::Quick(_) => TimerScene {
                kind: TimerKind::Quick(self.quick.minutes()),
                label: state(self.quick.running()),
                left: timers::format_clock(self.quick.left(now), true),
                running: self.quick.running(),
                on_break: false,
                detail: format!("{} menit", self.quick.minutes()),
                picking: self.quick.idle(),
                with_music,
                progress: self.quick.progress(now),
            },
            TimerKind::Stopwatch => TimerScene {
                kind,
                label: if self.stopwatch.idle() {
                    "Siap"
                } else {
                    state(self.stopwatch.running())
                },
                left: timers::format_clock(self.stopwatch.elapsed(now), false),
                running: self.stopwatch.running(),
                on_break: false,
                detail: "Hitung naik".to_string(),
                picking: false,
                with_music,
                progress: self.stopwatch.progress(now),
            },
        }
    }

    /// The card shows the lengths of the quick timer, to choose from.
    fn picking_minutes(&self) -> bool {
        matches!(self.timer_view, TimerKind::Quick(_)) && self.quick.idle()
    }

    /// Acts on a press of the timer card's controls.
    fn on_timer_control(&mut self, control: TimerControl) {
        let now = Instant::now();
        match control {
            TimerControl::StartPause => match self.timer_view {
                TimerKind::Pomodoro => self.pomodoro.toggle(now),
                TimerKind::Quick(_) => self.quick.toggle(now),
                TimerKind::Stopwatch => self.stopwatch.toggle(now),
            },
            TimerControl::Reset => match self.timer_view {
                TimerKind::Pomodoro => self.pomodoro.reset(),
                TimerKind::Quick(_) => self.quick.reset(),
                TimerKind::Stopwatch => self.stopwatch.reset(),
            },
            TimerControl::Tab(kind) => self.timer_view = kind,
            TimerControl::Minutes(minutes) => self.quick.set(minutes),
        }
    }

    fn on_quick_end(&mut self, minutes: u64) {
        let notice = Notice {
            icon: glyph::TIMER,
            motion: Motion::Shake,
            accent: Accent::Blue,
            progress: 1.0,
            title: "Timer selesai".to_string(),
            subtitle: format!("{minutes} menit sudah lewat"),
            trailing: Trailing::None,
            dim: false,
        };
        self.notify_for(notice, PRAYER_NOTICE_DURATION);
    }

    /// Announces the end of a Pomodoro phase.
    fn on_phase_end(&mut self, finished: Phase) {
        let notice = match finished {
            Phase::Focus => Notice {
                icon: glyph::TIMER,
                motion: Motion::Shake,
                accent: Accent::Green,
                progress: 1.0,
                title: "Waktunya break".to_string(),
                subtitle: format!("Sesi {} selesai · break dulu", self.pomodoro.session),
                trailing: Trailing::Text(calendar::format_left(self.pomodoro.left(Instant::now()))),
                dim: false,
            },
            Phase::Break => Notice {
                icon: glyph::TIMER,
                motion: Motion::Shake,
                accent: Accent::Teal,
                progress: 1.0,
                title: "Break selesai".to_string(),
                subtitle: "Lanjut sesi fokus".to_string(),
                trailing: Trailing::None,
                dim: false,
            },
        };
        self.notify_for(notice, PRAYER_NOTICE_DURATION);
    }

    /// Keeps lyrics that arrive for the song still playing.
    fn on_lyrics(&mut self, found: lyrics::Found) {
        let current = self
            .track
            .as_ref()
            .is_some_and(|track| track.artist == found.artist && track.title == found.title);
        if !current {
            return;
        }
        match found.lyrics {
            Some(lines) => {
                self.lyric_strip = self.renderer.lyric_strip_width(lines.lines());
                self.lyrics = Some((found.artist, found.title, lines));
            }
            None => self.lyrics_missing = true,
        }
        self.tick();
    }

    /// The lines lyric mode shows, or word of the search while there are none.
    fn shown_lyric(&self) -> Option<(&str, &str, &str)> {
        if !self.show_lyrics || !self.settings.module_lyrics {
            return None;
        }
        self.music()?;
        Some(self.lyric().unwrap_or(if self.lyrics_missing {
            ("", "Liriknya belum ada", "")
        } else {
            ("", "Memuat lirik\u{2026}", "")
        }))
    }

    /// Notes when a new line begins. Returns true while it rolls into place.
    fn update_lyric_roll(&mut self, now: Instant) -> bool {
        let line = self
            .lyrics
            .as_ref()
            .zip(self.music())
            .filter(|_| self.show_lyrics)
            .and_then(|((_, _, lines), track)| lines.current(track.position_now()));
        if line != self.lyric_line {
            self.lyric_line = line;
            self.lyric_moved = line.map(|_| now);
        }
        self.lyric_scroll(now) < 1.0
    }

    /// How far the line sung has rolled into place, 0..1.
    fn lyric_scroll(&self, now: Instant) -> f32 {
        self.lyric_moved.map_or(1.0, |moved| {
            (now.duration_since(moved).as_secs_f32() / LYRIC_ROLL.as_secs_f32()).min(1.0)
        })
    }

    /// The lines around the one being sung, when the song has lyrics.
    fn lyric(&self) -> Option<(&str, &str, &str)> {
        let track = self.music()?;
        let (artist, title, lines) = self.lyrics.as_ref()?;
        (self.settings.module_lyrics && track.artist == *artist && track.title == *title)
            .then(|| lines.at(track.position_now()))
    }

    fn on_events(&mut self, events: Vec<Event>) {
        self.events = events;
        // An event may already be close.
        self.update_agenda();
        self.draw();
    }

    /// The next events: all-day ones from today, timed ones that have not
    /// begun more than half an hour ago, for two weeks.
    fn upcoming(&self) -> Vec<&Event> {
        let today = today_number();
        let now = minutes_now();
        self.events
            .iter()
            .filter(|event| match (event.day - today, event.minute) {
                (0, Some(minute)) => minute + AGENDA_GRACE >= now,
                (days, _) => (0..=AGENDA_DAYS).contains(&days),
            })
            .take(anim::AGENDA_ROWS)
            .collect()
    }

    /// The shown month's first events: from now on in this month, so what is
    /// over makes way, and from the first day in any other.
    fn month_events(&self, month: (i32, u32)) -> Vec<&Event> {
        let after = (self.month_offset == 0)
            .then(|| (today_number(), minutes_now().saturating_sub(AGENDA_GRACE)));
        let mut events = ical::in_month(&self.events, month, after);
        events.truncate(anim::AGENDA_ROWS);
        events
    }

    fn agenda_scene<'a>(&self, events: Vec<&'a Event>) -> Vec<AgendaItem<'a>> {
        const DAYS: [&str; 7] = ["Sen", "Sel", "Rab", "Kam", "Jum", "Sab", "Min"];
        let today = today_number();
        events
            .into_iter()
            .map(|event| {
                let (year, month, date) = calendar::civil_from_days(event.day);
                AgendaItem {
                    day: match event.day - today {
                        0 => "Hari ini".to_string(),
                        1 => "Besok".to_string(),
                        _ => format!(
                            "{} {date}",
                            DAYS[calendar::weekday(year, month, date) as usize]
                        ),
                    },
                    time: event
                        .minute
                        .map_or("All day".to_string(), prayer::format_time),
                    summary: &event.summary,
                    today: event.day == today,
                    source: event.source,
                }
            })
            .collect()
    }

    /// Announces an event a few minutes before it starts.
    fn update_agenda(&mut self) {
        let today = today_number();
        let now = minutes_now();
        let due: Vec<(u32, String)> = self
            .events
            .iter()
            .filter(|event| event.day == today)
            .filter_map(|event| Some((event.minute?, event.summary.clone())))
            .filter(|(minute, _)| now + EVENT_LEAD >= *minute && now < *minute)
            .filter(|(minute, summary)| {
                !self
                    .event_reminded
                    .iter()
                    .any(|seen| *seen == (today, *minute, summary.clone()))
            })
            .collect();
        for (minute, summary) in due {
            self.event_reminded.push((today, minute, summary.clone()));
            self.notify(notices::event(&summary, minute - now, minute));
        }
        self.event_reminded.retain(|(day, ..)| *day == today);
    }

    fn on_weather(&mut self, weather: Weather) {
        self.weather = Some(weather);
        self.draw();
    }

    /// What the status page shows in place of the weather.
    fn weather_hint(&self) -> &'static str {
        if !self.settings.weather {
            "Cuaca off"
        } else if self.settings.prayer().is_none() {
            "Atur lokasi di Pengaturan"
        } else {
            "Loading cuaca…"
        }
    }

    /// Moves the level bars toward the current output peak. Returns true
    /// while they are on screen and moving.
    fn step_bars(&mut self, dt: f32) -> bool {
        let playing = self.track.as_ref().is_some_and(|track| track.playing);
        // On the strip, and in the card's header.
        let shown = self.notch.music_visible.value > 0.01 || self.notch.card_visible.value > 0.01;
        if !(shown && playing) {
            self.bars = [0.0; 4];
            return false;
        }
        self.bar_phase += dt;
        let peak = self.audio.as_ref().map_or(0.0, Audio::peak);
        // Measured against the song's own recent loudness, so beats stand out
        // in loud and quiet songs alike rather than holding the bars full.
        self.peak_average += (peak - self.peak_average) * BAR_AVERAGING;
        let level = if peak < BAR_SILENCE {
            0.0
        } else {
            (peak / self.peak_average.max(BAR_SILENCE) * BAR_GAIN).min(1.0)
        };
        for (i, bar) in self.bars.iter_mut().enumerate() {
            // Each bar sways at its own pace so they do not move in lockstep.
            let phase = self.bar_phase * BAR_SPEEDS[i] + i as f32;
            let wave = 0.6 * phase.sin() + 0.4 * (phase * 1.7 + i as f32).sin();
            let target = level * (0.6 + 0.4 * wave);
            // Quick up, slower down, like a level meter.
            *bar += (target - *bar) * if target > *bar { 0.85 } else { 0.45 };
        }
        true
    }

    fn on_hover(&mut self, inside: bool) {
        let now = Instant::now();
        self.nav_open &= inside;
        self.volume_open &= inside || self.volume_drag;
        self.card_clicked &= inside;
        if !inside {
            self.hover_zoom = None;
            self.banner_hover = false;
            self.draw();
        }
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
        if self.volume_drag {
            self.volume_drag = false;
            unsafe {
                let _ = ReleaseCapture();
            }
            return;
        }
        if let Some(notice) = self.notch.app_banner().cloned() {
            let width = self.notch.width.value;
            let command = self
                .renderer
                .peek_control_at(x, y, width, 1.0)
                .filter(|_| self.track.is_some());
            if let Some(command) = command {
                self.send_media(command);
                return;
            }
            if self.renderer.app_close_at(x, y, width) {
                notifications::remove(notice.id);
            } else {
                self.open_notice(&notice);
            }
            self.notch.end_banner(Instant::now());
            self.tick();
            return;
        }
        if self.settings.open_on_click && self.notch.card_visible.target == 0.0 {
            // The widened notch's controls work without opening the card.
            let command = (self.notch.peek_visible.value >= 0.5 && self.track.is_some())
                .then(|| {
                    self.renderer
                        .peek_control_at(x, y, self.notch.width.value, self.notch.scale)
                })
                .flatten();
            match command {
                Some(command) => self.send_media(command),
                None => {
                    self.card_clicked = true;
                    self.tick();
                }
            }
            return;
        }
        if self.notch.card_visible.value >= 0.5 && !self.drag_over {
            if let Some(index) = self.nav_item_at(x, y) {
                self.go_to(index);
                return;
            }
            if let Some(direction) = self.arrow_at(x, y) {
                self.step_page(direction > 0.0);
                return;
            }
        }
        if self.shown_page() == Page::Shelf {
            self.on_shelf_click(x, y);
            return;
        }
        if self.notch.card_visible.value < 0.5 {
            return;
        }
        if self.shown_page() == Page::Todo {
            self.on_todo_click(x, y);
            return;
        }
        if self.shown_page() == Page::Usage {
            let width = self.notch.width.value;
            if let Some(week) = self.renderer.usage_switch_at(x, y, width) {
                self.set_usage_week(week);
            }
            return;
        }
        if self.shown_page() == Page::Notifications {
            let width = self.notch.width.value;
            if self.inbox.total > 0 && self.renderer.inbox_clear_at(x, y, width) {
                notifications::clear();
                self.inbox = Inbox::default();
                self.renderer.sync_logos(&[]);
                self.tick();
            } else if let Some(index) =
                self.renderer
                    .inbox_remove_at(x, y, width, self.inbox.items.len())
            {
                // The worker's next list brings up the row below it.
                let notice = self.inbox.items.remove(index);
                self.inbox.total -= 1;
                notifications::remove(notice.id);
                self.inbox_hover = None;
                self.tick();
            } else if let Some(notice) = self
                .renderer
                .inbox_row_at(x, y, width, self.inbox.items.len())
                .and_then(|index| self.inbox.items.get(index).cloned())
            {
                self.open_notice(&notice);
            }
            return;
        }
        let width = self.notch.width.value;
        if self.shown_page() == Page::Calendar {
            match self.renderer.calendar_control_at(x, y, width) {
                Some(CalendarControl::PreviousMonth) => self.month_offset -= 1,
                Some(CalendarControl::NextMonth) => self.month_offset += 1,
                None => return,
            }
            self.tick();
            return;
        }
        if self.shown_page() == Page::Pomodoro {
            match self
                .renderer
                .timer_control_at(x, y, width, self.picking_minutes())
            {
                Some(control) => self.on_timer_control(control),
                None => return,
            }
            self.tick();
            return;
        }
        if self.shown_page() != Page::Music {
            return;
        }
        let lyrics = self.shown_lyric().is_some();
        if self.renderer.lyrics_button_at(x, y, width, lyrics) {
            if self.settings.module_lyrics {
                self.toggle_lyrics();
            }
            return;
        }
        if let Some(fraction) = self.renderer.progress_at(x, y, width, lyrics) {
            if let Some(duration) = self.music().map(|track| track.duration) {
                self.seek(duration * f64::from(fraction));
            }
            return;
        }
        if lyrics {
            let target = self.renderer.lyric_line_at(x, y, width).and_then(|step| {
                let (_, _, lines) = self.lyrics.as_ref()?;
                lines.line_start(self.music()?.position_now(), step)
            });
            if let Some(target) = target {
                self.seek(target);
                return;
            }
        }
        if let Some(command) = self.renderer.music_control_at(x, y, width, lyrics) {
            self.send_media(command);
        }
    }

    fn send_media(&mut self, command: Command) {
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

    /// Turns lyric mode on or off. A song whose lyrics were not found gets
    /// another search when it is turned on.
    fn toggle_lyrics(&mut self) {
        self.show_lyrics = !self.show_lyrics;
        if self.show_lyrics && self.lyrics_missing {
            self.lyrics_missing = false;
            if let Some(track) = &self.track {
                self.lyrics_watch
                    .request(&track.artist, &track.title, track.duration);
            }
        }
        self.tick();
    }

    /// Jumps within the song, showing the new place before the player says so.
    fn seek(&mut self, seconds: f64) {
        if let Some(media) = &self.media {
            media.send(Command::Seek(seconds));
        }
        if let Some(track) = self.track.as_mut() {
            track.position = seconds;
            track.read_at = Instant::now();
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

    /// Windows' Do Not Disturb is on and the settings follow it.
    fn quiet(&self) -> bool {
        self.settings.follow_dnd && system::do_not_disturb()
    }

    fn notify_for(&mut self, notice: Notice, duration: Duration) {
        if self.settings.sound && !self.quiet() {
            sound::play_notice();
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
        let before = self.settings.prayer();
        self.settings = Settings::load();
        let moved = self.settings.prayer() != before;
        dragdrop::set_accepting(self.settings.module_shelf);
        self.notch.bounce = self.settings.card_bounce;
        self.aim_usage_switch();
        let resting = match self.settings.notch_size {
            NotchSize::Small => COMPACT_SMALL,
            NotchSize::Normal => COMPACT,
            NotchSize::Large => COMPACT_LARGE,
        };
        self.notch.set_resting(resting, self.settings.notch_minimal);
        let place = self.settings.latitude.zip(self.settings.longitude);
        let weather_place = place.filter(|_| self.settings.weather);
        if weather_place.is_none() {
            self.weather = None;
        }
        self.weather_watch.set_location(weather_place);
        if self.settings.ical_urls.iter().all(String::is_empty) {
            self.events.clear();
        }
        self.ical_watch.set_feeds(self.settings.ical_urls.clone());
        // Lyrics just switched on: look up the song already playing.
        if let (true, None, Some(track)) = (
            self.settings.module_lyrics,
            &self.lyrics,
            self.track.as_ref(),
        ) {
            self.lyrics_watch
                .request(&track.artist, &track.title, track.duration);
        }
        if !self.idle_from_args {
            self.idle_after_ms = self.settings.idle_minutes * 60_000;
        }
        self.prayer = self
            .settings
            .prayer()
            .map(|settings| PrayerDay::new(settings, prayer::today()));
        if announce && moved && self.settings.notify_prayer {
            if let Some(day) = &self.prayer {
                let notice = notices::prayer_ready(day);
                self.notify(notice);
            }
        }
        self.update_prayer();
    }

    /// Follows the app in front for Screen Time. Time away from the computer
    /// does not count, unless something full screen or music explains the
    /// quiet mouse.
    fn update_usage(&mut self) {
        let now = Instant::now();
        if !self.settings.module_usage {
            self.usage.stop(now);
            return;
        }
        let idle = Duration::from_millis(u64::from(idle_ms()));
        let watching = !self.shown || self.track.as_ref().is_some_and(|track| track.playing);
        if idle >= usage::IDLE_LIMIT && !watching {
            self.usage.pause(idle, now);
        } else {
            let app = self.usage.in_front();
            self.usage.update(app, prayer::today(), now);
        }
        self.sync_usage_icons();
    }

    /// The apps Screen Time lists now.
    fn usage_apps(&self) -> &[usage::AppTime] {
        if self.settings.usage_week {
            self.usage.week()
        } else {
            self.usage.apps()
        }
    }

    fn sync_usage_icons(&mut self) {
        let paths: Vec<PathBuf> = self
            .usage_apps()
            .iter()
            .take(usage::SHOWN)
            .filter_map(|app| app.path.clone())
            .collect();
        self.renderer.sync_usage_icons(&paths);
    }

    fn aim_usage_switch(&mut self) {
        self.usage_switch.target = if self.settings.usage_week { 1.0 } else { 0.0 };
    }

    /// Shows the last seven days on Screen Time, or today.
    fn set_usage_week(&mut self, week: bool) {
        if week == self.settings.usage_week {
            return;
        }
        self.settings.usage_week = week;
        let _ = self.settings.save();
        self.config_stamp = config::modified();
        self.aim_usage_switch();
        self.sync_usage_icons();
        self.tick();
    }

    /// Counts a minute of use toward the health reminders, held back while
    /// the notch steps aside for a full-screen app.
    fn update_health(&mut self) {
        let plan = self.settings.health();
        let idle_minutes = idle_ms() / 60_000;
        if let Some(kind) = self.health.minute(&plan, idle_minutes, !self.shown) {
            let minutes = plan.interval(kind).unwrap_or_default();
            self.notify(notices::health(kind, minutes));
        }
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
        let title =
            |i: usize| format!("{} {}", prayer::NAMES[i], prayer::format_time(day.times[i]));
        let reminder = if let Some(i) = due {
            day.reminded[i] = true;
            let minutes = day.times[i] - now;
            Some((title(i), format!("{minutes} Menit lagi")))
        } else {
            let arrived = (0..prayer::NAMES.len()).find(|&i| {
                !day.arrived[i]
                    && now >= day.times[i]
                    && now < day.times[i] + PRAYER_ARRIVAL_MINUTES
            });
            arrived.map(|i| {
                day.arrived[i] = true;
                (title(i), "Waktunya sholat".to_string())
            })
        };
        if let Some((title, wait)) = reminder {
            self.notify_prayer(title, wait);
        }
    }

    /// Widens the notch for a moment with a prayer reminder.
    fn notify_prayer(&mut self, title: String, wait: String) {
        if self.settings.sound && !self.quiet() {
            sound::play_notice();
        }
        self.notch
            .show_prayer(title, wait, PRAYER_NOTICE_DURATION, Instant::now());
        // A reminder replacing another moves nothing, so nothing would redraw.
        self.draw();
        self.schedule(FRAME_MS);
    }

    /// The prayer close at hand and the wait for it, for the closed notch.
    fn prayer_soon(&self) -> Option<(&'static str, String)> {
        let day = self
            .prayer
            .as_ref()
            .filter(|_| self.settings.module_prayer)?;
        let (next, wait) = prayer::soon(&day.times, minutes_now())?;
        Some((prayer::NAMES[next], prayer::format_wait(wait)))
    }

    /// The prayer page: the next prayer (tomorrow's Subuh after Isya), how
    /// long until it, today's times, and the sun's place among them.
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
        Some(PrayerScene {
            next_name: prayer::NAMES[next],
            next_time: prayer::format_time(time % (24 * 60)),
            wait: prayer::format_wait(time.saturating_sub(now)),
            times: day.times.map(prayer::format_time),
            next_today,
            progress: prayer::wait_progress(&day.times, now, time),
            sun: prayer::day_position(&day.times, now),
        })
    }

    /// Switches the thin bar on or off from the tray.
    fn toggle_minimal(&mut self) {
        self.settings.notch_minimal = !self.settings.notch_minimal;
        let _ = self.settings.save();
        self.config_stamp = config::modified();
        self.apply_settings(false);
        self.tick();
    }

    fn on_volume(&mut self, wparam: WPARAM, lparam: LPARAM) {
        // The music card has its own slider.
        if self.notch.card_visible.target > 0.0 && self.shown_page() == Page::Music {
            self.draw();
            return;
        }
        self.show_level(LevelKind::Volume, wparam.0 as f32 / 1000.0, lparam.0 != 0);
    }

    /// The camera or microphone started or stopped being used.
    fn on_privacy(&mut self, state: usize) {
        self.privacy = state;
        self.tick();
    }

    /// The microphone was muted or unmuted; a change is announced.
    fn on_mic(&mut self, muted: bool) {
        let before = self.mic_muted.replace(muted);
        if before.is_some_and(|before| before != muted) && self.settings.notify_mic {
            self.notify(notices::mic(muted));
        }
    }

    /// The Downloads folder changed: files still arriving, and those that
    /// have just landed.
    fn on_downloads(&mut self, update: downloads::Update) {
        self.downloads = update.active;
        for name in update.done {
            self.notify(notices::download(&name));
        }
        self.tick();
        self.draw();
    }

    /// Caps Lock was released, and has flipped.
    fn on_caps(&mut self) {
        let on = system::caps_lock_on();
        // Nothing to suppress: Windows draws no flyout of its own for this.
        self.notch.show_level(
            LevelKind::Caps { on },
            if on { 1.0 } else { 0.0 },
            false,
            Instant::now(),
        );
        self.tick();
        self.draw();
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
            PowerEvent::OnAc(on_ac) => {
                let changed = self.on_ac.replace(on_ac).is_some_and(|was| was != on_ac);
                if let Some(percent) = self.battery.filter(|_| changed) {
                    if self.settings.notify_battery {
                        self.notify(notices::charger(on_ac, percent));
                    }
                }
            }
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
            self.notch.fill_notice_meter();
            return;
        };
        let notice = notices::bluetooth(event.name, true, event.audio, Some(percent));
        let width = self.renderer.notice_width(&notice);
        self.notch.refresh_notice(notice, width, Instant::now());
    }

    fn draw(&self) {
        let n = &self.notch;
        let date = date_text();
        let scene = Scene {
            width: n.width.value,
            height: n.height.value,
            eyes_visible: n.eyes_visible.value,
            blink: n.eyes.blink,
            look: n.eyes.look.value,
            sleep: n.eyes.sleep.value,
            surprise: n.eyes.surprise.value,
            eyes_z: n.eyes.z,
            hop: n.eyes.hop,
            greeting: n.eyes.greeting,
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
                lyric: self.shown_lyric(),
                lyric_scroll: self.lyric_scroll(Instant::now()),
                lyrics_ready: self.settings.module_lyrics,
                lyrics_on: self.show_lyrics,
                volume_visible: n.volume_visible.value,
                volume: self.audio.as_ref().and_then(Audio::volume).unwrap_or(0.0),
            }),
            music_visible: n.music_visible.value,
            card_visible: n.card_visible.value,
            page: self.shown_page(),
            page_index: self
                .pages()
                .iter()
                .position(|page| *page == self.shown_page())
                .unwrap_or(0),
            page_list: self.pages(),
            nav_visible: n.nav_visible.value,
            zooms: &self.zooms,
            banner_hover: self.banner_hover,
            privacy: if self.settings.privacy_dots {
                self.privacy
            } else {
                0
            },
            peek_visible: n.peek_visible.value,
            closed_scale: n.scale,
            calendar: {
                let (year, month, day) = prayer::today();
                let (shown_year, shown_month) =
                    calendar::shift(year as i32, month as u32, self.month_offset);
                CalendarScene {
                    year: shown_year,
                    month: shown_month,
                    today: (self.month_offset == 0).then_some(day as u32),
                    marked: self
                        .events
                        .iter()
                        .map(|event| (calendar::civil_from_days(event.day), event.source))
                        .filter(|&((y, m, _), _)| (y, m) == (shown_year, shown_month))
                        .map(|((_, _, d), source)| (d, source))
                        .collect(),
                }
            },
            timer: Some(self.timer_scene(if self.shown_page() == Page::Pomodoro {
                self.timer_view
            } else {
                self.live_timer().unwrap_or(self.timer_view)
            })),
            download: self.download_scene(),
            strip: self.strip(),
            prayer: self.prayer_scene(),
            prayer_soon: self.prayer_soon(),
            notice_meter: n.notice_meter.value,
            notice_beat: n.notice_beat,
            notch_opacity: self.settings.notch_opacity(),
            floating: self.settings.notch_floating,
            arrows: self.settings.card_arrows,
            bell_angle: self.bell.angle,
            minimal: self.settings.notch_minimal,
            place: &self.settings.city,
            shelf: ShelfScene {
                items: self.shelf.items(),
                hover: self.shelf_hover,
                copied: self
                    .shelf_copied
                    .filter(|(_, at)| at.elapsed() < COPIED_SHOWN)
                    .map(|(index, _)| index),
                selected: &self.shelf_picked,
            },
            inbox: self.inbox_scene(),
            todo: self.todo_scene(),
            usage: UsageScene {
                apps: self.usage_apps(),
                week: self.settings.usage_week,
                switch: self.usage_switch.value,
                average_ms: self.usage.daily_average_ms(),
            },
            agenda: self.agenda_scene(self.upcoming()),
            month_agenda: {
                let (year, month, _) = prayer::today();
                let shown = calendar::shift(year as i32, month as u32, self.month_offset);
                self.agenda_scene(self.month_events(shown))
            },
            drop_zone: self.drag_over,
            liquid_glow: n.liquid_glow.value,
            ripple_at: n.ripple_at,
            ripples: n.ripple_rings(),
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
                date: &date,
                weather: self.weather.as_ref().map(|weather| WeatherScene {
                    glyph: weather.glyph(),
                    temperature: format!("{}°", weather.temperature.round()),
                    description: weather.description(),
                    place: &self.settings.city,
                }),
                weather_hint: self.weather_hint(),
                // A cheap call, and drawing happens only on changes.
                battery: system::battery_status(),
            },
        };
        let _ = self.renderer.present(self.hwnd, self.origin(), &scene);
    }

    /// Top-center of the primary monitor.
    fn origin(&self) -> POINT {
        let screen_w = unsafe { GetSystemMetrics(SM_CXSCREEN) };
        POINT {
            x: (screen_w - self.renderer.size().cx) / 2,
            y: if self.settings.notch_floating {
                (FLOAT_GAP * window_scale(self.hwnd)).round() as i32
            } else {
                0
            },
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
            self.sync_usage_icons();
            self.renderer.sync_logos(&self.inbox.items);
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
        WM_CLIPBOARDUPDATE => {
            with_app(|app| app.on_clipboard());
            LRESULT(0)
        }
        clipboard::CLIP_MSG => {
            let path = clipboard::take_path(lparam);
            with_app(|app| app.on_clip(path));
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
        WM_HOTKEY => {
            with_app(App::on_hotkey);
            LRESULT(0)
        }
        WM_CHAR => {
            with_app(|app| app.on_char(wparam.0 as u16));
            LRESULT(0)
        }
        // Clicked elsewhere while typing: keep what was typed.
        WM_KILLFOCUS => {
            with_app(|app| {
                app.finish_editing(true, false);
                app.tick();
            });
            LRESULT(0)
        }
        WM_MOUSEWHEEL => {
            with_app(|app| app.on_wheel(wparam));
            LRESULT(0)
        }
        system::FOREGROUND_MSG => {
            system::stay_on_top(hwnd);
            with_app(App::update_usage);
            LRESULT(0)
        }
        screenshots::SCREENSHOT_MSG => {
            let path = screenshots::take_path(lparam);
            with_app(|app| app.on_screenshot(path));
            LRESULT(0)
        }
        downloads::DOWNLOAD_MSG => {
            let update = downloads::take_update(lparam);
            with_app(|app| app.on_downloads(update));
            LRESULT(0)
        }
        notifications::INBOX_MSG => {
            let inbox = notifications::take_inbox(lparam);
            with_app(|app| app.on_inbox(inbox));
            LRESULT(0)
        }
        lyrics::LYRICS_MSG => {
            let found = lyrics::take_found(lparam);
            with_app(|app| app.on_lyrics(found));
            LRESULT(0)
        }
        ical::EVENTS_MSG => {
            let events = ical::take_events(lparam);
            with_app(|app| app.on_events(events));
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
        keys::CAPS_MSG => {
            with_app(App::on_caps);
            LRESULT(0)
        }
        settings::SAVED_MSG => {
            with_app(|app| {
                app.update_prayer();
                app.draw();
            });
            LRESULT(0)
        }
        system::DEVICE_MSG => {
            with_app(|app| {
                if let Some(audio) = app.audio.as_mut() {
                    audio.rebind();
                }
                // A new microphone's state is a starting value, not a change.
                app.mic_muted = app.audio.as_ref().and_then(Audio::mic_muted);
            });
            LRESULT(0)
        }
        system::MIC_MSG => {
            with_app(|app| app.on_mic(wparam.0 != 0));
            LRESULT(0)
        }
        privacy::PRIVACY_MSG => {
            with_app(|app| app.on_privacy(wparam.0));
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
        WM_ENDSESSION => {
            if wparam.0 != 0 {
                with_app(|app| app.usage.stop(Instant::now()));
            }
            LRESULT(0)
        }
        WM_DESTROY => {
            with_app(|app| app.usage.stop(Instant::now()));
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
        MENU_MINIMAL => {
            with_app(App::toggle_minimal);
        }
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

        let minimal = APP.with(|cell| {
            cell.try_borrow()
                .ok()
                .and_then(|app| app.as_ref().map(|app| app.settings.notch_minimal))
                .unwrap_or_default()
        });
        let minimal_flags = if minimal {
            MF_STRING | MF_CHECKED
        } else {
            MF_STRING
        };
        let _ = AppendMenuW(menu, minimal_flags, MENU_MINIMAL, w!("Notch minimal"));
        let autostart_flags = if autostart::is_enabled() {
            MF_STRING | MF_CHECKED
        } else {
            MF_STRING
        };
        let _ = AppendMenuW(menu, autostart_flags, MENU_AUTOSTART, w!("Mulai otomatis"));
        let _ = AppendMenuW(menu, MF_STRING, MENU_SETTINGS, w!("Settings…"));
        let _ = AppendMenuW(menu, MF_SEPARATOR, 0, None);
        let _ = AppendMenuW(menu, MF_STRING, MENU_QUIT, w!("Keluar"));
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

/// Today as days since 1970-01-01.
fn today_number() -> i64 {
    let (year, month, day) = prayer::today();
    calendar::days_from_civil(year as i32, month as u32, day as u32)
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

/// The notice cards Drop shows.
mod notices {
    use crate::anim::{Accent, Motion, Notice, Trailing};
    use crate::health::Kind;
    use crate::prayer::{format_time, NAMES};
    use crate::render::glyph;
    use crate::{minutes_now, PrayerDay};

    /// `copied` says the picture is on the clipboard, so a paste is enough.
    pub fn screenshot(copied: bool) -> Notice {
        Notice {
            icon: glyph::CAMERA,
            motion: Motion::Flash,
            accent: Accent::Teal,
            progress: 1.0,
            title: "Screenshot masuk Shelf".to_string(),
            subtitle: if copied {
                "Siap paste pakai Ctrl+V".to_string()
            } else {
                "Copy dari Shelf".to_string()
            },
            trailing: Trailing::None,
            dim: false,
        }
    }

    /// A file that was downloading has landed in the Downloads folder.
    pub fn download(name: &str) -> Notice {
        Notice {
            icon: glyph::DOWNLOAD,
            motion: Motion::Bob,
            accent: Accent::Blue,
            progress: 1.0,
            title: "Unduhan selesai".to_string(),
            subtitle: name.to_string(),
            trailing: Trailing::None,
            dim: false,
        }
    }

    pub fn wifi(ssid: &str, connected: bool) -> Notice {
        Notice {
            icon: glyph::WIFI,
            motion: Motion::Pulse,
            accent: Accent::Teal,
            progress: 1.0,
            title: if connected {
                ssid.to_string()
            } else {
                "Wi-Fi disconnected".to_string()
            },
            subtitle: if connected {
                "Wi-Fi connected".to_string()
            } else {
                ssid.to_string()
            },
            trailing: Trailing::None,
            dim: !connected,
        }
    }

    /// `battery` fills the ring and joins the subtitle once the device reports it.
    pub fn bluetooth(name: String, connected: bool, audio: bool, battery: Option<u32>) -> Notice {
        let state = if connected {
            "Connected"
        } else {
            "Disconnected"
        };
        Notice {
            icon: glyph::BLUETOOTH,
            motion: Motion::Pulse,
            accent: Accent::Blue,
            progress: battery.map_or(0.0, |percent| percent as f32 / 100.0),
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

    /// The microphone was muted or unmuted.
    pub fn mic(muted: bool) -> Notice {
        Notice {
            icon: if muted { glyph::MIC_OFF } else { glyph::MIC },
            motion: Motion::Pulse,
            accent: if muted { Accent::Red } else { Accent::Green },
            progress: 1.0,
            title: if muted {
                "Mic dimatikan"
            } else {
                "Mic menyala"
            }
            .to_string(),
            subtitle: if muted {
                "Suaramu tidak terdengar"
            } else {
                "Suaramu terdengar lagi"
            }
            .to_string(),
            trailing: Trailing::None,
            dim: muted,
        }
    }

    /// The charger was plugged in or out, with the charge on its ring.
    pub fn charger(plugged: bool, percent: u32) -> Notice {
        Notice {
            icon: glyph::BOLT,
            motion: Motion::Flash,
            accent: Accent::Green,
            progress: percent as f32 / 100.0,
            title: if plugged {
                "Mengisi daya"
            } else {
                "Charger dilepas"
            }
            .to_string(),
            subtitle: format!("Battery {percent}%"),
            trailing: Trailing::Icon(glyph::battery(percent)),
            dim: !plugged,
        }
    }

    pub fn low_battery(percent: u32) -> Notice {
        Notice {
            icon: glyph::battery(percent),
            motion: Motion::Flash,
            accent: Accent::Red,
            progress: percent as f32 / 100.0,
            title: "Battery low".to_string(),
            subtitle: "Colok charger dulu".to_string(),
            trailing: Trailing::Text(format!("{percent}%")),
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
            motion: Motion::Pulse,
            accent: Accent::Green,
            progress: 1.0,
            title: "Jadwal sholat aktif".to_string(),
            subtitle: format!("{city} · berikutnya {}", NAMES[next]),
            trailing: Trailing::Text(format_time(day.times[next])),
            dim: false,
        }
    }

    /// The event's title, how soon it starts, and its time on the right.
    pub fn event(summary: &str, minutes_left: u32, start: u32) -> Notice {
        Notice {
            icon: glyph::CALENDAR,
            motion: Motion::Shake,
            accent: Accent::Blue,
            progress: 1.0,
            title: summary.to_string(),
            subtitle: format!("Agenda \u{b7} {minutes_left} menit lagi"),
            trailing: Trailing::Text(format_time(start)),
            dim: false,
        }
    }

    /// A health reminder after `minutes` of use.
    pub fn health(kind: Kind, minutes: u32) -> Notice {
        let (icon, accent, title, subtitle) = match kind {
            Kind::Water => (
                glyph::WATER,
                Accent::Blue,
                "Waktunya minum",
                format!("{minutes} menit di depan layar"),
            ),
            Kind::Eyes => (
                glyph::EYE,
                Accent::Teal,
                "Istirahat mata",
                "Lihat sejauh 6 meter, 20 detik".to_string(),
            ),
            Kind::Stretch => (
                glyph::WALK,
                Accent::Green,
                "Waktunya stretching",
                "Berdiri, gerakin badan sebentar".to_string(),
            ),
        };
        Notice {
            icon,
            motion: match kind {
                Kind::Water => Motion::Bob,
                Kind::Eyes => Motion::Blink,
                Kind::Stretch => Motion::Walk,
            },
            accent,
            progress: 1.0,
            title: title.to_string(),
            subtitle,
            trailing: Trailing::None,
            dim: false,
        }
    }

    /// `--demo` cycles through sample notices to preview their look.
    pub fn demo(index: usize) -> Notice {
        match index % 12 {
            10 => charger(true, 62),
            11 => charger(false, 62),
            9 => health(Kind::Stretch, 50),
            8 => health(Kind::Eyes, 20),
            7 => health(Kind::Water, 60),
            6 => event("Rapat tim", 10, 14 * 60),
            0 => wifi("Rumah 5G", true),
            1 => bluetooth("Galaxy Buds2 Pro".to_string(), true, true, Some(99)),
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
