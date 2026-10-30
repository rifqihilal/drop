//! Minimal HTTP(S) GET over WinHTTP, for the place search, the weather and
//! calendar feeds.
//! Calls block, so they run on worker threads.

use std::ffi::c_void;

use windows::core::{w, HSTRING, PCWSTR};
use windows::Win32::Networking::WinHttp::{
    WinHttpCloseHandle, WinHttpConnect, WinHttpOpen, WinHttpOpenRequest, WinHttpQueryDataAvailable,
    WinHttpReadData, WinHttpReceiveResponse, WinHttpSendRequest, WinHttpSetTimeouts,
    WINHTTP_ACCESS_TYPE_AUTOMATIC_PROXY, WINHTTP_FLAG_SECURE, WINHTTP_OPEN_REQUEST_FLAGS,
};

const TIMEOUT_MS: i32 = 10_000;

/// HTTPS GET of `path` on `host`; None on any network failure.
pub fn get(host: &str, path: &str) -> Option<Vec<u8>> {
    request(host, 443, path, true)
}

/// GET of a full link: `https://`, `http://`, or `webcal://` (a calendar
/// feed served over HTTPS), with an optional port.
pub fn get_url(url: &str) -> Option<Vec<u8>> {
    let url = url.trim();
    let (secure, rest) = if let Some(rest) = strip_scheme(url, "https://") {
        (true, rest)
    } else if let Some(rest) = strip_scheme(url, "webcal://") {
        (true, rest)
    } else {
        (false, strip_scheme(url, "http://")?)
    };
    let (authority, path) = match rest.find('/') {
        Some(slash) => rest.split_at(slash),
        None => (rest, "/"),
    };
    let (host, port) = match authority.rsplit_once(':') {
        Some((host, port)) => (host, port.parse().ok()?),
        None => (authority, if secure { 443 } else { 80 }),
    };
    request(host, port, path, secure)
}

fn strip_scheme<'a>(url: &'a str, scheme: &str) -> Option<&'a str> {
    let head = url.get(..scheme.len())?;
    head.eq_ignore_ascii_case(scheme)
        .then(|| &url[scheme.len()..])
}

fn request(host: &str, port: u16, path: &str, secure: bool) -> Option<Vec<u8>> {
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
        let connection = Handle(WinHttpConnect(session.0, &HSTRING::from(host), port, 0));
        connection.valid()?;
        let request = Handle(WinHttpOpenRequest(
            connection.0,
            w!("GET"),
            &HSTRING::from(path),
            PCWSTR::null(),
            PCWSTR::null(),
            std::ptr::null(),
            if secure {
                WINHTTP_FLAG_SECURE
            } else {
                WINHTTP_OPEN_REQUEST_FLAGS(0)
            },
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

pub fn percent_encode(text: &str) -> String {
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
    use super::percent_encode;

    #[test]
    fn encodes_spaces_and_unicode() {
        assert_eq!(percent_encode("Kota Sorong"), "Kota%20Sorong");
        assert_eq!(percent_encode("é"), "%C3%A9");
    }
}
