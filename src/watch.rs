//! Sleeps until a folder changes, so its watchers cost nothing in between.

use std::path::Path;

use windows::core::{Result, HSTRING};
use windows::Win32::Foundation::{CloseHandle, HANDLE};
use windows::Win32::Storage::FileSystem::{
    CreateFileW, ReadDirectoryChangesW, FILE_ACTION_ADDED, FILE_ACTION_RENAMED_NEW_NAME,
    FILE_FLAG_BACKUP_SEMANTICS, FILE_LIST_DIRECTORY, FILE_NOTIFY_CHANGE, FILE_NOTIFY_INFORMATION,
    FILE_SHARE_DELETE, FILE_SHARE_READ, FILE_SHARE_WRITE, OPEN_EXISTING,
};

pub struct Change {
    pub name: String,
    /// The name just appeared: a new file, or one renamed to it.
    pub added: bool,
}

pub struct Folder(HANDLE);

impl Folder {
    pub fn open(path: &Path) -> Result<Self> {
        let handle = unsafe {
            CreateFileW(
                &HSTRING::from(path.as_os_str()),
                FILE_LIST_DIRECTORY.0,
                FILE_SHARE_READ | FILE_SHARE_WRITE | FILE_SHARE_DELETE,
                None,
                OPEN_EXISTING,
                FILE_FLAG_BACKUP_SEMANTICS,
                None,
            )?
        };
        Ok(Self(handle))
    }

    /// Blocks until something in the folder changes. Changes made between
    /// two calls are kept by Windows and returned by the next one.
    pub fn wait(&self, filter: FILE_NOTIFY_CHANGE) -> Result<Vec<Change>> {
        // Aligned for the records Windows writes into it.
        let mut buffer = [0u32; 1024];
        let mut bytes = 0;
        unsafe {
            ReadDirectoryChangesW(
                self.0,
                buffer.as_mut_ptr().cast(),
                size_of_val(&buffer) as u32,
                false,
                filter,
                Some(&mut bytes),
                None,
                None,
            )?;
        }
        let mut changes = Vec::new();
        if bytes == 0 {
            return Ok(changes);
        }
        let mut record = buffer.as_ptr().cast::<u8>();
        unsafe {
            loop {
                let info = &*record.cast::<FILE_NOTIFY_INFORMATION>();
                let name = std::slice::from_raw_parts(
                    info.FileName.as_ptr(),
                    info.FileNameLength as usize / size_of::<u16>(),
                );
                changes.push(Change {
                    name: String::from_utf16_lossy(name),
                    added: info.Action == FILE_ACTION_ADDED
                        || info.Action == FILE_ACTION_RENAMED_NEW_NAME,
                });
                if info.NextEntryOffset == 0 {
                    return Ok(changes);
                }
                record = record.add(info.NextEntryOffset as usize);
            }
        }
    }
}

impl Drop for Folder {
    fn drop(&mut self) {
        unsafe {
            let _ = CloseHandle(self.0);
        }
    }
}
