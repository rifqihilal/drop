//! Volume and brightness changes, delivered to the notch window as messages.
//!
//! Both are event-driven: Core Audio calls back on volume changes and the power
//! manager broadcasts brightness changes, so nothing polls in the background.

use std::ffi::c_void;

use windows::core::{implement, Result, PCWSTR};
use windows::Win32::Foundation::{HANDLE, HWND, LPARAM, PROPERTYKEY, WPARAM};
use windows::Win32::Media::Audio::Endpoints::{
    IAudioEndpointVolume, IAudioEndpointVolumeCallback, IAudioEndpointVolumeCallback_Impl,
};
use windows::Win32::Media::Audio::{
    eConsole, eRender, EDataFlow, ERole, IMMDeviceEnumerator, IMMNotificationClient,
    IMMNotificationClient_Impl, MMDeviceEnumerator, AUDIO_VOLUME_NOTIFICATION_DATA, DEVICE_STATE,
};
use windows::Win32::System::Com::{CoCreateInstance, CLSCTX_ALL};
use windows::Win32::System::Power::{RegisterPowerSettingNotification, POWERBROADCAST_SETTING};
use windows::Win32::System::SystemServices::GUID_VIDEO_CURRENT_MONITOR_BRIGHTNESS;
use windows::Win32::UI::Input::KeyboardAndMouse::{VK_VOLUME_DOWN, VK_VOLUME_MUTE, VK_VOLUME_UP};
use windows::Win32::UI::WindowsAndMessaging::{
    PostMessageW, DEVICE_NOTIFY_WINDOW_HANDLE, PBT_POWERSETTINGCHANGE, WM_APP,
};

/// Volume changed. WPARAM: level in per mille, LPARAM: 1 when muted.
pub const VOLUME_MSG: u32 = WM_APP + 2;
/// The default output device changed; the volume watch has to move to it.
pub const DEVICE_MSG: u32 = WM_APP + 3;

#[implement(IAudioEndpointVolumeCallback)]
struct VolumeCallback {
    hwnd: isize,
}

impl IAudioEndpointVolumeCallback_Impl for VolumeCallback_Impl {
    fn OnNotify(&self, data: *mut AUDIO_VOLUME_NOTIFICATION_DATA) -> Result<()> {
        // Runs on a Core Audio thread; hand the values to the UI thread.
        if let Some(data) = unsafe { data.as_ref() } {
            let level = (data.fMasterVolume.clamp(0.0, 1.0) * 1000.0).round() as usize;
            post(self.hwnd, VOLUME_MSG, level, data.bMuted.as_bool() as isize);
        }
        Ok(())
    }
}

#[implement(IMMNotificationClient)]
struct DeviceWatcher {
    hwnd: isize,
}

impl IMMNotificationClient_Impl for DeviceWatcher_Impl {
    fn OnDefaultDeviceChanged(&self, flow: EDataFlow, role: ERole, _id: &PCWSTR) -> Result<()> {
        if flow == eRender && role == eConsole {
            post(self.hwnd, DEVICE_MSG, 0, 0);
        }
        Ok(())
    }

    fn OnDeviceStateChanged(&self, _id: &PCWSTR, _state: DEVICE_STATE) -> Result<()> {
        Ok(())
    }

    fn OnDeviceAdded(&self, _id: &PCWSTR) -> Result<()> {
        Ok(())
    }

    fn OnDeviceRemoved(&self, _id: &PCWSTR) -> Result<()> {
        Ok(())
    }

    fn OnPropertyValueChanged(&self, _id: &PCWSTR, _key: &PROPERTYKEY) -> Result<()> {
        Ok(())
    }
}

fn post(hwnd: isize, msg: u32, wparam: usize, lparam: isize) {
    unsafe {
        let _ = PostMessageW(
            Some(HWND(hwnd as *mut c_void)),
            msg,
            WPARAM(wparam),
            LPARAM(lparam),
        );
    }
}

/// Watches the master volume of the default output device.
pub struct Audio {
    hwnd: isize,
    enumerator: IMMDeviceEnumerator,
    watcher: IMMNotificationClient,
    endpoint: Option<(IAudioEndpointVolume, IAudioEndpointVolumeCallback)>,
}

impl Audio {
    /// Needs COM initialized on the calling thread.
    pub fn new(hwnd: HWND) -> Result<Self> {
        let hwnd = hwnd.0 as isize;
        unsafe {
            let enumerator: IMMDeviceEnumerator =
                CoCreateInstance(&MMDeviceEnumerator, None, CLSCTX_ALL)?;
            let watcher: IMMNotificationClient = DeviceWatcher { hwnd }.into();
            enumerator.RegisterEndpointNotificationCallback(&watcher)?;
            let mut audio = Self {
                hwnd,
                enumerator,
                watcher,
                endpoint: None,
            };
            audio.rebind();
            Ok(audio)
        }
    }

    /// Moves the volume watch to the current default output device.
    pub fn rebind(&mut self) {
        self.unbind();
        unsafe {
            let Ok(device) = self.enumerator.GetDefaultAudioEndpoint(eRender, eConsole) else {
                return;
            };
            let Ok(volume) = device.Activate::<IAudioEndpointVolume>(CLSCTX_ALL, None) else {
                return;
            };
            let callback: IAudioEndpointVolumeCallback = VolumeCallback { hwnd: self.hwnd }.into();
            if volume.RegisterControlChangeNotify(&callback).is_ok() {
                self.endpoint = Some((volume, callback));
            }
        }
    }

    /// Applies a volume key and returns the resulting level and mute state.
    /// Returns a value even when nothing changed (already at 100%), so the
    /// notch can still show where the volume is.
    pub fn press(&self, vk: u16) -> Option<(f32, bool)> {
        let (volume, _) = self.endpoint.as_ref()?;
        unsafe {
            if vk == VK_VOLUME_UP.0 {
                // Raising the volume also unmutes, as the Windows keys do.
                let _ = volume.SetMute(false, std::ptr::null());
                let _ = volume.VolumeStepUp(std::ptr::null());
            } else if vk == VK_VOLUME_DOWN.0 {
                let _ = volume.VolumeStepDown(std::ptr::null());
            } else if vk == VK_VOLUME_MUTE.0 {
                let muted = volume.GetMute().map(|m| m.as_bool()).unwrap_or(false);
                let _ = volume.SetMute(!muted, std::ptr::null());
            }
            let level = volume.GetMasterVolumeLevelScalar().ok()?;
            let muted = volume.GetMute().ok()?.as_bool();
            Some((level, muted))
        }
    }

    fn unbind(&mut self) {
        if let Some((volume, callback)) = self.endpoint.take() {
            unsafe {
                let _ = volume.UnregisterControlChangeNotify(&callback);
            }
        }
    }
}

impl Drop for Audio {
    fn drop(&mut self) {
        self.unbind();
        unsafe {
            let _ = self
                .enumerator
                .UnregisterEndpointNotificationCallback(&self.watcher);
        }
    }
}

/// Asks for WM_POWERBROADCAST whenever the built-in display's brightness
/// changes. Windows also sends the current value right after registering.
pub fn watch_brightness(hwnd: HWND) {
    unsafe {
        let _ = RegisterPowerSettingNotification(
            HANDLE(hwnd.0),
            &GUID_VIDEO_CURRENT_MONITOR_BRIGHTNESS,
            DEVICE_NOTIFY_WINDOW_HANDLE,
        );
    }
}

/// Brightness percentage carried by a WM_POWERBROADCAST message, if it is one.
pub fn brightness_from(wparam: WPARAM, lparam: LPARAM) -> Option<u32> {
    if wparam.0 as u32 != PBT_POWERSETTINGCHANGE || lparam.0 == 0 {
        return None;
    }
    unsafe {
        let setting = &*(lparam.0 as *const POWERBROADCAST_SETTING);
        if setting.PowerSetting != GUID_VIDEO_CURRENT_MONITOR_BRIGHTNESS
            || (setting.DataLength as usize) < size_of::<u32>()
        {
            return None;
        }
        Some(std::ptr::read_unaligned(setting.Data.as_ptr() as *const u32).min(100))
    }
}
