//! Screen time per app for the last seven days, kept in
//! `%APPDATA%\Drop\screen_time.txt`: each day's date on a line of its own,
//! then one app per line as `seconds name<tab>path`.
//!
//! Event-driven: the foreground hook in `system.rs` reports each switch, and
//! the notch's minute tick settles the time and notices idling. Older days
//! are dropped, and nothing leaves this computer.

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

use crate::calendar;

/// Apps listed on the card.
pub const SHOWN: usize = 4;
/// Days kept, today included.
const DAYS: i64 = 7;
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
/// Pixels with less colour than this say nothing about an app's own colour.
const MIN_CHROMA: f32 = 0.2;
/// An app's colour is lifted to at least this bright, for the black card.
const MIN_BRIGHTNESS: f32 = 0.85;

type Day = (u16, u16, u16);

#[derive(Clone, Debug, PartialEq)]
pub struct AppTime {
    pub name: String,
    /// The program's file, for its icon.
    pub path: Option<PathBuf>,
    pub ms: u64,
}

#[derive(Debug, PartialEq)]
struct DayLog {
    day: Day,
    /// Longest first.
    apps: Vec<AppTime>,
}

/// The app being timed.
struct Open {
    name: String,
    path: PathBuf,
    since: Instant,
}

pub struct Usage {
    /// Newest first; the first is always today.
    days: Vec<DayLog>,
    /// All kept days added up, longest first.
    week: Vec<AppTime>,
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
        Self::with_days(parse(&text, today), today, now)
    }

    fn with_days(mut days: Vec<DayLog>, today: Day, now: Instant) -> Self {
        if days.first().is_none_or(|log| log.day != today) {
            days.insert(
                0,
                DayLog {
                    day: today,
                    apps: Vec::new(),
                },
            );
        }
        let mut usage = Self {
            days,
            week: Vec::new(),
            open: None,
            names: HashMap::new(),
            unsaved: false,
            saved_at: now,
        };
        usage.add_up_week();
        usage
    }

    /// Today's apps, longest first.
    pub fn apps(&self) -> &[AppTime] {
        &self.days[0].apps
    }

    /// The last seven days' apps, longest first.
    pub fn week(&self) -> &[AppTime] {
        &self.week
    }

    /// The average over the days with any screen time.
    pub fn daily_average_ms(&self) -> u64 {
        let used = self.days.iter().filter(|log| !log.apps.is_empty()).count();
        total_ms(&self.week) / used.max(1) as u64
    }

    /// Follows the app in front; None stops the clock.
    pub fn update(&mut self, app: Option<(String, PathBuf)>, today: Day, now: Instant) {
        self.settle(now);
        if today != self.days[0].day {
            self.days.insert(
                0,
                DayLog {
                    day: today,
                    apps: Vec::new(),
                },
            );
            self.days.retain(|log| recent(log.day, today));
            self.add_up_week();
            self.unsaved = true;
        }
        match (&self.open, app) {
            (Some(open), Some((name, _))) if open.name == name => {}
            (_, Some((name, path))) => {
                self.open = Some(Open {
                    name,
                    path,
                    since: now,
                })
            }
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
    pub fn in_front(&mut self) -> Option<(String, PathBuf)> {
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
            return self
                .open
                .as_ref()
                .map(|open| (open.name.clone(), open.path.clone()));
        }
        let name = self
            .names
            .entry(path.clone())
            .or_insert_with_key(|path| display_name(path))
            .clone();
        Some((name, path))
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
        let (name, path) = (open.name.clone(), open.path.clone());
        self.add(name, path, spent.as_millis() as u64);
        true
    }

    fn add(&mut self, name: String, path: PathBuf, ms: u64) {
        let apps = &mut self.days[0].apps;
        match apps.iter_mut().find(|app| app.name == name) {
            Some(app) => {
                app.ms += ms;
                app.path = Some(path);
            }
            None => apps.push(AppTime {
                name,
                path: Some(path),
                ms,
            }),
        }
        apps.sort_by_key(|app| std::cmp::Reverse(app.ms));
        self.add_up_week();
        self.unsaved = true;
    }

    fn forget(&mut self, name: &str, time: Duration) {
        let apps = &mut self.days[0].apps;
        if let Some(app) = apps.iter_mut().find(|app| app.name == name) {
            app.ms = app.ms.saturating_sub(time.as_millis() as u64);
        }
        apps.retain(|app| app.ms > 0);
        apps.sort_by_key(|app| std::cmp::Reverse(app.ms));
        self.add_up_week();
        self.unsaved = true;
    }

    fn add_up_week(&mut self) {
        let mut week: Vec<AppTime> = Vec::new();
        for app in self.days.iter().flat_map(|log| &log.apps) {
            match week.iter_mut().find(|known| known.name == app.name) {
                Some(known) => {
                    known.ms += app.ms;
                    known.path = known.path.take().or_else(|| app.path.clone());
                }
                None => week.push(app.clone()),
            }
        }
        week.sort_by_key(|app| std::cmp::Reverse(app.ms));
        self.week = week;
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
        let _ = std::fs::write(file, format(&self.days));
    }
}

/// Everything counted in `apps`, listed on the card or not.
pub fn total_ms(apps: &[AppTime]) -> u64 {
    apps.iter().map(|app| app.ms).sum()
}

/// "2j 15m", "45m", or "<1m" under a minute.
pub fn format_time(ms: u64) -> String {
    let minutes = ms / 60_000;
    match (minutes / 60, minutes % 60) {
        (0, 0) => "<1m".to_string(),
        (0, minutes) => format!("{minutes}m"),
        (hours, 0) => format!("{hours}{}", crate::lang::tr("j", "h")),
        (hours, minutes) => format!("{hours}{} {minutes}m", crate::lang::tr("j", "h")),
    }
}

/// The colour an icon shows most, brightened for the black card: the fullest
/// hue among its 32 bpp premultiplied BGRA `pixels`. None for black, white
/// and grey icons.
pub fn icon_color(pixels: &[u8]) -> Option<[f32; 3]> {
    const HUES: usize = 12;
    let mut hues = [([0.0_f32; 3], 0.0_f32); HUES];
    for pixel in pixels.as_chunks::<4>().0 {
        let alpha = f32::from(pixel[3]) / 255.0;
        if alpha < 0.5 {
            continue;
        }
        let rgb = [pixel[2], pixel[1], pixel[0]].map(|v| (f32::from(v) / 255.0 / alpha).min(1.0));
        let high = rgb[0].max(rgb[1]).max(rgb[2]);
        let chroma = high - rgb[0].min(rgb[1]).min(rgb[2]);
        if chroma < MIN_CHROMA {
            continue;
        }
        let sextant = if high == rgb[0] {
            ((rgb[1] - rgb[2]) / chroma).rem_euclid(6.0)
        } else if high == rgb[1] {
            (rgb[2] - rgb[0]) / chroma + 2.0
        } else {
            (rgb[0] - rgb[1]) / chroma + 4.0
        };
        let hue = ((sextant / 6.0 * HUES as f32) as usize).min(HUES - 1);
        let weight = chroma * chroma;
        for (total, value) in hues[hue].0.iter_mut().zip(rgb) {
            *total += value * weight;
        }
        hues[hue].1 += weight;
    }
    let (sum, weight) = hues.into_iter().max_by(|a, b| a.1.total_cmp(&b.1))?;
    if weight <= 0.0 {
        return None;
    }
    let mean = sum.map(|value| value / weight);
    let high = mean[0].max(mean[1]).max(mean[2]);
    let lift = (MIN_BRIGHTNESS / high).max(1.0);
    Some(mean.map(|value| (value * lift).min(1.0)))
}

fn file() -> Option<PathBuf> {
    // Tests jump through hours, which reaches the save; keep the real file out.
    if cfg!(test) {
        return None;
    }
    let appdata = std::env::var_os("APPDATA")?;
    Some(PathBuf::from(appdata).join("Drop").join("screen_time.txt"))
}

fn date_line((year, month, day): Day) -> String {
    format!("{year:04}-{month:02}-{day:02}")
}

fn parse_date(line: &str) -> Option<Day> {
    let mut parts = line.split('-');
    let (year, month, day) = (parts.next()?, parts.next()?, parts.next()?);
    if parts.next().is_some() {
        return None;
    }
    Some((year.parse().ok()?, month.parse().ok()?, day.parse().ok()?))
}

fn day_number((year, month, day): Day) -> i64 {
    calendar::days_from_civil(i32::from(year), u32::from(month), u32::from(day))
}

/// Whether `day` is among the days kept on `today`.
fn recent(day: Day, today: Day) -> bool {
    (0..DAYS).contains(&(day_number(today) - day_number(day)))
}

fn parse_app(line: &str) -> Option<AppTime> {
    let (seconds, rest) = line.split_once(' ')?;
    let ms = seconds.parse::<u64>().ok()?.checked_mul(1000)?;
    let (name, path) = match rest.split_once('\t') {
        Some((name, path)) => (name, Some(PathBuf::from(path.trim()))),
        None => (rest, None),
    };
    let name = name.trim();
    (!name.is_empty()).then(|| AppTime {
        name: name.to_string(),
        path,
        ms,
    })
}

/// The days in `text` still kept on `today`, newest first.
fn parse(text: &str, today: Day) -> Vec<DayLog> {
    let mut days: Vec<DayLog> = Vec::new();
    for line in text.trim_start_matches('\u{feff}').lines() {
        let line = line.trim();
        if let Some(day) = parse_date(line) {
            days.push(DayLog {
                day,
                apps: Vec::new(),
            });
        } else if let (Some(log), Some(app)) = (days.last_mut(), parse_app(line)) {
            log.apps.push(app);
        }
    }
    days.retain(|log| recent(log.day, today));
    days.sort_by_key(|log| std::cmp::Reverse(log.day));
    for log in &mut days {
        log.apps.sort_by_key(|app| std::cmp::Reverse(app.ms));
    }
    days
}

fn format(days: &[DayLog]) -> String {
    let mut text = String::new();
    for log in days {
        text.push_str(&date_line(log.day));
        text.push('\n');
        for app in &log.apps {
            text.push_str(&format!("{} {}", app.ms / 1000, app.name));
            if let Some(path) = &app.path {
                text.push('\t');
                text.push_str(&path.to_string_lossy());
            }
            text.push('\n');
        }
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
        Usage::with_days(Vec::new(), TODAY, now)
    }

    fn app(name: &str) -> Option<(String, PathBuf)> {
        Some((name.to_string(), PathBuf::from(format!(r"C:\{name}.exe"))))
    }

    fn times(apps: &[AppTime]) -> Vec<(&str, u64)> {
        apps.iter().map(|app| (app.name.as_str(), app.ms)).collect()
    }

    fn entry(name: &str, seconds: u64) -> AppTime {
        AppTime {
            name: name.to_string(),
            path: None,
            ms: seconds * 1000,
        }
    }

    #[test]
    fn switching_credits_the_app_left_behind() {
        let start = Instant::now();
        let mut usage = usage(start);
        usage.update(app("Chrome"), TODAY, start);
        usage.update(app("Code"), TODAY, start + Duration::from_secs(30));
        usage.update(None, TODAY, start + Duration::from_secs(100));
        assert_eq!(times(usage.apps()), [("Code", 70_000), ("Chrome", 30_000)]);
        assert_eq!(total_ms(usage.apps()), 100_000);
        assert_eq!(usage.apps()[0].path, Some(PathBuf::from(r"C:\Code.exe")));
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
        assert_eq!(times(usage.apps()), [("Chrome", 180_000)]);
    }

    #[test]
    fn sleep_is_not_screen_time() {
        let start = Instant::now();
        let mut usage = usage(start);
        usage.update(app("Chrome"), TODAY, start);
        usage.update(app("Chrome"), TODAY, start + Duration::from_secs(60));
        usage.update(app("Chrome"), TODAY, start + Duration::from_secs(8 * 3600));
        assert_eq!(times(usage.apps()), [("Chrome", 60_000)]);
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
        assert_eq!(times(usage.apps()), [("Chrome", 15 * 60_000)]);
        // Nothing more counts until the app is followed again.
        usage.pause(IDLE_LIMIT, start + Duration::from_secs(21 * 60));
        assert_eq!(times(usage.apps()), [("Chrome", 15 * 60_000)]);
        assert_eq!(times(usage.week()), [("Chrome", 15 * 60_000)]);
    }

    #[test]
    fn waking_from_sleep_leaves_the_day_alone() {
        let start = Instant::now();
        let mut usage = usage(start);
        usage.update(app("Chrome"), TODAY, start);
        usage.update(app("Chrome"), TODAY, start + Duration::from_secs(60));
        usage.pause(Duration::from_secs(3600), start + Duration::from_secs(3660));
        assert_eq!(times(usage.apps()), [("Chrome", 60_000)]);
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
        assert_eq!(times(usage.apps()), [("Chrome", 24 * 60_000)]);
    }

    #[test]
    fn a_new_day_starts_from_zero_and_keeps_the_week() {
        let start = Instant::now();
        let mut usage = usage(start);
        usage.update(app("Chrome"), TODAY, start);
        usage.update(
            app("Chrome"),
            (2026, 11, 1),
            start + Duration::from_secs(60),
        );
        assert!(usage.apps().is_empty());
        assert_eq!(times(usage.week()), [("Chrome", 60_000)]);
        usage.update(None, (2026, 11, 1), start + Duration::from_secs(90));
        assert_eq!(times(usage.apps()), [("Chrome", 30_000)]);
        assert_eq!(times(usage.week()), [("Chrome", 90_000)]);
        assert_eq!(usage.daily_average_ms(), 45_000);
    }

    #[test]
    fn the_week_adds_up_every_kept_day() {
        let text = "2026-10-31\n60 Chrome\n2026-10-30\n120 Chrome\n600 Code\n";
        let usage = Usage::with_days(parse(text, TODAY), TODAY, Instant::now());
        assert_eq!(times(usage.apps()), [("Chrome", 60_000)]);
        assert_eq!(
            times(usage.week()),
            [("Code", 600_000), ("Chrome", 180_000)]
        );
        assert_eq!(usage.daily_average_ms(), 390_000);
    }

    #[test]
    fn days_past_the_week_are_dropped() {
        let text = "2026-10-25\n60 Kept\n2026-10-24\n60 Dropped\n";
        let usage = Usage::with_days(parse(text, TODAY), TODAY, Instant::now());
        assert!(usage.apps().is_empty());
        assert_eq!(times(usage.week()), [("Kept", 60_000)]);
    }

    #[test]
    fn a_week_can_cross_a_year() {
        assert!(recent((2026, 12, 28), (2027, 1, 3)));
        assert!(!recent((2026, 12, 27), (2027, 1, 3)));
        assert!(!recent((2027, 1, 4), (2027, 1, 3)));
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
    fn reads_what_it_writes() {
        let mut chrome = entry("Google Chrome", 90);
        chrome.path = Some(PathBuf::from(r"C:\Program Files\Chrome\chrome.exe"));
        let days = vec![
            DayLog {
                day: TODAY,
                apps: vec![chrome, entry("Visual Studio Code", 30)],
            },
            DayLog {
                day: (2026, 10, 29),
                apps: vec![entry("WhatsApp", 60)],
            },
        ];
        assert_eq!(parse(&format(&days), TODAY), days);
    }

    #[test]
    fn reads_the_format_before_the_week() {
        let text = "2026-10-31\nrusak\n60 Google Chrome\n";
        assert_eq!(
            parse(text, TODAY),
            [DayLog {
                day: TODAY,
                apps: vec![entry("Google Chrome", 60)],
            }]
        );
        assert!(parse("", TODAY).is_empty());
    }

    #[test]
    fn formats_time() {
        assert_eq!(format_time(59_000), "<1m");
        assert_eq!(format_time(45 * 60_000), "45m");
        assert_eq!(format_time(120 * 60_000), "2j");
        assert_eq!(format_time(135 * 60_000), "2j 15m");
    }

    #[test]
    fn an_icon_gives_its_fullest_colour() {
        // BGRA: three blue pixels, one red, one clear green.
        let pixels = [
            220, 40, 20, 255, 220, 40, 20, 255, 220, 40, 20, 255, 20, 20, 220, 255, 0, 200, 0, 0,
        ];
        let [red, green, blue] = icon_color(&pixels).unwrap();
        assert!(blue > red && blue > green);
        assert!(blue >= MIN_BRIGHTNESS);
    }

    #[test]
    fn a_dark_colour_is_brightened() {
        let [red, _, _] = icon_color(&[0, 0, 100, 255]).unwrap();
        assert!((red - MIN_BRIGHTNESS).abs() < 0.001);
    }

    #[test]
    fn grey_icons_have_no_colour() {
        assert_eq!(icon_color(&[128, 128, 128, 255, 255, 255, 255, 255]), None);
        assert_eq!(icon_color(&[]), None);
    }
}
