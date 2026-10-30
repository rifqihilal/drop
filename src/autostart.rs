//! Start with Windows through the per-user Run key.

use std::ffi::c_void;
use std::os::windows::ffi::OsStrExt;

use windows::core::{w, PCWSTR};
use windows::Win32::System::Registry::{
    RegDeleteKeyValueW, RegGetValueW, RegSetKeyValueW, HKEY_CURRENT_USER, REG_SZ, RRF_RT_REG_SZ,
};

const RUN_KEY: PCWSTR = w!("Software\\Microsoft\\Windows\\CurrentVersion\\Run");
const VALUE_NAME: PCWSTR = w!("DropNotch");

pub fn is_enabled() -> bool {
    unsafe {
        RegGetValueW(
            HKEY_CURRENT_USER,
            RUN_KEY,
            VALUE_NAME,
            RRF_RT_REG_SZ,
            None,
            None,
            None,
        )
        .is_ok()
    }
}

pub fn set_enabled(enabled: bool) {
    unsafe {
        if !enabled {
            let _ = RegDeleteKeyValueW(HKEY_CURRENT_USER, RUN_KEY, VALUE_NAME);
            return;
        }
        let Ok(exe) = std::env::current_exe() else {
            return;
        };
        // Quoted so a path with spaces still launches; NUL-terminated for REG_SZ.
        let mut command: Vec<u16> = "\"".encode_utf16().collect();
        command.extend(exe.as_os_str().encode_wide());
        command.extend("\"\0".encode_utf16());
        let _ = RegSetKeyValueW(
            HKEY_CURRENT_USER,
            RUN_KEY,
            VALUE_NAME,
            REG_SZ.0,
            Some(command.as_ptr() as *const c_void),
            (command.len() * size_of::<u16>()) as u32,
        );
    }
}
