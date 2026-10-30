//! Time-synced lyrics for the song playing, from LRCLIB (lrclib.net), a free
//! open lyrics library. A worker thread looks each new song up and posts
//! the lines to the notch window.

use std::ffi::c_void;
use std::sync::mpsc::{channel, RecvTimeoutError, Sender};
use std::time::Duration;

use windows::Win32::Foundation::{HWND, LPARAM, WPARAM};
use windows::Win32::UI::WindowsAndMessaging::{PostMessageW, WM_APP};

use crate::net::{self, percent_encode};

/// Lyrics found, or not. LPARAM: a `Box<Found>`.
pub const LYRICS_MSG: u32 = WM_APP + 14;
const HOST: &str = "lrclib.net";
/// Lines show this many seconds early, so they arrive with the voice.
const LEAD: f64 = 0.3;
/// When the server cannot answer, it is asked again after this long, a few times.
const RETRY: Duration = Duration::from_secs(20);
const MAX_TRIES: u32 = 6;

/// Lines with the second each starts at, in order.
#[derive(Clone, Debug, PartialEq)]
pub struct Lyrics {
    lines: Vec<(f64, String)>,
}

impl Lyrics {
    /// The line sung at `position` seconds and the one after it. Before the
    /// first line and in pauses between lines the current line is empty.
    pub fn at(&self, position: f64) -> (&str, &str) {
        let line = |index: usize| self.lines.get(index).map_or("", |(_, line)| line.as_str());
        match self
            .lines
            .iter()
            .rposition(|(start, _)| *start <= position + LEAD)
        {
            Some(current) => (line(current), line(current + 1)),
            None => ("", line(0)),
        }
    }
}

/// Which song an answer is for, and its lyrics if any were found.
pub struct Found {
    pub artist: String,
    pub title: String,
    pub lyrics: Option<Lyrics>,
}

struct Request {
    artist: String,
    title: String,
    duration: f64,
}

pub struct LyricsWatch {
    requests: Sender<Request>,
}

impl LyricsWatch {
    pub fn start(hwnd: HWND) -> Self {
        let (requests, inbox) = channel::<Request>();
        let target = hwnd.0 as isize;
        std::thread::spawn(move || {
            // A song the server could not answer for yet, and how often it was tried.
            let mut pending: Option<Request> = None;
            let mut tries = 0;
            loop {
                let next = match pending {
                    Some(_) => inbox.recv_timeout(RETRY),
                    None => inbox.recv().map_err(|_| RecvTimeoutError::Disconnected),
                };
                match next {
                    Ok(request) => (pending, tries) = (Some(request), 0),
                    Err(RecvTimeoutError::Timeout) => {}
                    Err(RecvTimeoutError::Disconnected) => break,
                }
                // Skipping through songs: only the latest one matters.
                while let Ok(newer) = inbox.try_recv() {
                    (pending, tries) = (Some(newer), 0);
                }
                let Some(request) = pending.take() else {
                    continue;
                };
                match fetch(&request.artist, &request.title, request.duration) {
                    Ok(lyrics) => post(
                        target,
                        Found {
                            artist: request.artist,
                            title: request.title,
                            lyrics,
                        },
                    ),
                    Err(Unanswered) if tries < MAX_TRIES => {
                        tries += 1;
                        pending = Some(request);
                    }
                    Err(Unanswered) => {}
                }
            }
        });
        Self { requests }
    }

    pub fn request(&self, artist: &str, title: &str, duration: f64) {
        let _ = self.requests.send(Request {
            artist: artist.to_string(),
            title: title.to_string(),
            duration,
        });
    }
}

/// Exact match first (same length within a couple of seconds), then a
/// search, taking the first result with synced lines.
fn fetch(artist: &str, title: &str, duration: f64) -> Result<Option<Lyrics>, Unanswered> {
    let (artist, title) = clean(artist, title);
    if title.is_empty() {
        return Ok(None);
    }
    let mut path = format!(
        "/api/get?artist_name={}&track_name={}",
        percent_encode(&artist),
        percent_encode(&title)
    );
    if duration > 0.0 {
        path.push_str(&format!("&duration={}", duration.round()));
    }
    if let Some(lyrics) = synced(&ask(&path)?) {
        return Ok(Some(lyrics));
    }
    // A video title often names the artist itself: "Sheila On 7 - Dan",
    // posted by a channel with another name.
    let query = if title.contains(" - ") {
        title
    } else {
        format!("{artist} {title}")
    };
    let results = ask(&format!("/api/search?q={}", percent_encode(query.trim())))?;
    Ok(results
        .as_array()
        .and_then(|results| results.iter().find_map(synced)))
}

/// The server could not answer (offline, or too busy): worth asking again.
struct Unanswered;

/// A JSON answer from LRCLIB; "not found" is an answer too.
fn ask(path: &str) -> Result<serde_json::Value, Unanswered> {
    let body = net::get(HOST, path).ok_or(Unanswered)?;
    let answer: serde_json::Value = serde_json::from_slice(&body).map_err(|_| Unanswered)?;
    if answer["statusCode"]
        .as_u64()
        .is_some_and(|code| code >= 500)
    {
        return Err(Unanswered);
    }
    Ok(answer)
}

fn synced(record: &serde_json::Value) -> Option<Lyrics> {
    parse_lrc(record["syncedLyrics"].as_str()?)
}

/// Strips what video sites add to song names: "(Official Video)",
/// "[Lyrics]", an " - Topic" channel.
fn clean(artist: &str, title: &str) -> (String, String) {
    let mut title = title.to_string();
    for (open, close) in [('(', ')'), ('[', ']')] {
        while let (Some(start), Some(end)) = (title.find(open), title.find(close)) {
            if end < start {
                break;
            }
            let inside = title[start + 1..end].to_lowercase();
            let noise = [
                "official",
                "lyric",
                "video",
                "audio",
                "mv",
                "visualizer",
                "hd",
                "4k",
            ]
            .iter()
            .any(|word| inside.contains(word));
            if !noise {
                break;
            }
            title.replace_range(start..=end, "");
        }
    }
    let artist = artist.trim().trim_end_matches(" - Topic").to_string();
    (
        artist,
        title.split_whitespace().collect::<Vec<_>>().join(" "),
    )
}

/// Reads LRC text: `[01:02.34]A line`, several stamps on one line allowed;
/// tags such as `[ar:Artist]` are skipped.
fn parse_lrc(text: &str) -> Option<Lyrics> {
    let mut lines = Vec::new();
    for line in text.lines() {
        let mut rest = line.trim();
        let mut stamps = Vec::new();
        while let Some(stripped) = rest.strip_prefix('[') {
            let Some((stamp, after)) = stripped.split_once(']') else {
                break;
            };
            match seconds(stamp) {
                Some(at) => stamps.push(at),
                None => break,
            }
            rest = after;
        }
        for at in stamps {
            lines.push((at, rest.trim().to_string()));
        }
    }
    lines.sort_by(|a, b| a.0.total_cmp(&b.0));
    (!lines.is_empty()).then_some(Lyrics { lines })
}

/// "01:02.34" as seconds.
fn seconds(stamp: &str) -> Option<f64> {
    let (minutes, seconds) = stamp.split_once(':')?;
    let minutes: f64 = minutes.parse().ok()?;
    let seconds: f64 = seconds.parse().ok()?;
    Some(minutes * 60.0 + seconds)
}

fn post(target: isize, found: Found) {
    let payload = Box::into_raw(Box::new(found));
    unsafe {
        let posted = PostMessageW(
            Some(HWND(target as *mut c_void)),
            LYRICS_MSG,
            WPARAM(0),
            LPARAM(payload as isize),
        );
        if posted.is_err() {
            drop(Box::from_raw(payload));
        }
    }
}

/// Takes ownership of the answer carried by a [`LYRICS_MSG`].
///
/// # Safety
/// `lparam` must come from a [`LYRICS_MSG`] and be taken only once.
pub unsafe fn take_found(lparam: LPARAM) -> Found {
    *Box::from_raw(lparam.0 as *mut Found)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_lrc_with_tags_and_repeated_lines() {
        let lyrics = parse_lrc(
            "[ar:Penyanyi]\n[00:12.50]Baris satu\n[00:20.00][00:40.00]Reff\n[00:30.00]\n",
        )
        .unwrap();
        assert_eq!(lyrics.at(5.0), ("", "Baris satu"));
        assert_eq!(lyrics.at(12.4), ("Baris satu", "Reff"));
        assert_eq!(lyrics.at(25.0), ("Reff", ""));
        assert_eq!(lyrics.at(31.0), ("", "Reff"));
        assert_eq!(lyrics.at(99.0), ("Reff", ""));
    }

    #[test]
    fn cleans_video_titles() {
        assert_eq!(
            clean("Tulus", "Hati-Hati di Jalan (Official Music Video)"),
            ("Tulus".to_string(), "Hati-Hati di Jalan".to_string())
        );
        assert_eq!(
            clean("NOAH - Topic", "Separuh Aku [Lyric Video]"),
            ("NOAH".to_string(), "Separuh Aku".to_string())
        );
        assert_eq!(
            clean("Some Channel", "Sheila On 7 - Dan (Official Audio)").1,
            "Sheila On 7 - Dan"
        );
        // Parentheses that are part of the name stay.
        assert_eq!(clean("A", "Lagu (Remix)").1, "Lagu (Remix)");
    }
}
