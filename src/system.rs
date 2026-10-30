//! Volume, brightness and power changes, delivered to the notch window as
//! messages.
//!
//! All event-driven: Core Audio calls back on volume changes and the power
//! manager broadcasts brightness and battery changes, so nothing polls.

use std::ffi::c_void;

use windows::core::{implement, Result, GUID, PCWSTR};
use windows::Win32::Foundation::{HANDLE, HWND, LPARAM, PROPERTYKEY, WPARAM};
use windows::Win32::Media::Audio::Endpoints::{
    IAudioEndpointVolume, IAudioEndpointVolumeCallback, IAudioEndpointVolumeCallback_Impl,
    IAudioMeterInformation,
};
use windows::Win32::Media::Audio::{
    eConsole, eRender, EDataFlow, ERole, IMMDeviceEnumerator, IMMNotificationClient,
    IMMNotificationClient_Impl, MMDeviceEnumerator, AUDIO_VOLUME_NOTIFICATION_DATA, DEVICE_STATE,
};
use windows::Win32::System::Com::{CoCreateInstance, CLSCTX_ALL};
use windows::Win32::System::Power::{
    GetSystemPowerStatus, RegisterPowerSettingNotification, POWERBROADCAST_SETTING,
    SYSTEM_POWER_STATUS,
};
use windows::Win32::System::SystemServices::{
    GUID_ACDC_POWER_SOURCE, GUID_BATTERY_PERCENTAGE_REMAINING,
    GUID_VIDEO_CURRENT_MONITOR_BRIGHTNESS,
};
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
    /// Output level of the same device, for the music bars.
    meter: Option<IAudioMeterInformation>,
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
                meter: None,
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
            self.meter = device
                .Activate::<IAudioMeterInformation>(CLSCTX_ALL, None)
                .ok();
            let Ok(volume) = device.Activate::<IAudioEndpointVolume>(CLSCTX_ALL, None) else {
                return;
            };
            let callback: IAudioEndpointVolumeCallback = VolumeCallback { hwnd: self.hwnd }.into();
            if volume.RegisterControlChangeNotify(&callback).is_ok() {
                self.endpoint = Some((volume, callback));
            }
        }
    }

    /// Current output peak of the default device, 0..1. Cheap: it reads the
    /// level the audio engine already tracks, without capturing audio.
    pub fn peak(&self) -> f32 {
        self.meter
            .as_ref()
            .and_then(|meter| unsafe { meter.GetPeakValue() }.ok())
            .unwrap_or(0.0)
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

pub enum PowerEvent {
    /// Built-in display brightness, 0..100.
    Brightness(u32),
    /// True on mains power, false on battery.
    OnAc(bool),
    /// Battery charge, 0..100.
    Battery(u32),
}

const POWER_SETTINGS: [GUID; 3] = [
    GUID_VIDEO_CURRENT_MONITOR_BRIGHTNESS,
    GUID_ACDC_POWER_SOURCE,
    GUID_BATTERY_PERCENTAGE_REMAINING,
];

/// Asks for WM_POWERBROADCAST when brightness, the power source or the
/// battery charge changes. Windows also sends each current value right after
/// registering.
pub fn watch_power(hwnd: HWND) {
    for setting in &POWER_SETTINGS {
        unsafe {
            let _ = RegisterPowerSettingNotification(
                HANDLE(hwnd.0),
                setting,
                DEVICE_NOTIFY_WINDOW_HANDLE,
            );
        }
    }
}

/// Reads a WM_POWERBROADCAST message.
pub fn power_event(wparam: WPARAM, lparam: LPARAM) -> Option<PowerEvent> {
    if wparam.0 as u32 != PBT_POWERSETTINGCHANGE || lparam.0 == 0 {
        return None;
    }
    let (setting, value) = unsafe {
        let setting = &*(lparam.0 as *const POWERBROADCAST_SETTING);
        if (setting.DataLength as usize) < size_of::<u32>() {
            return None;
        }
        let value = std::ptr::read_unaligned(setting.Data.as_ptr() as *const u32);
        (setting.PowerSetting, value)
    };
    if setting == GUID_VIDEO_CURRENT_MONITOR_BRIGHTNESS {
        Some(PowerEvent::Brightness(value.min(100)))
    } else if setting == GUID_ACDC_POWER_SOURCE {
        // 0 = AC, 1 = battery, 2 = short-term source such as a UPS.
        Some(PowerEvent::OnAc(value == 0))
    } else if setting == GUID_BATTERY_PERCENTAGE_REMAINING {
        Some(PowerEvent::Battery(value.min(100)))
    } else {
        None
    }
}

/// Battery charge and whether it is charging; None without a battery.
pub fn battery_status() -> Option<(u32, bool)> {
    let mut status = SYSTEM_POWER_STATUS::default();
    unsafe { GetSystemPowerStatus(&mut status).ok()? };
    // Flag 128: no system battery; 255: charge unknown.
    if status.BatteryFlag & 128 != 0 || status.BatteryLifePercent > 100 {
        return None;
    }
    let charging = status.ACLineStatus == 1 || status.BatteryFlag & 8 != 0;
    Some((u32::from(status.BatteryLifePercent), charging))
}
