//! Screen time per app for today, kept in `%APPDATA%\Drop\screen_time.txt`:
//! the date on the first line, then one app per line as `seconds name`.
//!
//! Event-driven: the foreground hook in `system.rs` reports each switch, and
//! the notch's minute tick settles the time and notices idling. Yesterday's
//! numbers are overwritten, and nothing leaves this computer.

use std::collections::HashMap;
use std::ffi::c_void;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use windows::core::{w, BOOL, HSTRING, PWSTR};
use windows::Win32::Foundation::{CloseHandle, HWND, LPARAM};
use windows::Win32::Storage::FileSystem::{
    GetFileVersionInfoSizeW, GetFileVersionInfoW, VerQueryValueW,
};
use windows::Win32::System::Threading::{
    GetCurrentProcessId, OpenProcess, QueryFullProcessImageNameW, PROCESS_NAME_WIN32,
    PROCESS_QUERY_LIMITED_INFORMATION,
};
use windows::Win32::UI::WindowsAndMessaging::{
    EnumChildWindows, GetForegroundWindow, GetWindowThreadProcessId,
};

/// Apps listed on the card.
pub const SHOWN: usize = 4;
/// This long without input counts as being away.
pub const IDLE_LIMIT: Duration = Duration::from_secs(5 * 60);
/// A longer gap between two updates means the computer slept: updates come
/// at least every minute, so that time was not spent in the app.
const MAX_GAP: Duration = Duration::from_secs(2 * 60);
/// Time taken back from an app on going idle, at most: the limit, plus the
/// minute it can take to notice.
const MAX_FORGOTTEN: Duration = Duration::from_secs(6 * 60);
const SAVE_EVERY: Duration = Duration::from_secs(5 * 60);
/// While these are in front the screen is locked: no app is in use.
const LOCK_SCREEN: [&str; 2] = ["LockApp.exe", "LogonUI.exe"];
/// Parts of Windows that open over an app; its time keeps running.
const PASSING: [&str; 4] = [
    "SearchHost.exe",
    "ShellExperienceHost.exe",
    "StartMenuExperienceHost.exe",
    "TextInputHost.exe",
];
/// Store apps run inside this frame, each in a child window of its own.
const FRAME_HOST: &str = "ApplicationFrameHost.exe";

type Day = (u16, u16, u16);

#[derive(Clone, Debug, PartialEq)]
pub struct AppTime {
    pub name: String,
    pub ms: u64,
}

/// The app being timed.
struct Open {
    name: String,
    since: Instant,
}

pub struct Usage {
    day: Day,
    /// Longest first.
    apps: Vec<AppTime>,
    open: Option<Open>,
    /// Display names by file, so each is read once.
    names: HashMap<PathBuf, String>,
    unsaved: bool,
    saved_at: Instant,
}

impl Usage {
    pub fn load(today: Day, now: Instant) -> Self {
        let text = file()
            .and_then(|file| std::fs::read_to_string(file).ok())
            .unwrap_or_default();
        Self {
            day: today,
            apps: parse(&text, today),
            open: None,
            names: HashMap::new(),
            unsaved: false,
            saved_at: now,
        }
    }

    pub fn apps(&self) -> &[AppTime] {
        &self.apps
    }

    /// Everything counted today, listed or not.
    pub fn total_ms(&self) -> u64 {
        self.apps.iter().map(|app| app.ms).sum()
    }

    /// Follows the app in front; None stops the clock.
    pub fn update(&mut self, app: Option<String>, today: Day, now: Instant) {
        self.settle(now);
        if today != self.day {
            self.day = today;
            self.apps.clear();
            self.unsaved = true;
        }
        match (&self.open, app) {
            (Some(open), Some(app)) if open.name == app => {}
            (_, Some(name)) => self.open = Some(Open { name, since: now }),
            (_, None) => self.open = None,
        }
        if self.unsaved && now.duration_since(self.saved_at) >= SAVE_EVERY {
            self.save(now);
        }
    }

    /// Stops the clock after `idle` without input, taking that time back from
    /// the app that was in front.
    pub fn pause(&mut self, idle: Duration, now: Instant) {
        if self.settle(now) {
            if let Some(open) = self.open.as_ref().map(|open| open.name.clone()) {
                self.forget(&open, idle.min(MAX_FORGOTTEN));
            }
        }
        self.open = None;
    }

    /// Settles the running time and writes everything down.
    pub fn stop(&mut self, now: Instant) {
        self.settle(now);
        self.open = None;
        if self.unsaved {
            self.save(now);
        }
    }

    /// The app in front now. Windows passing over an app, and Drop itself,
    /// leave the app before them running.
    pub fn in_front(&mut self) -> Option<String> {
        let path = unsafe { front_path() }?;
        let file = path.file_name()?.to_string_lossy();
        if LOCK_SCREEN
            .iter()
            .any(|name| file.eq_ignore_ascii_case(name))
        {
            return None;
        }
        let own = std::env::current_exe().is_ok_and(|own| own == path);
        if own || PASSING.iter().any(|name| file.eq_ignore_ascii_case(name)) {
            return self.open.as_ref().map(|open| open.name.clone());
        }
        let name = self
            .names
            .entry(path)
            .or_insert_with_key(|path| display_name(path));
        Some(name.clone())
    }

    /// Adds the time since the last settling to the open app. Returns false
    /// when there was none, or when the computer slept through it.
    fn settle(&mut self, now: Instant) -> bool {
        let Some(open) = &mut self.open else {
            return false;
        };
        let spent = now.duration_since(open.since);
        open.since = now;
        if spent >= MAX_GAP {
            return false;
        }
        let name = open.name.clone();
        self.add(&name, spent.as_millis() as u64);
        true
    }

    fn add(&mut self, name: &str, ms: u64) {
        match self.apps.iter_mut().find(|app| app.name == name) {
            Some(app) => app.ms += ms,
            None => self.apps.push(AppTime {
                name: name.to_string(),
                ms,
            }),
        }
        self.apps.sort_by_key(|app| std::cmp::Reverse(app.ms));
        self.unsaved = true;
    }

    fn forget(&mut self, name: &str, time: Duration) {
        if let Some(app) = self.apps.iter_mut().find(|app| app.name == name) {
            app.ms = app.ms.saturating_sub(time.as_millis() as u64);
        }
        self.apps.retain(|app| app.ms > 0);
        self.apps.sort_by_key(|app| std::cmp::Reverse(app.ms));
        self.unsaved = true;
    }

    fn save(&mut self, now: Instant) {
        self.unsaved = false;
        self.saved_at = now;
        let Some(file) = file() else {
            return;
        };
        if let Some(dir) = file.parent() {
            let _ = std::fs::create_dir_all(dir);
        }
        let _ = std::fs::write(file, format(self.day, &self.apps));
    }
}

/// "2j 15m", "45m", or "<1m" under a minute.
pub fn format_time(ms: u64) -> String {
    let minutes = ms / 60_000;
    match (minutes / 60, minutes % 60) {
        (0, 0) => "<1m".to_string(),
        (0, minutes) => format!("{minutes}m"),
        (hours, 0) => format!("{hours}j"),
        (hours, minutes) => format!("{hours}j {minutes}m"),
    }
}

fn file() -> Option<PathBuf> {
    let appdata = std::env::var_os("APPDATA")?;
    Some(PathBuf::from(appdata).join("Drop").join("screen_time.txt"))
}

fn date_line((year, month, day): Day) -> String {
    format!("{year:04}-{month:02}-{day:02}")
}

/// The apps in `text`, if it was written `today`.
fn parse(text: &str, today: Day) -> Vec<AppTime> {
    let mut lines = text.trim_start_matches('\u{feff}').lines();
    if lines.next().map(str::trim) != Some(date_line(today).as_str()) {
        return Vec::new();
    }
    let mut apps: Vec<AppTime> = lines
        .filter_map(|line| {
            let (seconds, name) = line.trim().split_once(' ')?;
            let ms = seconds.parse::<u64>().ok()?.checked_mul(1000)?;
            Some(AppTime {
                name: name.trim().to_string(),
                ms,
            })
        })
        .filter(|app| !app.name.is_empty())
        .collect();
    apps.sort_by_key(|app| std::cmp::Reverse(app.ms));
    apps
}

fn format(day: Day, apps: &[AppTime]) -> String {
    let mut text = date_line(day);
    text.push('\n');
    for app in apps {
        text.push_str(&format!("{} {}\n", app.ms / 1000, app.name));
    }
    text
}

/// The file of the program that owns the foreground window.
unsafe fn front_path() -> Option<PathBuf> {
    let window = GetForegroundWindow();
    if window.is_invalid() {
        return None;
    }
    let mut pid = 0;
    GetWindowThreadProcessId(window, Some(&mut pid));
    let path = process_path(pid)?;
    let framed = path
        .file_name()
        .is_some_and(|file| file.eq_ignore_ascii_case(FRAME_HOST));
    if framed {
        if let Some(hosted) = hosted_process(window, pid) {
            return process_path(hosted);
        }
    }
    Some(path)
}

unsafe fn process_path(pid: u32) -> Option<PathBuf> {
    if pid == 0 || pid == GetCurrentProcessId() {
        return std::env::current_exe().ok().filter(|_| pid != 0);
    }
    let process = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, false, pid).ok()?;
    let mut buffer = [0u16; 1024];
    let mut length = buffer.len() as u32;
    let read = QueryFullProcessImageNameW(
        process,
        PROCESS_NAME_WIN32,
        PWSTR(buffer.as_mut_ptr()),
        &mut length,
    );
    let _ = CloseHandle(process);
    read.ok()?;
    Some(PathBuf::from(String::from_utf16_lossy(
        &buffer[..length as usize],
    )))
}

/// A process with a child window in `frame`, other than the frame's own.
unsafe fn hosted_process(frame: HWND, frame_pid: u32) -> Option<u32> {
    let mut found = (frame_pid, None::<u32>);
    let _ = EnumChildWindows(
        Some(frame),
        Some(child_window),
        LPARAM((&raw mut found) as isize),
    );
    found.1
}

unsafe extern "system" fn child_window(window: HWND, data: LPARAM) -> BOOL {
    let found = &mut *(data.0 as *mut (u32, Option<u32>));
    let mut pid = 0;
    GetWindowThreadProcessId(window, Some(&mut pid));
    if pid != 0 && pid != found.0 {
        found.1 = Some(pid);
        return false.into();
    }
    true.into()
}

/// The name an app gives itself, or its file name without the extension.
fn display_name(path: &Path) -> String {
    unsafe { file_description(path) }.unwrap_or_else(|| {
        path.file_stem()
            .map(|stem| stem.to_string_lossy().into_owned())
            .unwrap_or_default()
    })
}

unsafe fn file_description(path: &Path) -> Option<String> {
    let wide = HSTRING::from(path.as_os_str());
    let size = GetFileVersionInfoSizeW(&wide, None);
    if size == 0 {
        return None;
    }
    // In u16s, for the text's alignment.
    let mut data = vec![0u16; size as usize / 2 + 1];
    GetFileVersionInfoW(&wide, None, size, data.as_mut_ptr() as *mut c_void).ok()?;

    let block = data.as_ptr() as *const c_void;
    let mut value = std::ptr::null_mut();
    let mut length = 0;
    if !VerQueryValueW(
        block,
        w!("\\VarFileInfo\\Translation"),
        &mut value,
        &mut length,
    )
    .as_bool()
        || length < 4
    {
        return None;
    }
    let language = *(value as *const [u16; 2]);
    let key = HSTRING::from(format!(
        "\\StringFileInfo\\{:04x}{:04x}\\FileDescription",
        language[0], language[1]
    ));
    if !VerQueryValueW(block, &key, &mut value, &mut length).as_bool() || length == 0 {
        return None;
    }
    let text = std::slice::from_raw_parts(value as *const u16, length as usize);
    let name = String::from_utf16_lossy(text);
    let name = name.trim_matches(|c: char| c == '\0' || c.is_whitespace());
    (!name.is_empty()).then(|| name.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    const TODAY: Day = (2026, 10, 31);

    fn usage(now: Instant) -> Usage {
        Usage {
            day: TODAY,
            apps: Vec::new(),
            open: None,
            names: HashMap::new(),
            unsaved: false,
            saved_at: now,
        }
    }

    fn app(name: &str) -> Option<String> {
        Some(name.to_string())
    }

    fn times(usage: &Usage) -> Vec<(&str, u64)> {
        usage
            .apps()
            .iter()
            .map(|app| (app.name.as_str(), app.ms))
            .collect()
    }

    #[test]
    fn switching_credits_the_app_left_behind() {
        let start = Instant::now();
        let mut usage = usage(start);
        usage.update(app("Chrome"), TODAY, start);
        usage.update(app("Code"), TODAY, start + Duration::from_secs(30));
        usage.update(None, TODAY, start + Duration::from_secs(100));
        assert_eq!(times(&usage), [("Code", 70_000), ("Chrome", 30_000)]);
        assert_eq!(usage.total_ms(), 100_000);
    }

    #[test]
    fn the_same_app_keeps_counting() {
        let start = Instant::now();
        let mut usage = usage(start);
        for minute in 0..4 {
            usage.update(
                app("Chrome"),
                TODAY,
                start + Duration::from_secs(minute * 60),
            );
        }
        assert_eq!(times(&usage), [("Chrome", 180_000)]);
    }

    #[test]
    fn sleep_is_not_screen_time() {
        let start = Instant::now();
        let mut usage = usage(start);
        usage.update(app("Chrome"), TODAY, start);
        usage.update(app("Chrome"), TODAY, start + Duration::from_secs(60));
        usage.update(app("Chrome"), TODAY, start + Duration::from_secs(8 * 3600));
        assert_eq!(times(&usage), [("Chrome", 60_000)]);
    }

    #[test]
    fn going_idle_takes_the_idle_time_back() {
        let start = Instant::now();
        let mut usage = usage(start);
        for minute in 0..20 {
            usage.update(
                app("Chrome"),
                TODAY,
                start + Duration::from_secs(minute * 60),
            );
        }
        usage.pause(IDLE_LIMIT, start + Duration::from_secs(20 * 60));
        assert_eq!(times(&usage), [("Chrome", 15 * 60_000)]);
        // Nothing more counts until the app is followed again.
        usage.pause(IDLE_LIMIT, start + Duration::from_secs(21 * 60));
        assert_eq!(times(&usage), [("Chrome", 15 * 60_000)]);
    }

    #[test]
    fn waking_from_sleep_leaves_the_day_alone() {
        let start = Instant::now();
        let mut usage = usage(start);
        usage.update(app("Chrome"), TODAY, start);
        usage.update(app("Chrome"), TODAY, start + Duration::from_secs(60));
        usage.pause(Duration::from_secs(3600), start + Duration::from_secs(3660));
        assert_eq!(times(&usage), [("Chrome", 60_000)]);
    }

    #[test]
    fn a_long_idle_takes_back_only_what_was_counted() {
        let start = Instant::now();
        let mut usage = usage(start);
        for minute in 0..30 {
            usage.update(
                app("Chrome"),
                TODAY,
                start + Duration::from_secs(minute * 60),
            );
        }
        // Music kept it counting for an hour of idling, then stopped.
        usage.pause(
            Duration::from_secs(3600),
            start + Duration::from_secs(30 * 60),
        );
        assert_eq!(times(&usage), [("Chrome", 24 * 60_000)]);
    }

    #[test]
    fn a_new_day_starts_from_zero() {
        let start = Instant::now();
        let mut usage = usage(start);
        usage.update(app("Chrome"), TODAY, start);
        usage.update(
            app("Chrome"),
            (2026, 11, 1),
            start + Duration::from_secs(60),
        );
        assert!(usage.apps().is_empty());
        usage.update(None, (2026, 11, 1), start + Duration::from_secs(90));
        assert_eq!(times(&usage), [("Chrome", 30_000)]);
    }

    #[test]
    fn longest_comes_first() {
        let start = Instant::now();
        let mut usage = usage(start);
        usage.update(app("A"), TODAY, start);
        usage.update(app("B"), TODAY, start + Duration::from_secs(10));
        usage.update(None, TODAY, start + Duration::from_secs(40));
        assert_eq!(usage.apps()[0].name, "B");
    }

    #[test]
    fn reads_what_it_writes_today_only() {
        let apps = vec![
            AppTime {
                name: "Visual Studio Code".to_string(),
                ms: 90_000,
            },
            AppTime {
                name: "Google Chrome".to_string(),
                ms: 30_000,
            },
        ];
        let text = format(TODAY, &apps);
        assert_eq!(parse(&text, TODAY), apps);
        assert!(parse(&text, (2026, 11, 1)).is_empty());
        assert!(parse("", TODAY).is_empty());
        assert_eq!(
            parse("2026-10-31\nrusak\n60 App\n", TODAY),
            [AppTime {
                name: "App".to_string(),
                ms: 60_000
            }]
        );
    }

    #[test]
    fn formats_time() {
        assert_eq!(format_time(59_000), "<1m");
        assert_eq!(format_time(45 * 60_000), "45m");
        assert_eq!(format_time(120 * 60_000), "2j");
        assert_eq!(format_time(135 * 60_000), "2j 15m");
    }
}
