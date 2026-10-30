//! Wi-Fi connect and disconnect events from the WLAN service.

use std::ffi::c_void;
use std::sync::atomic::{AtomicIsize, Ordering};

use windows::Win32::Foundation::{HANDLE, HWND, LPARAM, WPARAM};
use windows::Win32::NetworkManagement::WiFi::{
    wlan_notification_acm_connection_complete, wlan_notification_acm_disconnected, WlanCloseHandle,
    WlanOpenHandle, WlanRegisterNotification, L2_NOTIFICATION_DATA,
    WLAN_CONNECTION_NOTIFICATION_DATA, WLAN_NOTIFICATION_SOURCE_ACM,
};
use windows::Win32::UI::WindowsAndMessaging::{PostMessageW, WM_APP};

/// Wi-Fi state changed. WPARAM: 1 connected, 0 disconnected.
/// LPARAM: a `Box<String>` with the network name; take it with [`take_ssid`].
pub const WIFI_MSG: u32 = WM_APP + 5;

static TARGET: AtomicIsize = AtomicIsize::new(0);

pub struct Wifi(HANDLE);

impl Wifi {
    /// None when the computer has no Wi-Fi or the WLAN service is off.
    pub fn watch(hwnd: HWND) -> Option<Self> {
        TARGET.store(hwnd.0 as isize, Ordering::Relaxed);
        unsafe {
            let mut version = 0;
            let mut handle = HANDLE::default();
            if WlanOpenHandle(2, None, &mut version, &mut handle) != 0 {
                return None;
            }
            let registered = WlanRegisterNotification(
                handle,
                WLAN_NOTIFICATION_SOURCE_ACM,
                true,
                Some(callback),
                None,
                None,
                None,
            );
            if registered != 0 {
                WlanCloseHandle(handle, None);
                return None;
            }
            Some(Self(handle))
        }
    }
}

impl Drop for Wifi {
    fn drop(&mut self) {
        // Closing the handle also ends the notifications.
        unsafe {
            WlanCloseHandle(self.0, None);
        }
    }
}

/// Runs on a WLAN service thread; hands the event to the UI thread.
unsafe extern "system" fn callback(data: *mut L2_NOTIFICATION_DATA, _context: *mut c_void) {
    let Some(data) = data.as_ref() else {
        return;
    };
    if data.NotificationSource != WLAN_NOTIFICATION_SOURCE_ACM || data.pData.is_null() {
        return;
    }
    let code = data.NotificationCode as i32;
    let connected = if code == wlan_notification_acm_connection_complete.0 {
        true
    } else if code == wlan_notification_acm_disconnected.0 {
        false
    } else {
        return;
    };
    let info = &*(data.pData as *const WLAN_CONNECTION_NOTIFICATION_DATA);
    // A failed attempt also reports "complete", with a reason code.
    if connected && info.wlanReasonCode != 0 {
        return;
    }
    let length = (info.dot11Ssid.uSSIDLength as usize).min(info.dot11Ssid.ucSSID.len());
    let ssid = String::from_utf8_lossy(&info.dot11Ssid.ucSSID[..length]).into_owned();

    let payload = Box::into_raw(Box::new(ssid));
    let target = HWND(TARGET.load(Ordering::Relaxed) as *mut c_void);
    let posted = PostMessageW(
        Some(target),
        WIFI_MSG,
        WPARAM(connected as usize),
        LPARAM(payload as isize),
    );
    if posted.is_err() {
        drop(Box::from_raw(payload));
    }
}

/// Takes ownership of the network name carried by a [`WIFI_MSG`].
///
/// # Safety
/// `lparam` must come from a [`WIFI_MSG`] and be taken only once.
pub unsafe fn take_ssid(lparam: LPARAM) -> String {
    *Box::from_raw(lparam.0 as *mut String)
}
