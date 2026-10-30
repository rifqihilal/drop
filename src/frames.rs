//! Frame pacing for animations. Window timers tick on a ~15.6 ms grid, so a
//! 16 ms timer lands on 16 or 31 ms and motion stutters. Instead, while
//! something animates, a thread waits for each display refresh (DwmFlush)
//! and asks the notch window for one frame.

use std::ffi::c_void;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Condvar, Mutex};

use windows::Win32::Foundation::{HWND, LPARAM, WPARAM};
use windows::Win32::Graphics::Dwm::DwmFlush;
use windows::Win32::UI::WindowsAndMessaging::{PostMessageW, WM_APP};

/// Time for a frame.
pub const FRAME_MSG: u32 = WM_APP + 12;

struct Shared {
    active: Mutex<bool>,
    wake: Condvar,
    /// A frame message is queued and not yet handled; no need for another.
    pending: AtomicBool,
}

pub struct Pacer {
    shared: Arc<Shared>,
}

impl Pacer {
    pub fn start(hwnd: HWND) -> Self {
        let shared = Arc::new(Shared {
            active: Mutex::new(false),
            wake: Condvar::new(),
            pending: AtomicBool::new(false),
        });
        let worker = shared.clone();
        let target = hwnd.0 as isize;
        std::thread::spawn(move || loop {
            {
                let Ok(mut active) = worker.active.lock() else {
                    return;
                };
                while !*active {
                    active = match worker.wake.wait(active) {
                        Ok(guard) => guard,
                        Err(_) => return,
                    };
                }
            }
            // Returns once the compositor has presented the next frame.
            if unsafe { DwmFlush() }.is_err() {
                std::thread::sleep(std::time::Duration::from_millis(16));
            }
            if !worker.pending.swap(true, Ordering::AcqRel) {
                unsafe {
                    let _ = PostMessageW(
                        Some(HWND(target as *mut c_void)),
                        FRAME_MSG,
                        WPARAM(0),
                        LPARAM(0),
                    );
                }
            }
        });
        Self { shared }
    }

    /// Runs frames while `active`; idles otherwise.
    pub fn set_active(&self, active: bool) {
        if let Ok(mut current) = self.shared.active.lock() {
            if *current != active {
                *current = active;
                self.shared.wake.notify_one();
            }
        }
    }

    /// Call when handling [`FRAME_MSG`], so the next refresh posts again.
    pub fn frame_taken(&self) {
        self.shared.pending.store(false, Ordering::Release);
    }
}
