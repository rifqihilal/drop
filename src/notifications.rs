//! Toasts from other apps, such as WhatsApp, read from the Windows
//! notification center.
//!
//! The listener's change event needs a packaged app, so a worker thread sleeps
//! on the folder of the notification database instead: Windows writes there
//! the moment a toast arrives, and only then is the list read.

use std::collections::HashSet;
use std::ffi::c_void;
use std::path::PathBuf;
use std::time::Duration;

use windows::core::Result;
use windows::Win32::Foundation::{HWND, LPARAM, WPARAM};
use windows::Win32::Storage::FileSystem::{FILE_NOTIFY_CHANGE_LAST_WRITE, FILE_NOTIFY_CHANGE_SIZE};
use windows::Win32::System::Com::{CoInitializeEx, COINIT_MULTITHREADED};
use windows::Win32::UI::WindowsAndMessaging::{PostMessageW, WM_APP};
use windows::UI::Notifications::Management::{
    UserNotificationListener, UserNotificationListenerAccessStatus as Access,
};
use windows::UI::Notifications::{KnownNotificationBindings, NotificationKinds, UserNotification};

use crate::watch::Folder;

/// A toast arrived. LPARAM: a `Box<AppNotice>`.
pub const APP_NOTICE_MSG: u32 = WM_APP + 16;
/// The database is written several times per toast; these are read as one.
const SETTLE: Duration = Duration::from_millis(150);

#[derive(Debug, PartialEq)]
pub struct AppNotice {
    /// Who it is from, or the app when the toast has no sender.
    pub title: String,
    pub app: String,
    pub body: String,
}

/// Reads the toast's lines: a sender over the message, or the message alone.
fn notice(app: String, texts: Vec<String>) -> Option<AppNotice> {
    let mut lines = texts.into_iter().filter(|text| !text.trim().is_empty());
    let first = lines.next()?;
    let rest: Vec<String> = lines.collect();
    Some(if rest.is_empty() {
        AppNotice {
            title: app.clone(),
            body: first,
            app: String::new(),
        }
    } else {
        AppNotice {
            title: first,
            body: rest.join(" "),
            app,
        }
    })
}

/// Snipping Tool's toast is replaced by the shelf's own notice.
fn is_hidden(app_id: &str) -> bool {
    app_id.to_lowercase().contains("screensketch")
}

/// `text` on one line, cut to `limit` characters.
pub fn one_line(text: &str, limit: usize) -> String {
    let joined = text.split_whitespace().collect::<Vec<_>>().join(" ");
    if joined.chars().count() <= limit {
        return joined;
    }
    let cut: String = joined.chars().take(limit).collect();
    format!("{}…", cut.trim_end())
}

fn read(toast: &UserNotification) -> Option<AppNotice> {
    let info = toast.AppInfo().ok();
    let app_id = info.as_ref().and_then(|info| info.AppUserModelId().ok());
    if app_id.is_some_and(|id| is_hidden(&id.to_string())) {
        return None;
    }
    let app = info
        .and_then(|info| info.DisplayInfo().ok()?.DisplayName().ok())
        .map(|name| name.to_string())
        .unwrap_or_default();
    let texts = texts(toast).ok()?;
    notice(app, texts)
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

/// Follows the notification center on its own thread. Only toasts that arrive
/// after this starts are reported, the newest of each batch.
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
        loop {
            if let Ok(toasts) = listener
                .GetNotificationsAsync(NotificationKinds::Toast)
                .and_then(|list| list.get())
            {
                let toasts: Vec<(u32, UserNotification)> = toasts
                    .into_iter()
                    .filter_map(|toast| Some((toast.Id().ok()?, toast)))
                    .collect();
                if let Some(known) = &known {
                    let newest = toasts
                        .iter()
                        .filter(|(id, _)| !known.contains(id))
                        .max_by_key(|(id, _)| *id);
                    if let Some(notice) = newest.and_then(|(_, toast)| read(toast)) {
                        let payload = Box::into_raw(Box::new(notice));
                        let _ = PostMessageW(
                            Some(HWND(target as *mut c_void)),
                            APP_NOTICE_MSG,
                            WPARAM(0),
                            LPARAM(payload as isize),
                        );
                    }
                }
                known = Some(toasts.iter().map(|(id, _)| *id).collect());
            }
            let filter = FILE_NOTIFY_CHANGE_LAST_WRITE | FILE_NOTIFY_CHANGE_SIZE;
            if folder.wait(filter).is_err() {
                return;
            }
            std::thread::sleep(SETTLE);
        }
    });
}

/// Takes ownership of the notice carried by an [`APP_NOTICE_MSG`].
///
/// # Safety
/// `lparam` must come from an [`APP_NOTICE_MSG`] and be taken only once.
pub unsafe fn take_notice(lparam: LPARAM) -> AppNotice {
    *Box::from_raw(lparam.0 as *mut AppNotice)
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
            })
        );
    }

    #[test]
    fn a_lone_line_goes_under_the_app_name() {
        let read = notice("Outlook".into(), lines(&["  ", "Pesan baru"])).unwrap();
        assert_eq!(read.title, "Outlook");
        assert_eq!(read.body, "Pesan baru");
        assert_eq!(read.app, "");
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
    fn long_text_is_cut_to_one_line() {
        assert_eq!(one_line("satu\n  dua", 20), "satu dua");
        assert_eq!(one_line("abcdefgh", 4), "abcd…");
    }
}
