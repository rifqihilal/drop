//! Place search for the settings window, using Open-Meteo's free geocoding
//! service over WinHTTP. Only runs when the user presses "Cari".

use std::ffi::c_void;

use windows::core::{w, HSTRING, PCWSTR};
use windows::Win32::Networking::WinHttp::{
    WinHttpCloseHandle, WinHttpConnect, WinHttpOpen, WinHttpOpenRequest, WinHttpQueryDataAvailable,
    WinHttpReadData, WinHttpReceiveResponse, WinHttpSendRequest, WinHttpSetTimeouts,
    WINHTTP_ACCESS_TYPE_AUTOMATIC_PROXY, WINHTTP_FLAG_SECURE,
};

const HOST: PCWSTR = w!("geocoding-api.open-meteo.com");
const TIMEOUT_MS: i32 = 10_000;

#[derive(Clone, Debug, PartialEq)]
pub struct Place {
    pub name: String,
    /// "Kabupaten Sorong, Papua Barat Daya, Indonesia"
    pub region: String,
    pub latitude: f64,
    pub longitude: f64,
    pub elevation: f64,
}

/// Up to six places matching `query`, Indonesian ones first.
pub fn search(query: &str) -> Result<Vec<Place>, String> {
    let path = format!(
        "/v1/search?name={}&count=10&language=id&format=json",
        percent_encode(query.trim())
    );
    let body = get(&path).ok_or("Tidak dapat terhubung ke layanan pencarian")?;
    let mut places = parse(&body)?;
    // Prayer times follow the Kemenag method, so places in Indonesia come
    // first, and only they are listed when there are any.
    if places
        .iter()
        .any(|place| place.region.ends_with("Indonesia"))
    {
        places.retain(|place| place.region.ends_with("Indonesia"));
    }
    places.sort_by_key(|place| !place.region.ends_with("Indonesia"));
    places.truncate(6);
    Ok(places)
}

fn parse(body: &[u8]) -> Result<Vec<Place>, String> {
    let json: serde_json::Value =
        serde_json::from_slice(body).map_err(|_| "Jawaban layanan tidak terbaca".to_string())?;
    let results = json["results"].as_array().cloned().unwrap_or_default();
    Ok(results
        .iter()
        .filter_map(|result| {
            let text = |key: &str| result[key].as_str().unwrap_or("").to_string();
            let region = [text("admin2"), text("admin1"), text("country")]
                .into_iter()
                .filter(|part| !part.is_empty())
                .collect::<Vec<_>>()
                .join(", ");
            Some(Place {
                name: text("name"),
                region,
                latitude: result["latitude"].as_f64()?,
                longitude: result["longitude"].as_f64()?,
                elevation: result["elevation"].as_f64().unwrap_or(0.0).max(0.0),
            })
        })
        .collect())
}

/// HTTPS GET on [`HOST`]; None on any network failure.
fn get(path: &str) -> Option<Vec<u8>> {
    unsafe {
        let session = Handle(WinHttpOpen(
            w!("Drop"),
            WINHTTP_ACCESS_TYPE_AUTOMATIC_PROXY,
            PCWSTR::null(),
            PCWSTR::null(),
            0,
        ));
        session.valid()?;
        let _ = WinHttpSetTimeouts(session.0, TIMEOUT_MS, TIMEOUT_MS, TIMEOUT_MS, TIMEOUT_MS);
        let connection = Handle(WinHttpConnect(session.0, HOST, 443, 0));
        connection.valid()?;
        let request = Handle(WinHttpOpenRequest(
            connection.0,
            w!("GET"),
            &HSTRING::from(path),
            PCWSTR::null(),
            PCWSTR::null(),
            std::ptr::null(),
            WINHTTP_FLAG_SECURE,
        ));
        request.valid()?;
        WinHttpSendRequest(request.0, None, None, 0, 0, 0).ok()?;
        WinHttpReceiveResponse(request.0, std::ptr::null_mut()).ok()?;

        let mut body = Vec::new();
        loop {
            let mut available = 0;
            WinHttpQueryDataAvailable(request.0, &mut available).ok()?;
            if available == 0 {
                break;
            }
            let start = body.len();
            body.resize(start + available as usize, 0);
            let mut read = 0;
            WinHttpReadData(
                request.0,
                body[start..].as_mut_ptr() as *mut c_void,
                available,
                &mut read,
            )
            .ok()?;
            body.truncate(start + read as usize);
        }
        Some(body)
    }
}

/// Closes a WinHTTP handle when dropped.
struct Handle(*mut c_void);

impl Handle {
    fn valid(&self) -> Option<()> {
        (!self.0.is_null()).then_some(())
    }
}

impl Drop for Handle {
    fn drop(&mut self) {
        if !self.0.is_null() {
            unsafe {
                let _ = WinHttpCloseHandle(self.0);
            }
        }
    }
}

fn percent_encode(text: &str) -> String {
    text.bytes()
        .map(|byte| match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'.' | b'_' | b'~' => {
                (byte as char).to_string()
            }
            _ => format!("%{byte:02X}"),
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn encodes_spaces_and_unicode() {
        assert_eq!(percent_encode("Kota Sorong"), "Kota%20Sorong");
        assert_eq!(percent_encode("é"), "%C3%A9");
    }

    #[test]
    fn reads_open_meteo_results() {
        let body = br#"{"results":[{"name":"Aimas","latitude":-0.95698,"longitude":131.33534,
            "elevation":9.0,"admin1":"Papua Barat Daya","admin2":"Kabupaten Sorong",
            "country":"Indonesia"}]}"#;
        let places = parse(body).unwrap();
        assert_eq!(places.len(), 1);
        assert_eq!(places[0].name, "Aimas");
        assert_eq!(
            places[0].region,
            "Kabupaten Sorong, Papua Barat Daya, Indonesia"
        );
        assert_eq!(places[0].elevation, 9.0);
    }

    #[test]
    fn no_results_is_an_empty_list() {
        assert_eq!(parse(br#"{"generationtime_ms":0.1}"#).unwrap(), vec![]);
    }
}
