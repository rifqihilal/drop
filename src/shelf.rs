//! Files parked in the notch. Only their paths are kept, never copies, and
//! the list survives restarts in `%APPDATA%\Drop\shelf.txt`. Copied text and
//! pictures sit beside them as clips: files of our own in the clips folder,
//! which, like the clipboard history they stand for, end with the session.

use std::path::{Path, PathBuf};

/// Two rows of five on the shelf card.
pub const CAPACITY: usize = 10;
/// Where clips are saved, under `%APPDATA%\Drop`.
const CLIPS: &str = "clips";
/// Length of the hash that starts a clip's file name, with its space.
const CLIP_PREFIX: usize = 9;

#[derive(Default)]
pub struct Shelf {
    items: Vec<PathBuf>,
}

impl Shelf {
    /// Loads the saved list, dropping files that no longer exist, and clears
    /// the last session's clips.
    pub fn load() -> Self {
        if let Some(dir) = clips_dir() {
            let _ = std::fs::remove_dir_all(dir);
        }
        let text = file().and_then(|file| std::fs::read_to_string(file).ok());
        let mut shelf = Self::default();
        for line in text.iter().flat_map(|text| text.lines()) {
            let path = PathBuf::from(line.trim());
            if !line.trim().is_empty() && path.exists() {
                shelf.push(path);
            }
        }
        shelf
    }

    pub fn items(&self) -> &[PathBuf] {
        &self.items
    }

    /// Adds files, newest last; a file already on the shelf moves to the
    /// end, and the oldest fall off past [`CAPACITY`]. Returns how many were
    /// added or moved.
    pub fn add(&mut self, paths: impl IntoIterator<Item = PathBuf>) -> usize {
        let mut count = 0;
        for path in paths {
            self.push(path);
            count += 1;
        }
        self.save();
        count
    }

    pub fn remove(&mut self, index: usize) {
        if index < self.items.len() {
            forget(&self.items.remove(index));
            self.save();
        }
    }

    pub fn clear(&mut self) {
        self.items.iter().for_each(|path| forget(path));
        self.items.clear();
        self.save();
    }

    /// Past capacity clips go first, so copying never pushes off a parked
    /// file.
    fn push(&mut self, path: PathBuf) {
        self.items.retain(|item| !same_path(item, &path));
        self.items.push(path);
        if self.items.len() > CAPACITY {
            let older = &self.items[..self.items.len() - 1];
            let oldest = older.iter().position(|item| is_clip(item)).unwrap_or(0);
            forget(&self.items.remove(oldest));
        }
    }

    fn save(&self) {
        let Some(file) = file() else {
            return;
        };
        if let Some(dir) = file.parent() {
            let _ = std::fs::create_dir_all(dir);
        }
        let text: String = self
            .items
            .iter()
            .filter(|path| !is_clip(path))
            .map(|path| format!("{}\n", path.display()))
            .collect();
        let _ = std::fs::write(file, text);
    }
}

/// Windows paths compare case-insensitively.
fn same_path(a: &Path, b: &Path) -> bool {
    a.as_os_str().eq_ignore_ascii_case(b.as_os_str())
}

/// Whether `path` is a clip rather than a parked file.
pub fn is_clip(path: &Path) -> bool {
    path.parent()
        .and_then(Path::file_name)
        .is_some_and(|folder| folder == CLIPS)
}

/// Whether `path` is a clip holding text.
pub fn is_text_clip(path: &Path) -> bool {
    is_clip(path) && path.extension().is_some_and(|extension| extension == "txt")
}

/// Clips are ours, so one leaving the shelf leaves the disk too.
fn forget(path: &Path) {
    if is_clip(path) {
        let _ = std::fs::remove_file(path);
    }
}

/// The name for a file's tile; a clip's without its hash or extension.
pub fn display_name(path: &Path) -> String {
    if is_clip(path) {
        if let Some(stem) = path.file_stem() {
            let stem = stem.to_string_lossy();
            return stem.get(CLIP_PREFIX..).unwrap_or(&stem).to_string();
        }
    }
    path.file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_else(|| path.display().to_string())
}

fn file() -> Option<PathBuf> {
    let appdata = std::env::var_os("APPDATA")?;
    Some(PathBuf::from(appdata).join("Drop").join("shelf.txt"))
}

/// The folder clips are saved in.
pub fn clips_dir() -> Option<PathBuf> {
    let appdata = std::env::var_os("APPDATA")?;
    Some(PathBuf::from(appdata).join("Drop").join(CLIPS))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn shelf_of(names: &[&str]) -> Shelf {
        let mut shelf = Shelf::default();
        for name in names {
            shelf.push(PathBuf::from(name));
        }
        shelf
    }

    #[test]
    fn re_adding_moves_a_file_to_the_end() {
        let mut shelf = shelf_of(&[r"C:\a.txt", r"C:\b.txt"]);
        shelf.push(PathBuf::from(r"c:\A.TXT"));
        let names: Vec<_> = shelf.items().iter().map(|p| display_name(p)).collect();
        assert_eq!(names, ["b.txt", "A.TXT"]);
    }

    #[test]
    fn clips_fall_off_before_parked_files() {
        let mut shelf = shelf_of(&[r"C:\file.txt", r"C:\Drop\clips\00000000 Halo.txt"]);
        for i in 0..9 {
            shelf.push(PathBuf::from(format!(r"C:\Drop\clips\0000000{i} Baru.txt")));
        }
        assert_eq!(shelf.items().len(), CAPACITY);
        assert_eq!(display_name(&shelf.items()[0]), "file.txt");
        assert!(!shelf
            .items()
            .iter()
            .any(|path| display_name(path) == "Halo"));
    }

    #[test]
    fn a_full_shelf_of_files_still_takes_a_clip() {
        let names: Vec<String> = (0..10).map(|i| format!(r"C:\file{i}.txt")).collect();
        let names: Vec<&str> = names.iter().map(String::as_str).collect();
        let mut shelf = shelf_of(&names);
        shelf.push(PathBuf::from(r"C:\Drop\clips\00000000 Halo.txt"));
        assert_eq!(display_name(&shelf.items()[9]), "Halo");
        assert_eq!(display_name(&shelf.items()[0]), "file1.txt");
    }

    #[test]
    fn clips_show_their_text() {
        let clip = Path::new(r"C:\Drop\clips\0a1b2c3d Halo dunia.txt");
        assert!(is_text_clip(clip));
        assert_eq!(display_name(clip), "Halo dunia");
        let picture = Path::new(r"C:\Drop\clips\0a1b2c3d Gambar.png");
        assert!(is_clip(picture) && !is_text_clip(picture));
        assert!(!is_clip(Path::new(r"C:\Users\a\b.txt")));
    }

    #[test]
    fn the_oldest_falls_off_past_capacity() {
        let names: Vec<String> = (0..12).map(|i| format!(r"C:\file{i}.txt")).collect();
        let names: Vec<&str> = names.iter().map(String::as_str).collect();
        let shelf = shelf_of(&names);
        assert_eq!(shelf.items().len(), CAPACITY);
        assert_eq!(display_name(&shelf.items()[0]), "file2.txt");
    }
}
