//! Bluetooth device connections, from the radio's HCI events.

use std::ffi::c_void;
use std::time::{Duration, Instant};

use windows::Win32::Devices::Bluetooth::{
    BluetoothFindFirstRadio, BluetoothFindRadioClose, BluetoothGetDeviceInfo, BLUETOOTH_ADDRESS,
    BLUETOOTH_ADDRESS_0, BLUETOOTH_DEVICE_INFO, BLUETOOTH_FIND_RADIO_PARAMS, BTH_HCI_EVENT_INFO,
    GUID_BLUETOOTH_HCI_EVENT, HCI_CONNECTION_TYPE_SCO,
};
use windows::Win32::Foundation::{CloseHandle, HANDLE, HWND, LPARAM, WPARAM};
use windows::Win32::UI::WindowsAndMessaging::{
    RegisterDeviceNotificationW, UnregisterDeviceNotification, DBT_CUSTOMEVENT, DBT_DEVTYP_HANDLE,
    DEVICE_NOTIFY_WINDOW_HANDLE, DEV_BROADCAST_HANDLE, HDEVNOTIFY,
};

/// A device reporting the same state again within this window is ignored;
/// headsets open several links while connecting.
const REPEAT_WINDOW: Duration = Duration::from_secs(5);

pub struct DeviceEvent {
    pub name: String,
    pub connected: bool,
    /// Headphones, speakers and the like.
    pub audio: bool,
}

pub struct Bluetooth {
    radio: HANDLE,
    notify: HDEVNOTIFY,
    recent: Vec<(u64, bool, Instant)>,
}

impl Bluetooth {
    /// Delivers WM_DEVICECHANGE for the first Bluetooth radio. None when
    /// there is no radio.
    pub fn watch(hwnd: HWND) -> Option<Self> {
        unsafe {
            let params = BLUETOOTH_FIND_RADIO_PARAMS {
                dwSize: size_of::<BLUETOOTH_FIND_RADIO_PARAMS>() as u32,
            };
            let mut radio = HANDLE::default();
            let find = BluetoothFindFirstRadio(&params, &mut radio).ok()?;
            let _ = BluetoothFindRadioClose(find);

            let filter = DEV_BROADCAST_HANDLE {
                dbch_size: size_of::<DEV_BROADCAST_HANDLE>() as u32,
                dbch_devicetype: DBT_DEVTYP_HANDLE.0,
                dbch_handle: radio,
                ..Default::default()
            };
            match RegisterDeviceNotificationW(
                HANDLE(hwnd.0),
                &filter as *const DEV_BROADCAST_HANDLE as *const c_void,
                DEVICE_NOTIFY_WINDOW_HANDLE,
            ) {
                Ok(notify) => Some(Self {
                    radio,
                    notify,
                    recent: Vec::new(),
                }),
                Err(_) => {
                    let _ = CloseHandle(radio);
                    None
                }
            }
        }
    }

    /// Reads a WM_DEVICECHANGE. Reports paired devices only.
    pub fn event(&mut self, wparam: WPARAM, lparam: LPARAM) -> Option<DeviceEvent> {
        if wparam.0 as u32 != DBT_CUSTOMEVENT || lparam.0 == 0 {
            return None;
        }
        let info = unsafe {
            let header = &*(lparam.0 as *const DEV_BROADCAST_HANDLE);
            if header.dbch_devicetype != DBT_DEVTYP_HANDLE.0
                || header.dbch_eventguid != GUID_BLUETOOTH_HCI_EVENT
            {
                return None;
            }
            std::ptr::read_unaligned(header.dbch_data.as_ptr() as *const BTH_HCI_EVENT_INFO)
        };
        // Audio side-channels come and go with calls; only device links count.
        if info.connectionType as u32 == HCI_CONNECTION_TYPE_SCO {
            return None;
        }
        let connected = info.connected != 0;

        let now = Instant::now();
        self.recent
            .retain(|(_, _, at)| now.duration_since(*at) < REPEAT_WINDOW);
        if self
            .recent
            .iter()
            .any(|&(address, state, _)| address == info.bthAddress && state == connected)
        {
            return None;
        }
        self.recent.push((info.bthAddress, connected, now));

        let (name, audio) = self.paired_device(info.bthAddress)?;
        Some(DeviceEvent {
            name,
            connected,
            audio,
        })
    }

    /// Name of a paired device and whether it is an audio device. Unpaired
    /// devices (nearby phones, beacons) connect in the background all the
    /// time and are not worth a notice.
    fn paired_device(&self, address: u64) -> Option<(String, bool)> {
        let mut device = BLUETOOTH_DEVICE_INFO {
            dwSize: size_of::<BLUETOOTH_DEVICE_INFO>() as u32,
            Address: BLUETOOTH_ADDRESS {
                Anonymous: BLUETOOTH_ADDRESS_0 { ullLong: address },
            },
            ..Default::default()
        };
        unsafe {
            if BluetoothGetDeviceInfo(Some(self.radio), &mut device) != 0 {
                return None;
            }
        }
        if !device.fRemembered.as_bool() {
            return None;
        }
        let length = device
            .szName
            .iter()
            .position(|&c| c == 0)
            .unwrap_or(device.szName.len());
        let name = String::from_utf16_lossy(&device.szName[..length]);
        let name = if name.trim().is_empty() {
            "Perangkat Bluetooth".to_string()
        } else {
            name
        };
        // Bits 8..12 of the class of device hold the major class; 4 is audio/video.
        let audio = (device.ulClassofDevice >> 8) & 0x1F == 4;
        Some((name, audio))
    }
}

impl Drop for Bluetooth {
    fn drop(&mut self) {
        unsafe {
            let _ = UnregisterDeviceNotification(self.notify);
            let _ = CloseHandle(self.radio);
        }
    }
}
