//! Bluetooth device connections, from the radio's HCI events.

use std::ffi::c_void;
use std::time::{Duration, Instant};

use windows::core::{w, GUID, PCWSTR};
use windows::Win32::Devices::Bluetooth::{
    BluetoothFindFirstRadio, BluetoothFindRadioClose, BluetoothGetDeviceInfo, BLUETOOTH_ADDRESS,
    BLUETOOTH_ADDRESS_0, BLUETOOTH_DEVICE_INFO, BLUETOOTH_FIND_RADIO_PARAMS, BTH_HCI_EVENT_INFO,
    GUID_BLUETOOTH_HCI_EVENT, HCI_CONNECTION_TYPE_SCO,
};
use windows::Win32::Devices::DeviceAndDriverInstallation::{
    CM_Get_DevNode_PropertyW, CM_Get_Device_ID_ListW, CM_Get_Device_ID_List_SizeW,
    CM_Locate_DevNodeW, CM_GETIDLIST_FILTER_ENUMERATOR, CM_GETIDLIST_FILTER_PRESENT,
    CM_LOCATE_DEVNODE_NORMAL, CR_SUCCESS,
};
use windows::Win32::Devices::Properties::{DEVPROPTYPE, DEVPROP_TYPE_BYTE};
use windows::Win32::Foundation::{CloseHandle, DEVPROPKEY, HANDLE, HWND, LPARAM, WPARAM};
use windows::Win32::UI::WindowsAndMessaging::{
    RegisterDeviceNotificationW, UnregisterDeviceNotification, DBT_CUSTOMEVENT, DBT_DEVTYP_HANDLE,
    DEVICE_NOTIFY_WINDOW_HANDLE, DEV_BROADCAST_HANDLE, HDEVNOTIFY,
};

/// A device reporting the same state again within this window is ignored;
/// headsets open several links while connecting.
const REPEAT_WINDOW: Duration = Duration::from_secs(5);

pub struct DeviceEvent {
    pub address: u64,
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
            address: info.bthAddress,
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

/// Battery level a device reports over Bluetooth (DEVPKEY_Bluetooth_Battery).
const BATTERY_KEY: DEVPROPKEY = DEVPROPKEY {
    fmtid: GUID::from_u128(0x104ea319_6ee2_4701_bd47_8ddbf425bbe5),
    pid: 2,
};

/// Last battery charge the device reported, if it reports one at all.
/// Windows stores it on one of the device's service nodes (usually the
/// hands-free one), whose instance IDs contain the device address.
pub fn battery_level(address: u64) -> Option<u32> {
    let needle = format!("{address:012X}");
    [w!("BTHENUM"), w!("BTHLE")]
        .into_iter()
        .flat_map(device_ids)
        .filter(|id| id.to_uppercase().contains(&needle))
        .find_map(|id| read_battery(&id))
}

/// Instance IDs of the present devices under one enumerator.
fn device_ids(enumerator: PCWSTR) -> Vec<String> {
    let flags = CM_GETIDLIST_FILTER_ENUMERATOR | CM_GETIDLIST_FILTER_PRESENT;
    unsafe {
        let mut length = 0;
        if CM_Get_Device_ID_List_SizeW(&mut length, enumerator, flags) != CR_SUCCESS {
            return Vec::new();
        }
        let mut buffer = vec![0u16; length as usize];
        if CM_Get_Device_ID_ListW(enumerator, &mut buffer, flags) != CR_SUCCESS {
            return Vec::new();
        }
        buffer
            .split(|&c| c == 0)
            .filter(|id| !id.is_empty())
            .map(String::from_utf16_lossy)
            .collect()
    }
}

fn read_battery(id: &str) -> Option<u32> {
    let wide: Vec<u16> = id.encode_utf16().chain(std::iter::once(0)).collect();
    unsafe {
        let mut node = 0;
        if CM_Locate_DevNodeW(&mut node, PCWSTR(wide.as_ptr()), CM_LOCATE_DEVNODE_NORMAL)
            != CR_SUCCESS
        {
            return None;
        }
        let mut kind = DEVPROPTYPE(0);
        let mut value = 0u8;
        let mut size = 1u32;
        let result = CM_Get_DevNode_PropertyW(
            node,
            &BATTERY_KEY,
            &mut kind,
            Some(&mut value),
            &mut size,
            0,
        );
        (result == CR_SUCCESS && kind == DEVPROP_TYPE_BYTE).then(|| u32::from(value).min(100))
    }
}

#[cfg(test)]
mod tests {
    /// Manual probe against a real paired device:
    /// `DROP_BT_ADDRESS=4142EA5A5F4E cargo test -- --ignored --nocapture`
    #[test]
    #[ignore]
    fn reads_battery_of_a_paired_device() {
        let address = std::env::var("DROP_BT_ADDRESS").expect("set DROP_BT_ADDRESS");
        let address = u64::from_str_radix(&address, 16).expect("hex address");
        let level = super::battery_level(address);
        println!("battery level: {level:?}");
        assert!(level.is_some());
    }
}
