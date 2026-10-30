//! Takes over the volume keys so Windows never opens its own flyout for them;
//! the notch window applies the change instead.

use std::ffi::c_void;
use std::sync::atomic::{AtomicIsize, Ordering};

use windows::Win32::Foundation::{HWND, LPARAM, LRESULT, WPARAM};
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::UI::Input::KeyboardAndMouse::{VK_VOLUME_DOWN, VK_VOLUME_MUTE, VK_VOLUME_UP};
use windows::Win32::UI::WindowsAndMessaging::{
    CallNextHookEx, PostMessageW, SetWindowsHookExW, HC_ACTION, KBDLLHOOKSTRUCT, WH_KEYBOARD_LL,
    WM_APP, WM_KEYDOWN, WM_SYSKEYDOWN,
};

/// A volume key was pressed. WPARAM: the virtual-key code.
pub const VOLUME_KEY_MSG: u32 = WM_APP + 4;

static TARGET: AtomicIsize = AtomicIsize::new(0);

pub fn install(hwnd: HWND) {
    TARGET.store(hwnd.0 as isize, Ordering::Relaxed);
    unsafe {
        let module = GetModuleHandleW(None).ok();
        let _ = SetWindowsHookExW(WH_KEYBOARD_LL, Some(hook), module.map(Into::into), 0);
    }
}

unsafe extern "system" fn hook(code: i32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
    if code == HC_ACTION as i32 {
        let key = &*(lparam.0 as *const KBDLLHOOKSTRUCT);
        let vk = key.vkCode as u16;
        if vk == VK_VOLUME_UP.0 || vk == VK_VOLUME_DOWN.0 || vk == VK_VOLUME_MUTE.0 {
            let message = wparam.0 as u32;
            // Key repeat arrives as more key-downs, so holding the key keeps stepping.
            if message == WM_KEYDOWN || message == WM_SYSKEYDOWN {
                let target = HWND(TARGET.load(Ordering::Relaxed) as *mut c_void);
                let _ = PostMessageW(Some(target), VOLUME_KEY_MSG, WPARAM(vk as usize), LPARAM(0));
            }
            // Swallow both down and up so the shell never sees the key.
            return LRESULT(1);
        }
    }
    CallNextHookEx(None, code, wparam, lparam)
}
