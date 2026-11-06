//! Toasts from other apps, such as WhatsApp, read from the Windows
//! notification center.
//!
//! The listener's change event needs a packaged app, so a worker thread sleeps
//! on the folder of the notification database instead: Windows writes there
//! the moment a toast arrives, and only then is the list read.

use std::cmp::Reverse;
use std::collections::{HashMap, HashSet};
use std::ffi::c_void;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use windows::core::{Interface, Result};
use windows::Foundation::Size;
use windows::Win32::Foundation::{HWND, LPARAM, WPARAM};
use windows::Win32::Storage::FileSystem::{FILE_NOTIFY_CHANGE_LAST_WRITE, FILE_NOTIFY_CHANGE_SIZE};
use windows::Win32::System::Com::{CoInitializeEx, COINIT_MULTITHREADED};
use windows::Win32::UI::WindowsAndMessaging::{PostMessageW, WM_APP};
use windows::UI::Notifications::Management::{
    UserNotificationListener, UserNotificationListenerAccessStatus as Access,
};
use windows::UI::Notifications::{KnownNotificationBindings, NotificationKinds, UserNotification};

use crate::lang::tr;
use crate::media;
use crate::watch::Folder;

/// The notification center changed. LPARAM: a `Box<Inbox>`.
pub const INBOX_MSG: u32 = WM_APP + 16;
/// The database is written several times per toast; these are read as one.
const SETTLE: Duration = Duration::from_millis(150);
/// Rows on the notification card.
pub const ROWS: usize = 3;
/// A title or message is kept up to this many characters.
const TEXT_LIMIT: usize = 120;
/// Logos are asked for at this size.
const LOGO_PIXELS: f32 = 96.0;
/// Windows counts time in ticks of 100 ns from 1601.
const TICKS_PER_SECOND: i64 = 10_000_000;
const SECONDS_TO_1970: i64 = 11_644_473_600;

#[derive(Clone, Debug, Default, PartialEq)]
pub struct AppNotice {
    /// The toast's number in the notification center.
    pub id: u32,
    /// Who it is from, or the app when the toast has no sender.
    pub title: String,
    pub app: String,
    /// The app's identity in Windows, which opens it.
    pub app_id: String,
    pub body: String,
    /// When it arrived, in seconds since 1970.
    pub created: i64,
    /// The app's logo, encoded, when it was read for showing.
    pub logo: Option<Arc<Vec<u8>>>,
}

/// What the notification center holds now.
#[derive(Default)]
pub struct Inbox {
    /// The newest first, at most [`ROWS`].
    pub items: Vec<AppNotice>,
    /// How many there are in all.
    pub total: usize,
    /// The first item has just arrived.
    pub fresh: bool,
}

/// Reads the toast's lines: a sender over the message, or the message alone.
fn notice(app: String, texts: Vec<String>) -> Option<AppNotice> {
    let mut lines = texts.into_iter().filter(|text| !text.trim().is_empty());
    let first = lines.next()?;
    let rest: Vec<String> = lines.collect();
    let (title, body) = if rest.is_empty() {
        (app.clone(), first)
    } else {
        (first, rest.join(" "))
    };
    Some(AppNotice {
        title: one_line(&title, TEXT_LIMIT),
        body: one_line(&body, TEXT_LIMIT),
        app,
        ..Default::default()
    })
}

pub fn now_seconds() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |since| since.as_secs() as i64)
}

/// How long ago, for a notification's row.
pub fn age_label(seconds: i64) -> String {
    match seconds / 60 {
        ..=0 => tr("Baru", "New").to_string(),
        minutes @ 1..=59 => format!("{minutes} {}", tr("mnt", "min")),
        minutes @ 60..=1439 => format!("{} {}", minutes / 60, tr("jam", "h")),
        minutes => format!("{} {}", minutes / 1440, tr("hari", "d")),
    }
}

/// Snipping Tool's toast is replaced by the shelf's own notice.
fn is_hidden(app_id: &str) -> bool {
    app_id.to_lowercase().contains("screensketch")
}

/// `text` on one line, cut to `limit` characters.
fn one_line(text: &str, limit: usize) -> String {
    let joined = text.split_whitespace().collect::<Vec<_>>().join(" ");
    if joined.chars().count() <= limit {
        return joined;
    }
    let cut: String = joined.chars().take(limit).collect();
    format!("{}…", cut.trim_end())
}

fn read(toast: &UserNotification) -> Option<AppNotice> {
    let info = toast.AppInfo().ok();
    let app_id = info
        .as_ref()
        .and_then(|info| info.AppUserModelId().ok())
        .map(|id| id.to_string())
        .unwrap_or_default();
    if is_hidden(&app_id) {
        return None;
    }
    let app = info
        .and_then(|info| info.DisplayInfo().ok()?.DisplayName().ok())
        .map(|name| name.to_string())
        .unwrap_or_default();
    let created = toast.CreationTime().map_or(0, |time| {
        time.UniversalTime / TICKS_PER_SECOND - SECONDS_TO_1970
    });
    let texts = texts(toast).ok()?;
    Some(AppNotice {
        id: toast.Id().ok()?,
        app_id,
        created,
        ..notice(app, texts)?
    })
}

fn read_logo(toast: &UserNotification) -> Option<Arc<Vec<u8>>> {
    let size = Size {
        Width: LOGO_PIXELS,
        Height: LOGO_PIXELS,
    };
    let logo = toast
        .AppInfo()
        .ok()?
        .DisplayInfo()
        .ok()?
        .GetLogo(size)
        .ok()?;
    media::read_image(&logo.cast().ok()?).map(Arc::new)
}

/// Removes one toast from the Windows notification center.
pub fn remove(id: u32) {
    if let Ok(listener) = UserNotificationListener::Current() {
        let _ = listener.RemoveNotification(id);
    }
}

/// Empties the Windows notification center.
pub fn clear() {
    if let Ok(listener) = UserNotificationListener::Current() {
        let _ = listener.ClearNotifications();
    }
}

fn texts(toast: &UserNotification) -> Result<Vec<String>> {
    let binding = toast
        .Notification()?
        .Visual()?
        .GetBinding(&KnownNotificationBindings::ToastGeneric()?)?;
    Ok(binding
        .GetTextElements()?
        .into_iter()
        .filter_map(|element| element.Text().ok())
        .map(|text| text.to_string())
        .collect())
}

fn database_folder() -> Option<PathBuf> {
    let local = std::env::var_os("LOCALAPPDATA")?;
    Some(PathBuf::from(local).join(r"Microsoft\Windows\Notifications"))
}

fn allowed(listener: &UserNotificationListener) -> Result<bool> {
    let mut access = listener.GetAccessStatus()?;
    if access == Access::Unspecified {
        access = listener.RequestAccessAsync()?.get()?;
    }
    Ok(access == Access::Allowed)
}

/// Follows the notification center on its own thread, posting an [`Inbox`]
/// at the start and whenever its toasts change.
pub fn watch(hwnd: HWND) {
    let target = hwnd.0 as isize;
    std::thread::spawn(move || unsafe {
        let _ = CoInitializeEx(None, COINIT_MULTITHREADED);
        let Ok(listener) = UserNotificationListener::Current() else {
            return;
        };
        if !allowed(&listener).unwrap_or(false) {
            return;
        }
        let Some(folder) = database_folder().and_then(|path| Folder::open(&path).ok()) else {
            return;
        };
        let mut known: Option<HashSet<u32>> = None;
        let mut logos: HashMap<String, Arc<Vec<u8>>> = HashMap::new();
        loop {
            if let Ok(toasts) = listener
                .GetNotificationsAsync(NotificationKinds::Toast)
                .and_then(|list| list.get())
            {
                let mut toasts: Vec<(u32, UserNotification)> = toasts
                    .into_iter()
                    .filter_map(|toast| Some((toast.Id().ok()?, toast)))
                    .collect();
                toasts.sort_by_key(|(id, _)| Reverse(*id));
                let ids: HashSet<u32> = toasts.iter().map(|(id, _)| *id).collect();
                if known.as_ref() != Some(&ids) {
                    let shown: Vec<(u32, AppNotice)> = toasts
                        .iter()
                        .filter_map(|(id, toast)| Some((*id, read(toast)?)))
                        .collect();
                    let fresh = shown.first().is_some_and(|(id, _)| {
                        known.as_ref().is_some_and(|known| !known.contains(id))
                    });
                    let total = shown.len();
                    let items = shown.into_iter().take(ROWS).map(|(id, mut item)| {
                        let toast = toasts.iter().find(|(known, _)| *known == id);
                        // Only a logo that was read is kept: a failed read
                        // gets another try with the next change.
                        item.logo = logos.get(&item.app_id).cloned().or_else(|| {
                            let logo = toast.and_then(|(_, toast)| read_logo(toast))?;
                            logos.insert(item.app_id.clone(), logo.clone());
                            Some(logo)
                        });
                        item
                    });
                    let payload = Box::into_raw(Box::new(Inbox {
                        items: items.collect(),
                        total,
                        fresh,
                    }));
                    let _ = PostMessageW(
                        Some(HWND(target as *mut c_void)),
                        INBOX_MSG,
                        WPARAM(0),
                        LPARAM(payload as isize),
                    );
                    known = Some(ids);
                }
            }
            let filter = FILE_NOTIFY_CHANGE_LAST_WRITE | FILE_NOTIFY_CHANGE_SIZE;
            if folder.wait(filter).is_err() {
                return;
            }
            std::thread::sleep(SETTLE);
        }
    });
}

/// Takes ownership of the inbox carried by an [`INBOX_MSG`].
///
/// # Safety
/// `lparam` must come from an [`INBOX_MSG`] and be taken only once.
pub unsafe fn take_inbox(lparam: LPARAM) -> Inbox {
    *Box::from_raw(lparam.0 as *mut Inbox)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn lines(texts: &[&str]) -> Vec<String> {
        texts.iter().map(|text| text.to_string()).collect()
    }

    #[test]
    fn sender_over_message() {
        let read = notice("WhatsApp".into(), lines(&["Andi", "Halo", "Sudah sampai?"]));
        assert_eq!(
            read,
            Some(AppNotice {
                title: "Andi".into(),
                app: "WhatsApp".into(),
                body: "Halo Sudah sampai?".into(),
                ..Default::default()
            })
        );
    }

    #[test]
    fn a_lone_line_goes_under_the_app_name() {
        let read = notice("Outlook".into(), lines(&["  ", "Pesan baru"])).unwrap();
        assert_eq!(read.title, "Outlook");
        assert_eq!(read.body, "Pesan baru");
        assert_eq!(read.app, "Outlook");
    }

    #[test]
    fn nothing_to_show_without_text() {
        assert_eq!(notice("App".into(), lines(&["", " "])), None);
    }

    #[test]
    fn snipping_tool_stays_out() {
        assert!(is_hidden("Microsoft.ScreenSketch_8wekyb3d8bbwe!App"));
        assert!(!is_hidden("5319275A.WhatsAppDesktop_cv1g1gvanyjgm!App"));
    }

    #[test]
    fn ages_read_in_the_largest_unit() {
        assert_eq!(age_label(30), "Baru");
        assert_eq!(age_label(300), "5 mnt");
        assert_eq!(age_label(7300), "2 jam");
        assert_eq!(age_label(90_000), "1 hari");
    }

    #[test]
    fn long_text_is_cut_to_one_line() {
        assert_eq!(one_line("satu\n  dua", 20), "satu dua");
        assert_eq!(one_line("abcdefgh", 4), "abcd…");
    }
}
