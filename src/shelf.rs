//! Files parked in the notch. Only their paths are kept, never copies, and
//! the list survives restarts in `%APPDATA%\Drop\shelf.txt`.

use std::path::{Path, PathBuf};

/// Two rows of five on the shelf card.
pub const CAPACITY: usize = 10;

#[derive(Default)]
pub struct Shelf {
    items: Vec<PathBuf>,
}

impl Shelf {
    /// Loads the saved list, dropping files that no longer exist.
    pub fn load() -> Self {
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

    pub fn is_empty(&self) -> bool {
        self.items.is_empty()
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
            self.items.remove(index);
            self.save();
        }
    }

    pub fn clear(&mut self) {
        self.items.clear();
        self.save();
    }

    fn push(&mut self, path: PathBuf) {
        self.items.retain(|item| !same_path(item, &path));
        self.items.push(path);
        if self.items.len() > CAPACITY {
            self.items.remove(0);
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
            .map(|path| format!("{}\n", path.display()))
            .collect();
        let _ = std::fs::write(file, text);
    }
}

/// Windows paths compare case-insensitively.
fn same_path(a: &Path, b: &Path) -> bool {
    a.as_os_str().eq_ignore_ascii_case(b.as_os_str())
}

/// The file's name for its tile.
pub fn display_name(path: &Path) -> String {
    path.file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_else(|| path.display().to_string())
}

fn file() -> Option<PathBuf> {
    let appdata = std::env::var_os("APPDATA")?;
    Some(PathBuf::from(appdata).join("Drop").join("shelf.txt"))
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
    fn the_oldest_falls_off_past_capacity() {
        let names: Vec<String> = (0..12).map(|i| format!(r"C:\file{i}.txt")).collect();
        let names: Vec<&str> = names.iter().map(String::as_str).collect();
        let shelf = shelf_of(&names);
        assert_eq!(shelf.items().len(), CAPACITY);
        assert_eq!(display_name(&shelf.items()[0]), "file2.txt");
    }
}
