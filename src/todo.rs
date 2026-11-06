//! A short to-do list on the notch, kept in `%APPDATA%\Drop\todo.txt`, one
//! task per line: `[ ] Beli galon` or `[x] Kirim laporan`.

use std::path::PathBuf;

/// Tasks the list keeps.
pub const CAPACITY: usize = 18;
/// Tasks the card shows at a time; more go on further views.
pub const PER_VIEW: usize = 6;
/// Longest task, in characters.
pub const MAX_CHARS: usize = 60;

#[derive(Clone, Debug, PartialEq)]
pub struct Task {
    pub text: String,
    pub done: bool,
}

#[derive(Default)]
pub struct Todo {
    tasks: Vec<Task>,
}

impl Todo {
    pub fn load() -> Self {
        let text = file()
            .and_then(|file| std::fs::read_to_string(file).ok())
            .unwrap_or_default();
        Self {
            tasks: parse(&text),
        }
    }

    /// Open tasks first, then finished ones, each in the order added.
    pub fn tasks(&self) -> &[Task] {
        &self.tasks
    }

    pub fn left(&self) -> usize {
        self.tasks.iter().filter(|task| !task.done).count()
    }

    pub fn is_full(&self) -> bool {
        self.tasks.len() >= CAPACITY
    }

    /// Adds an open task after the other open ones. Returns false when the
    /// text is blank or the list is full.
    pub fn add(&mut self, text: &str) -> bool {
        let text = clean(text);
        if text.is_empty() || self.is_full() {
            return false;
        }
        self.insert_open(text);
        self.save();
        true
    }

    /// Ticks a task off, or opens it again; it moves to its group's end.
    pub fn toggle(&mut self, index: usize) {
        if index < self.tasks.len() {
            self.flip(index);
            self.save();
        }
    }

    pub fn remove(&mut self, index: usize) {
        if index < self.tasks.len() {
            self.tasks.remove(index);
            self.save();
        }
    }

    pub fn clear_done(&mut self) {
        self.tasks.retain(|task| !task.done);
        self.save();
    }

    fn insert_open(&mut self, text: String) {
        let at = self.left();
        self.tasks.insert(at, Task { text, done: false });
    }

    fn flip(&mut self, index: usize) {
        let mut task = self.tasks.remove(index);
        task.done = !task.done;
        let at = if task.done {
            self.tasks.len()
        } else {
            self.left()
        };
        self.tasks.insert(at, task);
    }

    fn save(&self) {
        let Some(file) = file() else {
            return;
        };
        if let Some(dir) = file.parent() {
            let _ = std::fs::create_dir_all(dir);
        }
        let _ = std::fs::write(file, format(&self.tasks));
    }
}

/// One line, trimmed and cut to [`MAX_CHARS`].
pub fn clean(text: &str) -> String {
    let line = text.lines().next().unwrap_or("").trim();
    let cut: String = line.chars().take(MAX_CHARS).collect();
    cut.trim_end().to_string()
}

fn file() -> Option<PathBuf> {
    let appdata = std::env::var_os("APPDATA")?;
    Some(PathBuf::from(appdata).join("Drop").join("todo.txt"))
}

fn parse(text: &str) -> Vec<Task> {
    let mut tasks: Vec<Task> = text
        .trim_start_matches('\u{feff}')
        .lines()
        .filter_map(|line| {
            let line = line.trim();
            let (done, rest) = match line.get(..3) {
                Some("[x]" | "[X]") => (true, &line[3..]),
                Some("[ ]") => (false, &line[3..]),
                _ => (false, line),
            };
            let text = clean(rest);
            (!text.is_empty()).then_some(Task { text, done })
        })
        .take(CAPACITY)
        .collect();
    // A stable sort keeps each group's order.
    tasks.sort_by_key(|task| task.done);
    tasks
}

fn format(tasks: &[Task]) -> String {
    tasks
        .iter()
        .map(|task| format!("[{}] {}\n", if task.done { 'x' } else { ' ' }, task.text))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn todo(lines: &str) -> Todo {
        Todo {
            tasks: parse(lines),
        }
    }

    fn texts(todo: &Todo) -> Vec<&str> {
        todo.tasks().iter().map(|task| task.text.as_str()).collect()
    }

    #[test]
    fn reads_what_it_writes_and_plain_lines() {
        let list = todo("\u{feff}[x] Kirim laporan\n[ ] Beli galon\n\nTelepon ibu\n");
        assert_eq!(texts(&list), ["Beli galon", "Telepon ibu", "Kirim laporan"]);
        assert_eq!(list.left(), 2);
        assert_eq!(parse(&format(list.tasks())), list.tasks());
    }

    #[test]
    fn new_tasks_join_the_open_ones() {
        let mut list = todo("[ ] A\n[x] B\n");
        list.insert_open("C".to_string());
        assert_eq!(texts(&list), ["A", "C", "B"]);
    }

    #[test]
    fn ticking_moves_a_task_between_groups() {
        let mut list = todo("[ ] A\n[ ] B\n[x] C\n");
        list.flip(0);
        assert_eq!(texts(&list), ["B", "C", "A"]);
        list.flip(2);
        assert_eq!(texts(&list), ["B", "A", "C"]);
    }

    #[test]
    fn cleans_typed_text() {
        assert_eq!(clean("  Beli galon \r\nlagi"), "Beli galon");
        assert_eq!(clean(&"a".repeat(80)).len(), MAX_CHARS);
        // Multi-byte text is cut on character boundaries.
        assert_eq!(clean(&"é".repeat(80)).chars().count(), MAX_CHARS);
    }
}
