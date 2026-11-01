//! Volume, brightness and power changes, delivered to the notch window as
//! messages.
//!
//! All event-driven: Core Audio calls back on volume changes and the power
//! manager broadcasts brightness and battery changes, so nothing polls.

use std::ffi::c_void;
use std::os::windows::ffi::OsStrExt;
use std::path::Path;
use std::sync::atomic::{AtomicIsize, Ordering};

use windows::core::{implement, Result, GUID, HSTRING, PCWSTR};
use windows::Win32::Foundation::HGLOBAL;
use windows::Win32::Foundation::RECT;
use windows::Win32::Foundation::{
    GlobalFree, GENERIC_READ, HANDLE, HWND, LPARAM, PROPERTYKEY, WPARAM,
};
use windows::Win32::Graphics::Gdi::{
    GetMonitorInfoW, MonitorFromWindow, BITMAPINFOHEADER, MONITORINFO, MONITOR_DEFAULTTONEAREST,
};
use windows::Win32::Graphics::Imaging::{
    CLSID_WICImagingFactory, GUID_WICPixelFormat32bppBGRA, IWICImagingFactory,
    WICBitmapDitherTypeNone, WICBitmapPaletteTypeMedianCut, WICDecodeMetadataCacheOnDemand,
};
use windows::Win32::Media::Audio::Endpoints::{
    IAudioEndpointVolume, IAudioEndpointVolumeCallback, IAudioEndpointVolumeCallback_Impl,
    IAudioMeterInformation,
};
use windows::Win32::Media::Audio::{
    eConsole, eRender, EDataFlow, ERole, IMMDeviceEnumerator, IMMNotificationClient,
    IMMNotificationClient_Impl, MMDeviceEnumerator, AUDIO_VOLUME_NOTIFICATION_DATA, DEVICE_STATE,
};
use windows::Win32::System::Com::{CoCreateInstance, CLSCTX_ALL, CLSCTX_INPROC_SERVER};
use windows::Win32::System::DataExchange::{
    CloseClipboard, EmptyClipboard, GetClipboardData, IsClipboardFormatAvailable, OpenClipboard,
    SetClipboardData,
};
use windows::Win32::System::Memory::{
    GlobalAlloc, GlobalLock, GlobalSize, GlobalUnlock, GMEM_MOVEABLE,
};
use windows::Win32::System::Ole::{CF_DIB, CF_HDROP, CF_UNICODETEXT};
use windows::Win32::System::Power::{
    GetSystemPowerStatus, RegisterPowerSettingNotification, POWERBROADCAST_SETTING,
    SYSTEM_POWER_STATUS,
};
use windows::Win32::System::SystemServices::{
    GUID_ACDC_POWER_SOURCE, GUID_BATTERY_PERCENTAGE_REMAINING,
    GUID_VIDEO_CURRENT_MONITOR_BRIGHTNESS,
};
use windows::Win32::UI::Accessibility::{SetWinEventHook, HWINEVENTHOOK};
use windows::Win32::UI::Input::KeyboardAndMouse::{VK_VOLUME_DOWN, VK_VOLUME_MUTE, VK_VOLUME_UP};
use windows::Win32::UI::Shell::{
    SHQueryUserNotificationState, DROPFILES, QUNS_PRESENTATION_MODE, QUNS_RUNNING_D3D_FULL_SCREEN,
};
use windows::Win32::UI::WindowsAndMessaging::{
    GetClassNameW, GetForegroundWindow, GetShellWindow, GetWindowRect, PostMessageW, SetWindowPos,
    DEVICE_NOTIFY_WINDOW_HANDLE, EVENT_SYSTEM_FOREGROUND, HWND_TOPMOST, PBT_POWERSETTINGCHANGE,
    SWP_NOACTIVATE, SWP_NOMOVE, SWP_NOSIZE, WINEVENT_OUTOFCONTEXT, WM_APP,
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

    /// Master volume of the default device, 0..1.
    pub fn volume(&self) -> Option<f32> {
        let (volume, _) = self.endpoint.as_ref()?;
        unsafe { volume.GetMasterVolumeLevelScalar() }.ok()
    }

    /// Sets the master volume, 0..1; hearing it again also unmutes.
    pub fn set_volume(&self, level: f32) {
        if let Some((volume, _)) = &self.endpoint {
            unsafe {
                let _ = volume.SetMute(false, std::ptr::null());
                let _ = volume.SetMasterVolumeLevelScalar(level, std::ptr::null());
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

/// Another window came to the front. Windows sometimes leaves a topmost
/// window under ordinary ones then, so the notch takes its place back.
pub const FOREGROUND_MSG: u32 = WM_APP + 17;

static NOTCH: AtomicIsize = AtomicIsize::new(0);

unsafe extern "system" fn foreground_changed(
    _hook: HWINEVENTHOOK,
    _event: u32,
    _hwnd: HWND,
    _object: i32,
    _child: i32,
    _thread: u32,
    _time: u32,
) {
    post(NOTCH.load(Ordering::Relaxed), FOREGROUND_MSG, 0, 0);
}

/// Asks for [`FOREGROUND_MSG`] whenever the foreground window changes.
pub fn watch_foreground(hwnd: HWND) {
    NOTCH.store(hwnd.0 as isize, Ordering::Relaxed);
    unsafe {
        SetWinEventHook(
            EVENT_SYSTEM_FOREGROUND,
            EVENT_SYSTEM_FOREGROUND,
            None,
            Some(foreground_changed),
            0,
            0,
            WINEVENT_OUTOFCONTEXT,
        );
    }
}

/// Puts the notch back above every ordinary window, without taking the focus.
pub fn stay_on_top(hwnd: HWND) {
    unsafe {
        let _ = SetWindowPos(
            hwnd,
            Some(HWND_TOPMOST),
            0,
            0,
            0,
            0,
            SWP_NOMOVE | SWP_NOSIZE | SWP_NOACTIVATE,
        );
    }
}

/// True while another app fills the notch's monitor (a game, a full-screen
/// video, a presentation), so the notch can step aside.
pub fn fullscreen_app(own: HWND) -> bool {
    unsafe {
        if SHQueryUserNotificationState().is_ok_and(|state| {
            state == QUNS_RUNNING_D3D_FULL_SCREEN || state == QUNS_PRESENTATION_MODE
        }) {
            return true;
        }
        let window = GetForegroundWindow();
        if window.is_invalid() || window == own || window == GetShellWindow() {
            return false;
        }
        // The desktop itself covers the screen too.
        let mut class = [0u16; 16];
        let length = GetClassNameW(window, &mut class).max(0) as usize;
        let class = String::from_utf16_lossy(&class[..length]);
        if class == "WorkerW" || class == "Progman" {
            return false;
        }
        let monitor = MonitorFromWindow(window, MONITOR_DEFAULTTONEAREST);
        if monitor != MonitorFromWindow(own, MONITOR_DEFAULTTONEAREST) {
            return false;
        }
        let mut info = MONITORINFO {
            cbSize: size_of::<MONITORINFO>() as u32,
            ..Default::default()
        };
        let mut bounds = RECT::default();
        if !GetMonitorInfoW(monitor, &mut info).as_bool()
            || GetWindowRect(window, &mut bounds).is_err()
        {
            return false;
        }
        let screen = info.rcMonitor;
        bounds.left <= screen.left
            && bounds.top <= screen.top
            && bounds.right >= screen.right
            && bounds.bottom >= screen.bottom
    }
}

#[link(name = "ntdll")]
extern "system" {
    fn NtQueryWnfStateData(
        name: *const u64,
        type_id: *const c_void,
        scope: *const c_void,
        stamp: *mut u32,
        buffer: *mut c_void,
        size: *mut u32,
    ) -> i32;
}

/// The Windows state that holds the active quiet-hours profile. No public API
/// tells Do Not Disturb from a merely busy session.
const WNF_QUIET_HOURS: u64 = 0x0D83_063E_A3BF_1C75;

/// Whether Windows' Do Not Disturb is on, by hand or by an automatic rule.
pub fn do_not_disturb() -> bool {
    let (mut stamp, mut profile) = (0u32, 0u32);
    let mut size = size_of::<u32>() as u32;
    let status = unsafe {
        NtQueryWnfStateData(
            &WNF_QUIET_HOURS,
            std::ptr::null(),
            std::ptr::null(),
            &mut stamp,
            (&raw mut profile).cast(),
            &mut size,
        )
    };
    status >= 0 && profile != 0
}

/// Pictures larger than this are copied as a file only: their pixels would
/// sit in this process for as long as they stay on the clipboard.
const CLIPBOARD_PIXELS: u64 = 16_000_000;

/// Whether a picture is on the clipboard.
pub fn clipboard_has_image() -> bool {
    unsafe { IsClipboardFormatAvailable(CF_DIB.0 as u32).is_ok() }
}

/// Puts a file on the clipboard, as Explorer's copy does, and for a picture
/// its pixels too, so it pastes into chats and editors as well as folders.
pub fn copy_file(owner: HWND, path: &Path) -> bool {
    let picture = unsafe { read_picture(path) }.ok();
    unsafe {
        if OpenClipboard(Some(owner)).is_err() {
            return false;
        }
        let copied = EmptyClipboard().is_ok() && set_bytes(CF_HDROP.0, &file_list(path));
        if let Some(picture) = picture.filter(|_| copied) {
            set_bytes(CF_DIB.0, &picture);
        }
        let _ = CloseClipboard();
        copied
    }
}

/// Puts text on the clipboard.
pub fn copy_text(owner: HWND, text: &str) -> bool {
    let bytes: Vec<u8> = text
        .encode_utf16()
        .chain([0])
        .flat_map(u16::to_le_bytes)
        .collect();
    unsafe {
        if OpenClipboard(Some(owner)).is_err() {
            return false;
        }
        let copied = EmptyClipboard().is_ok() && set_bytes(CF_UNICODETEXT.0, &bytes);
        let _ = CloseClipboard();
        copied
    }
}

/// `path` in the layout of a file drop: a header, then a double-null-ended
/// list of wide strings.
fn file_list(path: &Path) -> Vec<u8> {
    let header = DROPFILES {
        pFiles: size_of::<DROPFILES>() as u32,
        fWide: true.into(),
        ..Default::default()
    };
    let mut bytes = unsafe {
        std::slice::from_raw_parts((&raw const header).cast::<u8>(), size_of::<DROPFILES>())
    }
    .to_vec();
    let units = path.as_os_str().encode_wide().chain([0, 0]);
    bytes.extend(units.flat_map(u16::to_le_bytes));
    bytes
}

/// Reads a picture as a device-independent bitmap, the clipboard's format.
unsafe fn read_picture(path: &Path) -> Result<Vec<u8>> {
    let wic: IWICImagingFactory =
        CoCreateInstance(&CLSID_WICImagingFactory, None, CLSCTX_INPROC_SERVER)?;
    let decoder = wic.CreateDecoderFromFilename(
        &HSTRING::from(path.as_os_str()),
        None,
        GENERIC_READ,
        WICDecodeMetadataCacheOnDemand,
    )?;
    let frame = decoder.GetFrame(0)?;
    let (mut width, mut height) = (0, 0);
    frame.GetSize(&mut width, &mut height)?;
    if u64::from(width) * u64::from(height) > CLIPBOARD_PIXELS {
        return Err(windows::core::Error::empty());
    }
    let converter = wic.CreateFormatConverter()?;
    converter.Initialize(
        &frame,
        &GUID_WICPixelFormat32bppBGRA,
        WICBitmapDitherTypeNone,
        None,
        0.0,
        WICBitmapPaletteTypeMedianCut,
    )?;
    let stride = width as usize * 4;
    let mut pixels = vec![0u8; stride * height as usize];
    converter.CopyPixels(std::ptr::null(), stride as u32, &mut pixels)?;

    let header = BITMAPINFOHEADER {
        biSize: size_of::<BITMAPINFOHEADER>() as u32,
        biWidth: width as i32,
        biHeight: height as i32,
        biPlanes: 1,
        biBitCount: 32,
        ..Default::default()
    };
    let mut bytes = Vec::with_capacity(size_of::<BITMAPINFOHEADER>() + pixels.len());
    bytes.extend_from_slice(std::slice::from_raw_parts(
        (&raw const header).cast::<u8>(),
        size_of::<BITMAPINFOHEADER>(),
    ));
    // Bitmaps run from the bottom row up.
    for row in pixels.rchunks_exact(stride) {
        bytes.extend_from_slice(row);
    }
    Ok(bytes)
}

/// Copies `bytes` onto the open clipboard, which then owns the memory.
unsafe fn set_bytes(format: u16, bytes: &[u8]) -> bool {
    let Ok(memory) = GlobalAlloc(GMEM_MOVEABLE, bytes.len()) else {
        return false;
    };
    let target = GlobalLock(memory) as *mut u8;
    if target.is_null() {
        let _ = GlobalFree(Some(memory));
        return false;
    }
    std::ptr::copy_nonoverlapping(bytes.as_ptr(), target, bytes.len());
    let _ = GlobalUnlock(memory);
    if SetClipboardData(format.into(), Some(HANDLE(memory.0))).is_err() {
        let _ = GlobalFree(Some(memory));
        return false;
    }
    true
}

/// The picture on the clipboard as a device-independent bitmap, if any.
pub fn clipboard_picture() -> Option<Vec<u8>> {
    unsafe {
        OpenClipboard(None).ok()?;
        let bytes = GetClipboardData(CF_DIB.0 as u32).ok().and_then(|handle| {
            let memory = HGLOBAL(handle.0);
            let source = GlobalLock(memory) as *const u8;
            if source.is_null() {
                return None;
            }
            let bytes = std::slice::from_raw_parts(source, GlobalSize(memory)).to_vec();
            let _ = GlobalUnlock(memory);
            Some(bytes)
        });
        let _ = CloseClipboard();
        bytes
    }
}

/// Text on the clipboard, if there is any.
pub fn clipboard_text() -> Option<String> {
    unsafe {
        OpenClipboard(None).ok()?;
        let text = GetClipboardData(CF_UNICODETEXT.0 as u32)
            .ok()
            .and_then(|handle| {
                let memory = HGLOBAL(handle.0);
                let units = GlobalLock(memory) as *const u16;
                if units.is_null() {
                    return None;
                }
                let mut len = 0;
                while *units.add(len) != 0 {
                    len += 1;
                }
                let text = String::from_utf16_lossy(std::slice::from_raw_parts(units, len));
                let _ = GlobalUnlock(memory);
                Some(text)
            });
        let _ = CloseClipboard();
        text
    }
}
