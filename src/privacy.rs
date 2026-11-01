//! Whether an app is using the camera or the microphone.
//!
//! The microphone shows as an active capture session in Core Audio and the
//! camera in Media Foundation's sensor activity report. Both call back on
//! change, and a worker thread sleeps until they do, so nothing polls.

use std::ffi::c_void;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

use windows::core::{implement, Ref, Result, BOOL, GUID, PCWSTR};
use windows::Win32::Foundation::{HANDLE, HWND, LPARAM, PROPERTYKEY, WAIT_OBJECT_0, WPARAM};
use windows::Win32::Media::Audio::{
    eCapture, AudioSessionDisconnectReason, AudioSessionState, AudioSessionStateActive, EDataFlow,
    ERole, IAudioSessionControl, IAudioSessionEvents, IAudioSessionEvents_Impl,
    IAudioSessionManager2, IAudioSessionNotification, IAudioSessionNotification_Impl,
    IMMDeviceEnumerator, IMMNotificationClient, IMMNotificationClient_Impl, MMDeviceEnumerator,
    DEVICE_STATE, DEVICE_STATE_ACTIVE,
};
use windows::Win32::Media::MediaFoundation::{
    IMFSensorActivitiesReport, IMFSensorActivitiesReportCallback,
    IMFSensorActivitiesReportCallback_Impl, MFCreateSensorActivityMonitor, MFStartup,
    MFSTARTUP_LITE, MF_VERSION,
};
use windows::Win32::System::Com::{
    CoCreateInstance, CoInitializeEx, CLSCTX_ALL, COINIT_MULTITHREADED,
};
use windows::Win32::System::Threading::{CreateEventW, SetEvent, WaitForSingleObject, INFINITE};
use windows::Win32::UI::WindowsAndMessaging::{PostMessageW, WM_APP};

/// The camera or microphone started or stopped being used. WPARAM: the
/// [`CAMERA`] and [`MIC`] bits of what is in use now.
pub const PRIVACY_MSG: u32 = WM_APP + 20;
pub const CAMERA: usize = 1;
pub const MIC: usize = 2;

/// Wakes the worker: an event handle, which any thread may set.
#[derive(Clone, Copy)]
struct Wake(isize);

impl Wake {
    fn set(self) {
        unsafe {
            let _ = SetEvent(HANDLE(self.0 as *mut c_void));
        }
    }
}

#[implement(IAudioSessionNotification)]
struct NewSession(Wake);

impl IAudioSessionNotification_Impl for NewSession_Impl {
    fn OnSessionCreated(&self, _session: Ref<'_, IAudioSessionControl>) -> Result<()> {
        self.0.set();
        Ok(())
    }
}

#[implement(IAudioSessionEvents)]
struct SessionEvents(Wake);

impl IAudioSessionEvents_Impl for SessionEvents_Impl {
    fn OnDisplayNameChanged(&self, _name: &PCWSTR, _context: *const GUID) -> Result<()> {
        Ok(())
    }

    fn OnIconPathChanged(&self, _path: &PCWSTR, _context: *const GUID) -> Result<()> {
        Ok(())
    }

    fn OnSimpleVolumeChanged(&self, _level: f32, _mute: BOOL, _context: *const GUID) -> Result<()> {
        Ok(())
    }

    fn OnChannelVolumeChanged(
        &self,
        _count: u32,
        _levels: *const f32,
        _channel: u32,
        _context: *const GUID,
    ) -> Result<()> {
        Ok(())
    }

    fn OnGroupingParamChanged(&self, _param: *const GUID, _context: *const GUID) -> Result<()> {
        Ok(())
    }

    fn OnStateChanged(&self, _state: AudioSessionState) -> Result<()> {
        self.0.set();
        Ok(())
    }

    fn OnSessionDisconnected(&self, _reason: AudioSessionDisconnectReason) -> Result<()> {
        self.0.set();
        Ok(())
    }
}

#[implement(IMMNotificationClient)]
struct Devices(Wake);

impl IMMNotificationClient_Impl for Devices_Impl {
    fn OnDefaultDeviceChanged(&self, _flow: EDataFlow, _role: ERole, _id: &PCWSTR) -> Result<()> {
        self.0.set();
        Ok(())
    }

    fn OnDeviceStateChanged(&self, _id: &PCWSTR, _state: DEVICE_STATE) -> Result<()> {
        self.0.set();
        Ok(())
    }

    fn OnDeviceAdded(&self, _id: &PCWSTR) -> Result<()> {
        self.0.set();
        Ok(())
    }

    fn OnDeviceRemoved(&self, _id: &PCWSTR) -> Result<()> {
        self.0.set();
        Ok(())
    }

    fn OnPropertyValueChanged(&self, _id: &PCWSTR, _key: &PROPERTYKEY) -> Result<()> {
        Ok(())
    }
}

#[implement(IMFSensorActivitiesReportCallback)]
struct CameraReports {
    wake: Wake,
    in_use: Arc<AtomicBool>,
}

impl IMFSensorActivitiesReportCallback_Impl for CameraReports_Impl {
    fn OnActivitiesReport(&self, report: Ref<'_, IMFSensorActivitiesReport>) -> Result<()> {
        let streaming = report.ok().is_ok_and(streaming);
        // Reports repeat while a camera runs; only a change needs the worker.
        if self.in_use.swap(streaming, Ordering::Relaxed) != streaming {
            self.wake.set();
        }
        Ok(())
    }
}

/// Whether any process in `report` is receiving frames.
fn streaming(report: &IMFSensorActivitiesReport) -> bool {
    unsafe {
        let devices = report.GetCount().unwrap_or(0);
        (0..devices).any(|device| {
            let Ok(device) = report.GetActivityReport(device) else {
                return false;
            };
            let processes = device.GetProcessCount().unwrap_or(0);
            (0..processes).any(|process| {
                device
                    .GetProcessActivity(process)
                    .and_then(|process| process.GetStreamingState())
                    .is_ok_and(|state| state.as_bool())
            })
        })
    }
}

/// A microphone's capture sessions, watched until dropped.
struct Microphone {
    manager: IAudioSessionManager2,
    notice: IAudioSessionNotification,
    sessions: Vec<(IAudioSessionControl, IAudioSessionEvents)>,
}

impl Microphone {
    unsafe fn watch(manager: IAudioSessionManager2, wake: Wake) -> Result<Self> {
        let notice: IAudioSessionNotification = NewSession(wake).into();
        manager.RegisterSessionNotification(&notice)?;
        let mut microphone = Self {
            manager,
            notice,
            sessions: Vec::new(),
        };
        let sessions = microphone.manager.GetSessionEnumerator()?;
        for index in 0..sessions.GetCount()? {
            let session = sessions.GetSession(index)?;
            let events: IAudioSessionEvents = SessionEvents(wake).into();
            session.RegisterAudioSessionNotification(&events)?;
            microphone.sessions.push((session, events));
        }
        Ok(microphone)
    }

    fn in_use(&self) -> bool {
        self.sessions.iter().any(|(session, _)| {
            unsafe { session.GetState() }.is_ok_and(|state| state == AudioSessionStateActive)
        })
    }
}

impl Drop for Microphone {
    fn drop(&mut self) {
        unsafe {
            for (session, events) in &self.sessions {
                let _ = session.UnregisterAudioSessionNotification(events);
            }
            let _ = self.manager.UnregisterSessionNotification(&self.notice);
        }
    }
}

/// Every microphone plugged in, watched for its sessions.
unsafe fn microphones(enumerator: &IMMDeviceEnumerator, wake: Wake) -> Vec<Microphone> {
    let mut found = Vec::new();
    let Ok(devices) = enumerator.EnumAudioEndpoints(eCapture, DEVICE_STATE_ACTIVE) else {
        return found;
    };
    for index in 0..devices.GetCount().unwrap_or(0) {
        let manager = devices
            .Item(index)
            .and_then(|device| device.Activate::<IAudioSessionManager2>(CLSCTX_ALL, None));
        if let Ok(microphone) = manager.and_then(|manager| Microphone::watch(manager, wake)) {
            found.push(microphone);
        }
    }
    found
}

/// Posts [`PRIVACY_MSG`] at the start and whenever the state changes.
pub fn watch(hwnd: HWND) {
    let target = hwnd.0 as isize;
    std::thread::spawn(move || unsafe {
        let _ = CoInitializeEx(None, COINIT_MULTITHREADED);
        let Ok(event) = CreateEventW(None, false, false, None) else {
            return;
        };
        let wake = Wake(event.0 as isize);
        let camera = Arc::new(AtomicBool::new(false));

        // Held for as long as the thread lives, so their callbacks keep coming.
        let _monitor = (|| {
            MFStartup(MF_VERSION, MFSTARTUP_LITE)?;
            let reports: IMFSensorActivitiesReportCallback = CameraReports {
                wake,
                in_use: camera.clone(),
            }
            .into();
            let monitor = MFCreateSensorActivityMonitor(&reports)?;
            monitor.Start()?;
            Ok::<_, windows::core::Error>(monitor)
        })();
        let Ok(enumerator) =
            CoCreateInstance::<_, IMMDeviceEnumerator>(&MMDeviceEnumerator, None, CLSCTX_ALL)
        else {
            return;
        };
        let devices: IMMNotificationClient = Devices(wake).into();
        let _ = enumerator.RegisterEndpointNotificationCallback(&devices);

        let mut last = usize::MAX;
        loop {
            // Sessions come and go, so each wake watches them afresh, and
            // before reading them so that a change in between is not lost.
            let watched = microphones(&enumerator, wake);
            let mut now = 0;
            if camera.load(Ordering::Relaxed) {
                now |= CAMERA;
            }
            if watched.iter().any(Microphone::in_use) {
                now |= MIC;
            }
            if now != last {
                last = now;
                let _ = PostMessageW(
                    Some(HWND(target as *mut c_void)),
                    PRIVACY_MSG,
                    WPARAM(now),
                    LPARAM(0),
                );
            }
            if WaitForSingleObject(event, INFINITE) != WAIT_OBJECT_0 {
                return;
            }
        }
    });
}
