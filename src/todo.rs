//! The to-do list on the notch, kept in `%APPDATA%\Drop\todo.txt`, one task
//! per line: `[ ] Beli galon`, `[x] Kirim laporan`, and with a day and an
//! optional time to remind at, `[ ] Rapat @ 2026-11-10 15:00`.

use std::path::PathBuf;

use crate::calendar;

/// Tasks the list keeps, over every day.
pub const CAPACITY: usize = 60;
/// Longest task, in characters.
pub const MAX_CHARS: usize = 60;

/// A day as days since 1970-01-01, as [`calendar::days_from_civil`] counts.
pub type Day = i64;

#[derive(Clone, Debug, PartialEq)]
pub struct Task {
    pub text: String,
    pub done: bool,
    /// The day it is for. Tasks from before days existed have none and
    /// count as today's.
    pub day: Option<Day>,
    /// When to remind, in minutes after midnight on its day.
    pub time: Option<u32>,
}

impl Task {
    fn open(text: String, day: Option<Day>, time: Option<u32>) -> Self {
        Self {
            text,
            done: false,
            day,
            time: day.and(time),
        }
    }
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

    /// Open tasks first, then finished ones, each in the order the user keeps.
    pub fn tasks(&self) -> &[Task] {
        &self.tasks
    }

    pub fn is_full(&self) -> bool {
        self.tasks.len() >= CAPACITY
    }

    /// The tasks to show for `day`, as indexes into [`Todo::tasks`]: its own,
    /// and on `today` also those without a day and open ones left from
    /// earlier days.
    pub fn shown(&self, day: Day, today: Day) -> Vec<usize> {
        (0..self.tasks.len())
            .filter(|&index| {
                let task = &self.tasks[index];
                match task.day {
                    Some(own) if own == day => true,
                    None => day == today,
                    Some(own) => day == today && own < today && !task.done,
                }
            })
            .collect()
    }

    /// Days of the month with a task left to do.
    pub fn marked(&self, year: i32, month: u32) -> Vec<u32> {
        let mut days: Vec<u32> = self
            .tasks
            .iter()
            .filter(|task| !task.done)
            .filter_map(|task| task.day)
            .map(calendar::civil_from_days)
            .filter(|&(y, m, _)| y == year && m == month)
            .map(|(_, _, d)| d)
            .collect();
        days.sort_unstable();
        days.dedup();
        days
    }

    /// Open tasks of `day` whose time falls after `after` (from midnight
    /// when None) and up to `until`, in minutes after midnight.
    pub fn due(&self, day: Day, after: Option<u32>, until: u32) -> impl Iterator<Item = &Task> {
        self.tasks.iter().filter(move |task| {
            !task.done
                && task.day == Some(day)
                && task
                    .time
                    .is_some_and(|time| after.is_none_or(|after| time > after) && time <= until)
        })
    }

    /// Adds an open task after the other open ones. Returns false when the
    /// text is blank or the list is full.
    pub fn add(&mut self, text: &str, day: Option<Day>, time: Option<u32>) -> bool {
        let text = clean(text);
        if text.is_empty() || self.is_full() {
            return false;
        }
        let at = self.left();
        self.tasks.insert(at, Task::open(text, day, time));
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

    /// Whether two tasks may trade places: both open or both finished, so
    /// finished ones stay at the end.
    pub fn can_swap(&self, a: usize, b: usize) -> bool {
        match (self.tasks.get(a), self.tasks.get(b)) {
            (Some(a), Some(b)) => a.done == b.done,
            _ => false,
        }
    }

    pub fn swap(&mut self, a: usize, b: usize) {
        if a != b && self.can_swap(a, b) {
            self.tasks.swap(a, b);
            self.save();
        }
    }

    fn left(&self) -> usize {
        self.tasks.iter().filter(|task| !task.done).count()
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
        if cfg!(test) {
            return;
        }
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

/// A time typed as digits, as the field shows it: "15:3_" for "153".
pub fn time_mask(digits: &str) -> String {
    let mut shown: Vec<char> = "__:__".chars().collect();
    for (digit, slot) in digits.chars().zip([0, 1, 3, 4]) {
        shown[slot] = digit;
    }
    shown.into_iter().collect()
}

/// Minutes after midnight from typed digits: "1530" or "930" for 9:30, and
/// "9" or "09" for a whole hour. None for nothing typed; Err when it is no
/// time of day.
pub fn parse_time(digits: &str) -> Result<Option<u32>, ()> {
    let number = |text: &str| text.parse::<u32>().map_err(|_| ());
    let (hours, minutes) = match digits.len() {
        0 => return Ok(None),
        1 | 2 => (number(digits)?, 0),
        3 => (number(&digits[..1])?, number(&digits[1..])?),
        4 => (number(&digits[..2])?, number(&digits[2..])?),
        _ => return Err(()),
    };
    if hours < 24 && minutes < 60 {
        Ok(Some(hours * 60 + minutes))
    } else {
        Err(())
    }
}

/// "15:30"
pub fn format_time(minutes: u32) -> String {
    format!("{:02}:{:02}", minutes / 60, minutes % 60)
}

fn file() -> Option<PathBuf> {
    let appdata = std::env::var_os("APPDATA")?;
    Some(PathBuf::from(appdata).join("Drop").join("todo.txt"))
}

/// The day and time at the end of a line, " @ 2026-11-10 15:00", if any.
fn when(rest: &str) -> Option<(&str, Day, Option<u32>)> {
    let (text, when) = rest.rsplit_once(" @ ")?;
    let mut parts = when.split_whitespace();
    let mut date = parts.next()?.split('-');
    let year = date.next()?.parse().ok()?;
    let month = date.next()?.parse().ok().filter(|m| (1..=12).contains(m))?;
    let day = date.next()?.parse().ok()?;
    if date.next().is_some() || !(1..=calendar::days_in_month(year, month)).contains(&day) {
        return None;
    }
    let time = match parts.next() {
        None => None,
        Some(clock) => {
            let (hours, minutes) = clock.split_once(':')?;
            Some(parse_time(&format!("{hours:0>2}{minutes:0>2}")).ok()??)
        }
    };
    if parts.next().is_some() {
        return None;
    }
    Some((text, calendar::days_from_civil(year, month, day), time))
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
            let (rest, day, time) = match when(rest) {
                Some((text, day, time)) => (text, Some(day), time),
                None => (rest, None, None),
            };
            let text = clean(rest);
            (!text.is_empty()).then_some(Task {
                text,
                done,
                day,
                time,
            })
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
        .map(|task| {
            let mark = if task.done { 'x' } else { ' ' };
            let when = match (task.day, task.time) {
                (Some(day), time) => {
                    let (year, month, date) = calendar::civil_from_days(day);
                    let clock = time.map(|t| format!(" {}", format_time(t)));
                    format!(
                        " @ {year:04}-{month:02}-{date:02}{}",
                        clock.unwrap_or_default()
                    )
                }
                (None, _) => String::new(),
            };
            format!("[{mark}] {}{when}\n", task.text)
        })
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
        let list = todo(
            "\u{feff}[x] Kirim laporan\n[ ] Beli galon @ 2026-11-10\n\nTelepon ibu @ 2026-11-06 9:05\n",
        );
        assert_eq!(texts(&list), ["Beli galon", "Telepon ibu", "Kirim laporan"]);
        assert_eq!(list.left(), 2);
        assert_eq!(list.tasks()[1].time, Some(9 * 60 + 5));
        assert_eq!(
            list.tasks()[0].day,
            Some(calendar::days_from_civil(2026, 11, 10))
        );
        assert_eq!(parse(&format(list.tasks())), list.tasks());
    }

    #[test]
    fn odd_endings_stay_part_of_the_text() {
        let list = todo("[ ] Kopi @ besok\n[ ] Rapat @ 2026-02-30\n[ ] Makan @ 2026-11-10 25:00\n");
        assert_eq!(
            texts(&list),
            [
                "Kopi @ besok",
                "Rapat @ 2026-02-30",
                "Makan @ 2026-11-10 25:00"
            ]
        );
        assert!(list.tasks().iter().all(|task| task.day.is_none()));
    }

    #[test]
    fn new_tasks_join_the_open_ones() {
        let mut list = todo("[ ] A\n[x] B\n");
        list.add("C", Some(5), Some(60));
        assert_eq!(texts(&list), ["A", "C", "B"]);
        // A time needs a day.
        list.add("D", None, Some(60));
        assert_eq!(list.tasks()[2].time, None);
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
    fn finished_tasks_stay_below_when_moved() {
        let mut list = todo("[ ] A\n[ ] B\n[x] C\n");
        list.swap(0, 1);
        assert_eq!(texts(&list), ["B", "A", "C"]);
        assert!(!list.can_swap(1, 2));
        list.swap(1, 2);
        assert_eq!(texts(&list), ["B", "A", "C"]);
    }

    #[test]
    fn each_day_shows_its_own_and_today_what_is_left() {
        let today = calendar::days_from_civil(2026, 11, 6);
        let list = todo(
            "[ ] Lama @ 2026-11-01\n[ ] Hari ini @ 2026-11-06\n[ ] Kapan saja\n\
             [ ] Besok @ 2026-11-07 08:00\n[x] Sudah lama @ 2026-11-01\n",
        );
        let shown = |day| -> Vec<&str> {
            list.shown(day, today)
                .into_iter()
                .map(|i| list.tasks()[i].text.as_str())
                .collect()
        };
        assert_eq!(shown(today), ["Lama", "Hari ini", "Kapan saja"]);
        assert_eq!(shown(today + 1), ["Besok"]);
        assert_eq!(shown(today - 5), ["Lama", "Sudah lama"]);
        assert_eq!(list.marked(2026, 11), [1, 6, 7]);
        let due: Vec<_> = list.due(today + 1, Some(7 * 60 + 59), 8 * 60).collect();
        assert_eq!(due.len(), 1);
        assert_eq!(list.due(today + 1, Some(8 * 60), 9 * 60).count(), 0);
        assert_eq!(list.due(today + 1, None, 8 * 60).count(), 1);
    }

    #[test]
    fn reads_typed_times() {
        assert_eq!(parse_time(""), Ok(None));
        assert_eq!(parse_time("9"), Ok(Some(540)));
        assert_eq!(parse_time("930"), Ok(Some(570)));
        assert_eq!(parse_time("1530"), Ok(Some(930)));
        assert_eq!(parse_time("2460"), Err(()));
        assert_eq!(parse_time("25"), Err(()));
        assert_eq!(time_mask("153"), "15:3_");
        assert_eq!(time_mask(""), "__:__");
        assert_eq!(format_time(570), "09:30");
    }

    #[test]
    fn cleans_typed_text() {
        assert_eq!(clean("  Beli galon \r\nlagi"), "Beli galon");
        assert_eq!(clean(&"a".repeat(80)).len(), MAX_CHARS);
        // Multi-byte text is cut on character boundaries.
        assert_eq!(clean(&"é".repeat(80)).chars().count(), MAX_CHARS);
    }
}
