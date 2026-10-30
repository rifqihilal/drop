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
mod config;
mod geo;
mod keys;
mod osd;
mod prayer;
mod render;
mod settings;
mod system;
mod tray;
mod wifi;

use std::cell::RefCell;
use std::time::{Duration, Instant, SystemTime};

use windows::core::{w, Result, HSTRING};
use windows::Win32::Foundation::{
    GetLastError, ERROR_ALREADY_EXISTS, HWND, LPARAM, LRESULT, POINT, WPARAM,
};
use windows::Win32::Media::Audio::{PlaySoundW, SND_ALIAS, SND_ASYNC, SND_NODEFAULT};
use windows::Win32::System::Com::{CoInitializeEx, COINIT_APARTMENTTHREADED};
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::System::SystemInformation::{GetLocalTime, GetTickCount, GetTickCount64};
use windows::Win32::System::Threading::CreateMutexW;
use windows::Win32::UI::HiDpi::{
    GetDpiForWindow, SetProcessDpiAwarenessContext, DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2,
};
use windows::Win32::UI::Input::KeyboardAndMouse::{GetLastInputInfo, LASTINPUTINFO};
use windows::Win32::UI::WindowsAndMessaging::{
    AppendMenuW, CreatePopupMenu, CreateWindowExW, DefWindowProcW, DestroyMenu, DestroyWindow,
    DispatchMessageW, GetCursorPos, GetMessageW, GetSystemMetrics, PostMessageW, PostQuitMessage,
    RegisterClassExW, SetForegroundWindow, SetTimer, ShowWindow, TrackPopupMenu, TranslateMessage,
    MF_CHECKED, MF_GRAYED, MF_POPUP, MF_SEPARATOR, MF_STRING, MSG, SM_CXSCREEN, SW_SHOWNOACTIVATE,
    TPM_RETURNCMD, TPM_RIGHTBUTTON, WM_DESTROY, WM_DEVICECHANGE, WM_DISPLAYCHANGE, WM_DPICHANGED,
    WM_LBUTTONUP, WM_NULL, WM_POWERBROADCAST, WM_RBUTTONUP, WM_SETTINGCHANGE, WM_TIMER,
    WNDCLASSEXW, WS_EX_LAYERED, WS_EX_NOACTIVATE, WS_EX_TOOLWINDOW, WS_EX_TOPMOST, WS_POPUP,
};

use anim::{LevelKind, Notch, Notice};
use bluetooth::{Bluetooth, DeviceEvent};
use config::{PrayerSettings, Settings};
use render::{Renderer, Scene};
use system::{Audio, PowerEvent};
use wifi::Wifi;

const TIMER_ID: usize = 1;
/// Frame interval while something is animating (~60 fps).
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
        CoInitializeEx(None, COINIT_APARTMENTTHREADED).ok()?;

        let instance = GetModuleHandleW(None)?;
        let class = WNDCLASSEXW {
            cbSize: size_of::<WNDCLASSEXW>() as u32,
            lpfnWndProc: Some(wndproc),
            hInstance: instance.into(),
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
        };
        config::ensure_exists();
        // Load quietly: the "schedule ready" notice is for later edits only.
        app.config_stamp = config::modified();
        app.apply_settings(false);
        app.draw();
        let _ = ShowWindow(hwnd, SW_SHOWNOACTIVATE);
        tray::add(hwnd);
        osd::set_suppressed(true);
        keys::install(hwnd);
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

        let moving = self.notch.step(now, dt);

        if moving || clock_changed {
            self.draw();
        }
        let waiting =
            self.wifi_lost.is_some() || self.battery_check.is_some() || self.demo.is_some();
        self.schedule(if moving {
            FRAME_MS
        } else if self.notch.busy() || waiting {
            BUSY_MS
        } else {
            COMPACT_MS
        });
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
        let scene = Scene {
            width: n.width.value,
            height: n.height.value,
            eyes_visible: n.eyes_visible.value,
            blink: n.eyes.blink,
            look: n.eyes.look.value,
            clock: &self.clock,
            banner_visible: n.banner_visible.value,
            banner: n.banner.as_ref().map(|banner| &banner.content),
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

    fn schedule(&mut self, ms: u32) {
        if ms != self.timer_ms {
            self.timer_ms = ms;
            unsafe { SetTimer(Some(self.hwnd), TIMER_ID, ms, None) };
        }
    }

    /// Rebuilds the surface after a DPI or resolution change.
    fn relayout(&mut self) {
        if let Ok(renderer) = Renderer::new(window_scale(self.hwnd)) {
            self.renderer = renderer;
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
        WM_DPICHANGED | WM_DISPLAYCHANGE => {
            with_app(App::relayout);
            LRESULT(0)
        }
        WM_RBUTTONUP => {
            handle_menu(hwnd);
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
fn with_app(f: impl FnOnce(&mut App)) {
    APP.with(|cell| {
        if let Ok(mut app) = cell.try_borrow_mut() {
            if let Some(app) = app.as_mut() {
                f(app);
            }
        }
    });
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

fn current_clock() -> String {
    let time = unsafe { GetLocalTime() };
    format!("{:02}.{:02}", time.wHour, time.wMinute)
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
