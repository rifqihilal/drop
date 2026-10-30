//! What is playing, from Windows' media sessions (the same source as the
//! media flyout), and the play/pause/next/previous controls.
//!
//! A worker thread owns the WinRT objects: session events wake it, it reads
//! the track and posts a [`Track`] to the notch window, so the UI thread never
//! waits on WinRT calls.

use std::ffi::c_void;
use std::sync::mpsc::{channel, RecvTimeoutError, Sender};
use std::sync::Arc;
use std::time::{Duration, Instant};

use windows::core::{Result, RuntimeType};
use windows::Foundation::TypedEventHandler;
use windows::Media::Control::{
    GlobalSystemMediaTransportControlsSession as Session,
    GlobalSystemMediaTransportControlsSessionManager as Manager,
    GlobalSystemMediaTransportControlsSessionPlaybackStatus as Status,
};
use windows::Storage::Streams::DataReader;
use windows::Win32::Foundation::{HWND, LPARAM, WPARAM};
use windows::Win32::System::Com::{CoInitializeEx, COINIT_MULTITHREADED};
use windows::Win32::UI::WindowsAndMessaging::{PostMessageW, WM_APP};

/// The track changed. LPARAM: a `Box<Option<Track>>`; None when nothing plays.
pub const MEDIA_MSG: u32 = WM_APP + 8;
/// Sessions that forget to signal a change are caught by this re-check.
const SAFETY_REFRESH: Duration = Duration::from_secs(10);
/// Several events usually arrive together; they are handled as one.
const SETTLE: Duration = Duration::from_millis(120);

#[derive(Clone)]
pub struct Track {
    pub title: String,
    pub artist: String,
    pub playing: bool,
    /// Seconds into the track when this was read.
    pub position: f64,
    /// Track length in seconds; 0 when the player does not say.
    pub duration: f64,
    pub read_at: Instant,
    /// Encoded album art (JPEG/PNG), shared between updates of one track.
    pub art: Option<Arc<Vec<u8>>>,
}

impl Track {
    /// Same song, so the art and the "now playing" moment carry over.
    pub fn same_song(&self, other: &Track) -> bool {
        self.title == other.title && self.artist == other.artist
    }

    /// Position now, advanced since it was read while playing.
    pub fn position_now(&self) -> f64 {
        let elapsed = if self.playing {
            self.read_at.elapsed().as_secs_f64()
        } else {
            0.0
        };
        let position = self.position + elapsed;
        if self.duration > 0.0 {
            position.min(self.duration)
        } else {
            position
        }
    }
}

#[derive(Clone, Copy)]
pub enum Command {
    TogglePlay,
    Next,
    Previous,
}

enum Event {
    Refresh,
    Command(Command),
}

pub struct Media {
    events: Sender<Event>,
}

impl Media {
    pub fn start(hwnd: HWND) -> Self {
        let (events, inbox) = channel();
        let wake = events.clone();
        let target = hwnd.0 as isize;
        std::thread::spawn(move || unsafe {
            let _ = CoInitializeEx(None, COINIT_MULTITHREADED);
            let Ok(manager) = Manager::RequestAsync().and_then(|op| op.get()) else {
                return;
            };
            let refresh = {
                let wake = wake.clone();
                move || {
                    let _ = wake.send(Event::Refresh);
                }
            };
            let _ = manager.CurrentSessionChanged(&waker(refresh.clone()));
            let _ = manager.SessionsChanged(&waker(refresh.clone()));

            let mut watched: Option<Watched> = None;
            let mut last: Option<Track> = None;
            let mut event = Ok(Event::Refresh);
            loop {
                match event {
                    Ok(Event::Command(command)) => {
                        if let Some(watched) = &watched {
                            send_command(&watched.session, command);
                        }
                    }
                    Ok(Event::Refresh) | Err(RecvTimeoutError::Timeout) => {
                        // Let a burst of events settle, then read once.
                        while let Ok(Event::Refresh) = inbox.recv_timeout(SETTLE) {}
                        let session = pick_session(&manager);
                        if !watched
                            .as_ref()
                            .is_some_and(|w| Some(&w.session) == session.as_ref())
                        {
                            watched =
                                session.and_then(|session| Watched::new(session, &refresh).ok());
                        }
                        let track = watched
                            .as_ref()
                            .and_then(|w| read_track(&w.session, last.as_ref()));
                        last.clone_from(&track);
                        post(target, track);
                    }
                    Err(RecvTimeoutError::Disconnected) => break,
                }
                event = inbox.recv_timeout(SAFETY_REFRESH);
            }
        });
        Self { events }
    }

    pub fn send(&self, command: Command) {
        let _ = self.events.send(Event::Command(command));
    }
}

/// A session whose change events wake the worker; unsubscribes when dropped.
struct Watched {
    session: Session,
    tokens: [i64; 3],
}

impl Watched {
    fn new(
        session: Session,
        refresh: &(impl Fn() + Clone + Send + Sync + 'static),
    ) -> Result<Self> {
        let tokens = [
            session.PlaybackInfoChanged(&waker(refresh.clone()))?,
            session.MediaPropertiesChanged(&waker(refresh.clone()))?,
            session.TimelinePropertiesChanged(&waker(refresh.clone()))?,
        ];
        Ok(Self { session, tokens })
    }
}

impl Drop for Watched {
    fn drop(&mut self) {
        let _ = self.session.RemovePlaybackInfoChanged(self.tokens[0]);
        let _ = self.session.RemoveMediaPropertiesChanged(self.tokens[1]);
        let _ = self.session.RemoveTimelinePropertiesChanged(self.tokens[2]);
    }
}

/// An event handler that only wakes the worker.
fn waker<Sender: RuntimeType + 'static, Args: RuntimeType + 'static>(
    refresh: impl Fn() + Send + Sync + 'static,
) -> TypedEventHandler<Sender, Args> {
    TypedEventHandler::new(move |_, _| {
        refresh();
        Ok(())
    })
}

/// A session that is playing, else the one Windows considers current.
fn pick_session(manager: &Manager) -> Option<Session> {
    let playing = manager.GetSessions().ok().and_then(|sessions| {
        sessions.into_iter().find(|session| {
            session
                .GetPlaybackInfo()
                .and_then(|info| info.PlaybackStatus())
                .is_ok_and(|status| status == Status::Playing)
        })
    });
    playing.or_else(|| manager.GetCurrentSession().ok())
}

fn read_track(session: &Session, last: Option<&Track>) -> Option<Track> {
    let properties = session.TryGetMediaPropertiesAsync().ok()?.get().ok()?;
    let title = properties.Title().ok()?.to_string();
    if title.trim().is_empty() {
        return None;
    }
    let artist = properties
        .Artist()
        .map(|a| a.to_string())
        .unwrap_or_default();
    let playing = session
        .GetPlaybackInfo()
        .and_then(|info| info.PlaybackStatus())
        .is_ok_and(|status| status == Status::Playing);
    // Timeline values are in 100 ns ticks.
    let seconds = |ticks: i64| ticks as f64 / 10_000_000.0;
    let (position, duration) = session
        .GetTimelineProperties()
        .map(|timeline| {
            let start = timeline.StartTime().map(|t| t.Duration).unwrap_or(0);
            let end = timeline.EndTime().map(|t| t.Duration).unwrap_or(0);
            let position = timeline.Position().map(|t| t.Duration).unwrap_or(0);
            (
                seconds(position - start).max(0.0),
                seconds(end - start).max(0.0),
            )
        })
        .unwrap_or((0.0, 0.0));

    let mut track = Track {
        title,
        artist,
        playing,
        position,
        duration,
        read_at: Instant::now(),
        art: None,
    };
    // Album art is only fetched once per song.
    track.art = match last.filter(|last| last.same_song(&track)) {
        Some(last) if last.art.is_some() => last.art.clone(),
        _ => read_art(&properties).map(Arc::new),
    };
    Some(track)
}

fn read_art(
    properties: &windows::Media::Control::GlobalSystemMediaTransportControlsSessionMediaProperties,
) -> Option<Vec<u8>> {
    let stream = properties
        .Thumbnail()
        .ok()?
        .OpenReadAsync()
        .ok()?
        .get()
        .ok()?;
    let reader = DataReader::CreateDataReader(&stream).ok()?;
    let mut bytes = Vec::new();
    loop {
        let loaded = reader.LoadAsync(64 * 1024).ok()?.get().ok()?;
        if loaded == 0 {
            break;
        }
        let start = bytes.len();
        bytes.resize(start + loaded as usize, 0);
        reader.ReadBytes(&mut bytes[start..]).ok()?;
    }
    (!bytes.is_empty()).then_some(bytes)
}

fn send_command(session: &Session, command: Command) {
    let _ = match command {
        Command::TogglePlay => session.TryTogglePlayPauseAsync().and_then(|op| op.get()),
        Command::Next => session.TrySkipNextAsync().and_then(|op| op.get()),
        Command::Previous => session.TrySkipPreviousAsync().and_then(|op| op.get()),
    };
}

fn post(target: isize, track: Option<Track>) {
    let payload = Box::into_raw(Box::new(track));
    unsafe {
        let posted = PostMessageW(
            Some(HWND(target as *mut c_void)),
            MEDIA_MSG,
            WPARAM(0),
            LPARAM(payload as isize),
        );
        if posted.is_err() {
            drop(Box::from_raw(payload));
        }
    }
}

/// Takes ownership of the track carried by a [`MEDIA_MSG`].
///
/// # Safety
/// `lparam` must come from a [`MEDIA_MSG`] and be taken only once.
pub unsafe fn take_track(lparam: LPARAM) -> Option<Track> {
    *Box::from_raw(lparam.0 as *mut Option<Track>)
}
