//! Files dragged onto the notch (OLE drop target), and shelf files dragged
//! out of it to other apps (OLE drag source).

use std::ffi::c_void;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};

use windows::core::{implement, Ref, Result, BOOL, HRESULT, HSTRING};
use windows::Win32::Foundation::{
    DRAGDROP_S_CANCEL, DRAGDROP_S_DROP, DRAGDROP_S_USEDEFAULTCURSORS, HWND, LPARAM, POINTL, S_OK,
    WPARAM,
};
use windows::Win32::System::Com::{IDataObject, DVASPECT_CONTENT, FORMATETC, TYMED_HGLOBAL};
use windows::Win32::System::Ole::{
    DoDragDrop, IDropSource, IDropSource_Impl, IDropTarget, IDropTarget_Impl, RegisterDragDrop,
    ReleaseStgMedium, CF_HDROP, DROPEFFECT, DROPEFFECT_COPY, DROPEFFECT_LINK, DROPEFFECT_NONE,
};
use windows::Win32::System::SystemServices::{MK_LBUTTON, MODIFIERKEYS_FLAGS};
use windows::Win32::UI::Shell::{
    BHID_DataObject, DragQueryFileW, IShellItem, SHCreateItemFromParsingName, HDROP,
};
use windows::Win32::UI::WindowsAndMessaging::{PostMessageW, WM_APP};

/// Files are being dragged over the notch. WPARAM: 1 entering, 0 leaving.
pub const DRAG_MSG: u32 = WM_APP + 10;
/// Files were dropped. LPARAM: a `Box<Vec<PathBuf>>`.
pub const DROP_MSG: u32 = WM_APP + 11;

/// Set while a shelf file is being dragged out, so the notch does not offer
/// to take it back.
static DRAGGING_OUT: AtomicBool = AtomicBool::new(false);
/// Cleared while the shelf module is switched off.
static ACCEPTING: AtomicBool = AtomicBool::new(true);

/// Whether files dragged onto the notch are taken (the shelf is on).
pub fn set_accepting(on: bool) {
    ACCEPTING.store(on, Ordering::Relaxed);
}

/// Files may be dropped: the shelf is on and not dragging one of its own.
fn taking() -> bool {
    ACCEPTING.load(Ordering::Relaxed) && !DRAGGING_OUT.load(Ordering::Relaxed)
}

#[implement(IDropTarget)]
struct DropTarget {
    hwnd: isize,
}

impl DropTarget {
    fn post(&self, msg: u32, wparam: usize, lparam: isize) {
        unsafe {
            let _ = PostMessageW(
                Some(HWND(self.hwnd as *mut c_void)),
                msg,
                WPARAM(wparam),
                LPARAM(lparam),
            );
        }
    }
}

impl IDropTarget_Impl for DropTarget_Impl {
    fn DragEnter(
        &self,
        data: Ref<'_, IDataObject>,
        _keys: MODIFIERKEYS_FLAGS,
        _point: &POINTL,
        effect: *mut DROPEFFECT,
    ) -> Result<()> {
        let files = taking()
            && data
                .as_ref()
                .is_some_and(|data| unsafe { data.QueryGetData(&hdrop_format()) } == S_OK);
        unsafe {
            *effect = if files {
                choose_effect(*effect)
            } else {
                DROPEFFECT_NONE
            }
        };
        if files {
            self.post(DRAG_MSG, 1, 0);
        }
        Ok(())
    }

    fn DragOver(
        &self,
        _keys: MODIFIERKEYS_FLAGS,
        _point: &POINTL,
        effect: *mut DROPEFFECT,
    ) -> Result<()> {
        unsafe {
            *effect = if taking() {
                choose_effect(*effect)
            } else {
                DROPEFFECT_NONE
            };
        }
        Ok(())
    }

    fn DragLeave(&self) -> Result<()> {
        self.post(DRAG_MSG, 0, 0);
        Ok(())
    }

    fn Drop(
        &self,
        data: Ref<'_, IDataObject>,
        _keys: MODIFIERKEYS_FLAGS,
        _point: &POINTL,
        effect: *mut DROPEFFECT,
    ) -> Result<()> {
        self.post(DRAG_MSG, 0, 0);
        let paths = if taking() {
            data.as_ref().map(read_paths).unwrap_or_default()
        } else {
            Vec::new()
        };
        unsafe {
            *effect = if paths.is_empty() {
                DROPEFFECT_NONE
            } else {
                choose_effect(*effect)
            }
        };
        if !paths.is_empty() {
            let payload = Box::into_raw(Box::new(paths));
            self.post(DROP_MSG, 0, payload as isize);
        }
        Ok(())
    }
}

/// The shelf keeps a reference, so the source must not move the file: copy
/// when it allows that, else link.
fn choose_effect(allowed: DROPEFFECT) -> DROPEFFECT {
    if allowed.0 & DROPEFFECT_COPY.0 != 0 {
        DROPEFFECT_COPY
    } else if allowed.0 & DROPEFFECT_LINK.0 != 0 {
        DROPEFFECT_LINK
    } else {
        DROPEFFECT_NONE
    }
}

fn hdrop_format() -> FORMATETC {
    FORMATETC {
        cfFormat: CF_HDROP.0,
        ptd: std::ptr::null_mut(),
        dwAspect: DVASPECT_CONTENT.0,
        lindex: -1,
        tymed: TYMED_HGLOBAL.0 as u32,
    }
}

/// File paths in a dragged data object.
fn read_paths(data: &IDataObject) -> Vec<PathBuf> {
    unsafe {
        let Ok(mut medium) = data.GetData(&hdrop_format()) else {
            return Vec::new();
        };
        let drop = HDROP(medium.u.hGlobal.0);
        let count = DragQueryFileW(drop, u32::MAX, None);
        let mut paths = Vec::new();
        for index in 0..count {
            let length = DragQueryFileW(drop, index, None) as usize;
            let mut buffer = vec![0u16; length + 1];
            let copied = DragQueryFileW(drop, index, Some(&mut buffer)) as usize;
            paths.push(PathBuf::from(String::from_utf16_lossy(&buffer[..copied])));
        }
        ReleaseStgMedium(&mut medium);
        paths
    }
}

/// Makes the notch accept dropped files. Needs OLE initialized.
pub fn accept_drops(hwnd: HWND) {
    let target: IDropTarget = DropTarget {
        hwnd: hwnd.0 as isize,
    }
    .into();
    unsafe {
        let _ = RegisterDragDrop(hwnd, &target);
    }
}

#[implement(IDropSource)]
struct DropSource;

impl IDropSource_Impl for DropSource_Impl {
    fn QueryContinueDrag(&self, escape: BOOL, keys: MODIFIERKEYS_FLAGS) -> HRESULT {
        if escape.as_bool() {
            DRAGDROP_S_CANCEL
        } else if keys.0 & MK_LBUTTON.0 == 0 {
            DRAGDROP_S_DROP
        } else {
            S_OK
        }
    }

    fn GiveFeedback(&self, _effect: DROPEFFECT) -> HRESULT {
        DRAGDROP_S_USEDEFAULTCURSORS
    }
}

/// Drags `path` out to wherever the user drops it, as Explorer would. Runs a
/// modal loop until the drop, so it must not be called while app state is
/// borrowed.
pub fn drag_out(path: &Path) {
    unsafe {
        let Ok(item) =
            SHCreateItemFromParsingName::<_, _, IShellItem>(&HSTRING::from(path.as_os_str()), None)
        else {
            return;
        };
        let Ok(data) = item.BindToHandler::<_, IDataObject>(None, &BHID_DataObject) else {
            return;
        };
        let source: IDropSource = DropSource.into();
        let mut effect = DROPEFFECT_NONE;
        DRAGGING_OUT.store(true, Ordering::Relaxed);
        let _ = DoDragDrop(
            &data,
            &source,
            DROPEFFECT_COPY | DROPEFFECT_LINK,
            &mut effect,
        );
        DRAGGING_OUT.store(false, Ordering::Relaxed);
    }
}

/// Takes ownership of the paths carried by a [`DROP_MSG`].
///
/// # Safety
/// `lparam` must come from a [`DROP_MSG`] and be taken only once.
pub unsafe fn take_paths(lparam: LPARAM) -> Vec<PathBuf> {
    *Box::from_raw(lparam.0 as *mut Vec<PathBuf>)
}
