//! Current weather at the prayer location, from Open-Meteo's free forecast
//! service. A worker thread fetches it every half hour and whenever the
//! location changes, and posts it to the notch window.

use std::ffi::c_void;
use std::sync::mpsc::{channel, RecvTimeoutError, Sender};
use std::time::Duration;

use windows::Win32::Foundation::{HWND, LPARAM, WPARAM};
use windows::Win32::UI::WindowsAndMessaging::{PostMessageW, WM_APP};

use crate::net;

/// New weather. LPARAM: a `Box<Weather>`.
pub const WEATHER_MSG: u32 = WM_APP + 9;
const HOST: &str = "api.open-meteo.com";
const REFRESH: Duration = Duration::from_secs(30 * 60);
/// After a failed fetch (offline, say), try again sooner.
const RETRY: Duration = Duration::from_secs(5 * 60);

#[derive(Clone, Debug, PartialEq)]
pub struct Weather {
    /// °C
    pub temperature: f64,
    /// WMO weather code.
    pub code: u32,
    pub day: bool,
}

impl Weather {
    pub fn description(&self) -> &'static str {
        match self.code {
            0 => "Cerah",
            1 | 2 => "Cerah berawan",
            3 => "Berawan",
            45 | 48 => "Berkabut",
            51..=57 => "Gerimis",
            61..=67 | 80..=82 => "Hujan",
            71..=77 | 85 | 86 => "Salju",
            95..=99 => "Badai petir",
            _ => "Cuaca",
        }
    }

    /// A Segoe UI Symbol glyph for the sky.
    pub fn glyph(&self) -> &'static str {
        match self.code {
            0 if self.day => "\u{2600}",
            0 => "\u{263E}",
            1 | 2 => "\u{26C5}",
            3 | 45 | 48 => "\u{2601}",
            51..=57 => "\u{2602}",
            61..=67 | 80..=82 => "\u{2614}",
            71..=77 | 85 | 86 => "\u{2744}",
            95..=99 => "\u{26C8}",
            _ => "\u{2601}",
        }
    }
}

/// Coordinates to fetch for, or None to stop.
pub struct WeatherWatch {
    location: Sender<Option<(f64, f64)>>,
}

impl WeatherWatch {
    pub fn start(hwnd: HWND) -> Self {
        let (location, inbox) = channel::<Option<(f64, f64)>>();
        let target = hwnd.0 as isize;
        std::thread::spawn(move || {
            let mut place: Option<(f64, f64)> = None;
            let mut wait = REFRESH;
            loop {
                match inbox.recv_timeout(wait) {
                    Ok(new) => place = new,
                    Err(RecvTimeoutError::Timeout) => {}
                    Err(RecvTimeoutError::Disconnected) => break,
                }
                let Some((latitude, longitude)) = place else {
                    wait = REFRESH;
                    continue;
                };
                match fetch(latitude, longitude) {
                    Some(weather) => {
                        post(target, weather);
                        wait = REFRESH;
                    }
                    None => wait = RETRY,
                }
            }
        });
        Self { location }
    }

    /// Fetches now for the given place (None pauses fetching).
    pub fn set_location(&self, location: Option<(f64, f64)>) {
        let _ = self.location.send(location);
    }
}

fn fetch(latitude: f64, longitude: f64) -> Option<Weather> {
    let path = format!(
        "/v1/forecast?latitude={latitude}&longitude={longitude}\
         &current=temperature_2m,weather_code,is_day&timezone=auto"
    );
    parse(&net::get(HOST, &path)?)
}

fn parse(body: &[u8]) -> Option<Weather> {
    let json: serde_json::Value = serde_json::from_slice(body).ok()?;
    let current = &json["current"];
    Some(Weather {
        temperature: current["temperature_2m"].as_f64()?,
        code: current["weather_code"].as_u64()? as u32,
        day: current["is_day"].as_u64().unwrap_or(1) == 1,
    })
}

fn post(target: isize, weather: Weather) {
    let payload = Box::into_raw(Box::new(weather));
    unsafe {
        let posted = PostMessageW(
            Some(HWND(target as *mut c_void)),
            WEATHER_MSG,
            WPARAM(0),
            LPARAM(payload as isize),
        );
        if posted.is_err() {
            drop(Box::from_raw(payload));
        }
    }
}

/// Takes ownership of the weather carried by a [`WEATHER_MSG`].
///
/// # Safety
/// `lparam` must come from a [`WEATHER_MSG`] and be taken only once.
pub unsafe fn take_weather(lparam: LPARAM) -> Weather {
    *Box::from_raw(lparam.0 as *mut Weather)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_open_meteo_current_weather() {
        let body = br#"{"utc_offset_seconds":32400,"current":{"time":"2026-10-30T12:30",
            "interval":900,"temperature_2m":29.1,"weather_code":95,"is_day":1}}"#;
        let weather = parse(body).unwrap();
        assert_eq!(weather.temperature, 29.1);
        assert_eq!(weather.description(), "Badai petir");
        assert!(weather.day);
    }

    #[test]
    fn clear_night_shows_the_moon() {
        let night = Weather {
            temperature: 24.0,
            code: 0,
            day: false,
        };
        assert_eq!(night.glyph(), "\u{263E}");
    }
}
