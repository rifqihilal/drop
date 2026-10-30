#![windows_subsystem = "windows"]

//! Drop: a lightweight native notch for Windows.
//!
//! Prototype scope: a compact notch with a clock while the computer is in use,
//! growing into a pair of animated eyes once it has been idle for a while.
//! Right-click the notch or the tray icon for the menu.

mod anim;
mod autostart;
mod render;
mod tray;

use std::cell::RefCell;
use std::time::Instant;

use windows::core::{w, Result};
use windows::Win32::Foundation::{
    GetLastError, ERROR_ALREADY_EXISTS, HWND, LPARAM, LRESULT, POINT, WPARAM,
};
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
    MF_CHECKED, MF_SEPARATOR, MF_STRING, MSG, SM_CXSCREEN, SW_SHOWNOACTIVATE, TPM_RETURNCMD,
    TPM_RIGHTBUTTON, WM_DESTROY, WM_DISPLAYCHANGE, WM_DPICHANGED, WM_LBUTTONUP, WM_NULL,
    WM_RBUTTONUP, WM_SETTINGCHANGE, WM_TIMER, WNDCLASSEXW, WS_EX_LAYERED, WS_EX_NOACTIVATE,
    WS_EX_TOOLWINDOW, WS_EX_TOPMOST, WS_POPUP,
};

use anim::Notch;
use render::{Renderer, Scene};

const TIMER_ID: usize = 1;
/// Frame interval while something is animating (~60 fps).
const FRAME_MS: u32 = 16;
/// Wake-up interval while the eyes are showing: catches the next blink or glance.
const EYES_MS: u32 = 100;
/// Wake-up interval while compact: only the idle check and the clock need it.
const COMPACT_MS: u32 = 500;
const DEFAULT_IDLE_SECS: u32 = 120;
const MENU_QUIT: usize = 1;
const MENU_AUTOSTART: usize = 2;

struct App {
    hwnd: HWND,
    renderer: Renderer,
    notch: Notch,
    idle_after_ms: u32,
    last_tick: Instant,
    timer_ms: u32,
    clock: String,
}

thread_local! {
    static APP: RefCell<Option<App>> = const { RefCell::new(None) };
}

fn main() -> Result<()> {
    let idle_secs = parse_idle_secs().unwrap_or(DEFAULT_IDLE_SECS);
    unsafe {
        // One notch per session: a second launch (say, from auto-start) just exits.
        // The handle stays open for the life of the process.
        let _instance_lock = CreateMutexW(None, false, w!("Local\\DropNotch"))?;
        if GetLastError() == ERROR_ALREADY_EXISTS {
            return Ok(());
        }

        let _ = SetProcessDpiAwarenessContext(DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2);

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
            idle_after_ms: idle_secs * 1000,
            last_tick: now,
            timer_ms: 0,
            clock: current_clock(),
        };
        app.draw();
        let _ = ShowWindow(hwnd, SW_SHOWNOACTIVATE);
        tray::add(hwnd);
        app.schedule(COMPACT_MS);
        APP.with(|cell| *cell.borrow_mut() = Some(app));

        let mut msg = MSG::default();
        while GetMessageW(&mut msg, None, 0, 0).as_bool() {
            let _ = TranslateMessage(&msg);
            DispatchMessageW(&msg);
        }
    }
    Ok(())
}

impl App {
    fn tick(&mut self) {
        let now = Instant::now();
        let dt = (now - self.last_tick).as_secs_f32().min(0.05);
        self.last_tick = now;

        self.notch.set_idle(idle_ms() >= self.idle_after_ms, now);
        let moving = self.notch.step(now, dt);

        let clock = current_clock();
        let clock_changed = clock != self.clock;
        self.clock = clock;

        if moving || clock_changed {
            self.draw();
        }
        self.schedule(if moving {
            FRAME_MS
        } else if self.notch.eyes_showing() {
            EYES_MS
        } else {
            COMPACT_MS
        });
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
