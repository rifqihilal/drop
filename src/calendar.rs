//! Month grid arithmetic and the Pomodoro timer, kept free of any drawing.

use std::time::{Duration, Instant};

pub const MONTHS: [&str; 12] = [
    "Januari",
    "Februari",
    "Maret",
    "April",
    "Mei",
    "Juni",
    "Juli",
    "Agustus",
    "September",
    "Oktober",
    "November",
    "Desember",
];

/// Week starts on Monday, as on Indonesian calendars.
pub const WEEKDAYS: [&str; 7] = ["Sn", "Sl", "Rb", "Km", "Jm", "Sb", "Mg"];

/// The month `offset` months away from (`year`, `month`).
pub fn shift(year: i32, month: u32, offset: i32) -> (i32, u32) {
    let index = year * 12 + month as i32 - 1 + offset;
    (index.div_euclid(12), index.rem_euclid(12) as u32 + 1)
}

pub fn days_in_month(year: i32, month: u32) -> u32 {
    let (next_year, next_month) = shift(year, month, 1);
    (days_from_civil(next_year, next_month, 1) - days_from_civil(year, month, 1)) as u32
}

/// Day of the week with Monday = 0.
pub fn weekday(year: i32, month: u32, day: u32) -> u32 {
    // 1970-01-01 was a Thursday.
    (days_from_civil(year, month, day) + 3).rem_euclid(7) as u32
}

/// Days since 1970-01-01 (Howard Hinnant's algorithm).
pub fn days_from_civil(year: i32, month: u32, day: u32) -> i64 {
    let year = i64::from(year) - i64::from(month <= 2);
    let era = year.div_euclid(400);
    let year_of_era = year - era * 400;
    let month = i64::from(month);
    let shifted = if month > 2 { month - 3 } else { month + 9 };
    let day_of_year = (153 * shifted + 2) / 5 + i64::from(day) - 1;
    let day_of_era = year_of_era * 365 + year_of_era / 4 - year_of_era / 100 + day_of_year;
    era * 146_097 + day_of_era - 719_468
}

/// The (year, month, day) `days` after 1970-01-01; the inverse of
/// [`days_from_civil`].
pub fn civil_from_days(days: i64) -> (i32, u32, u32) {
    let days = days + 719_468;
    let era = days.div_euclid(146_097);
    let day_of_era = days - era * 146_097;
    let year_of_era =
        (day_of_era - day_of_era / 1460 + day_of_era / 36_524 - day_of_era / 146_096) / 365;
    let day_of_year = day_of_era - (365 * year_of_era + year_of_era / 4 - year_of_era / 100);
    let shifted = (5 * day_of_year + 2) / 153;
    let day = (day_of_year - (153 * shifted + 2) / 5 + 1) as u32;
    let month = if shifted < 10 {
        shifted + 3
    } else {
        shifted - 9
    } as u32;
    let year = year_of_era + era * 400 + i64::from(month <= 2);
    (year as i32, month, day)
}

const FOCUS: Duration = Duration::from_secs(25 * 60);
const BREAK: Duration = Duration::from_secs(5 * 60);

#[derive(Clone, Copy, PartialEq, Debug)]
pub enum Phase {
    Focus,
    Break,
}

/// Focus for 25 minutes, then a 5 minute break that starts on its own; the
/// next focus waits for the user.
pub struct Pomodoro {
    pub phase: Phase,
    /// Session number, counting focus periods.
    pub session: u32,
    /// When the phase ends, while running.
    ends_at: Option<Instant>,
    /// Time left while paused.
    left: Duration,
}

impl Default for Pomodoro {
    fn default() -> Self {
        Self {
            phase: Phase::Focus,
            session: 1,
            ends_at: None,
            left: FOCUS,
        }
    }
}

impl Pomodoro {
    pub fn running(&self) -> bool {
        self.ends_at.is_some()
    }

    /// Untouched since the last reset: nothing to show in the notch.
    pub fn idle(&self) -> bool {
        !self.running() && self.phase == Phase::Focus && self.left == FOCUS && self.session == 1
    }

    pub fn left(&self, now: Instant) -> Duration {
        match self.ends_at {
            Some(end) => end.saturating_duration_since(now),
            None => self.left,
        }
    }

    /// How much of the current phase has gone, 0..1.
    pub fn progress(&self, now: Instant) -> f32 {
        let total = match self.phase {
            Phase::Focus => FOCUS,
            Phase::Break => BREAK,
        };
        1.0 - self.left(now).as_secs_f32() / total.as_secs_f32()
    }

    pub fn toggle(&mut self, now: Instant) {
        match self.ends_at.take() {
            Some(end) => self.left = end.saturating_duration_since(now),
            None => self.ends_at = Some(now + self.left),
        }
    }

    pub fn reset(&mut self) {
        *self = Self::default();
    }

    /// Moves to the next phase when the current one is over, and returns the
    /// phase that just ended.
    pub fn advance(&mut self, now: Instant) -> Option<Phase> {
        let end = self.ends_at?;
        if now < end {
            return None;
        }
        let finished = self.phase;
        match finished {
            Phase::Focus => {
                self.phase = Phase::Break;
                self.left = BREAK;
                self.ends_at = Some(now + BREAK);
            }
            Phase::Break => {
                self.phase = Phase::Focus;
                self.session += 1;
                self.left = FOCUS;
                self.ends_at = None;
            }
        }
        Some(finished)
    }

    pub fn label(&self) -> &'static str {
        match self.phase {
            Phase::Focus => "Fokus",
            Phase::Break => "Istirahat",
        }
    }
}

/// "24:59"
pub fn format_left(left: Duration) -> String {
    // Round up, so the last second shows 0:01 rather than 0:00.
    let seconds = left.as_millis().div_ceil(1000) as u64;
    format!("{}:{:02}", seconds / 60, seconds % 60)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn progress_runs_from_empty_to_full() {
        let mut pomodoro = Pomodoro::default();
        let now = Instant::now();
        assert_eq!(pomodoro.progress(now), 0.0);
        pomodoro.toggle(now);
        let halfway = pomodoro.progress(now + FOCUS / 2);
        assert!((halfway - 0.5).abs() < 0.01, "{halfway}");
    }

    #[test]
    fn knows_month_lengths_and_weekdays() {
        assert_eq!(days_in_month(2026, 10), 31);
        assert_eq!(days_in_month(2026, 2), 28);
        assert_eq!(days_in_month(2028, 2), 29);
        assert_eq!(days_in_month(2026, 12), 31);
        // 30 October 2026 is a Friday.
        assert_eq!(weekday(2026, 10, 30), 4);
        // 1 January 2026 was a Thursday.
        assert_eq!(weekday(2026, 1, 1), 3);
    }

    #[test]
    fn shifts_across_years() {
        assert_eq!(shift(2026, 12, 1), (2027, 1));
        assert_eq!(shift(2026, 1, -1), (2025, 12));
        assert_eq!(shift(2026, 10, -22), (2024, 12));
    }

    #[test]
    fn pomodoro_runs_focus_then_break_then_waits() {
        let start = Instant::now();
        let mut timer = Pomodoro::default();
        assert!(timer.idle());
        timer.toggle(start);
        assert_eq!(format_left(timer.left(start)), "25:00");
        assert_eq!(timer.advance(start + FOCUS - Duration::from_secs(1)), None);

        let focus_over = start + FOCUS;
        assert_eq!(timer.advance(focus_over), Some(Phase::Focus));
        assert_eq!(timer.phase, Phase::Break);
        assert!(timer.running(), "the break starts on its own");

        assert_eq!(timer.advance(focus_over + BREAK), Some(Phase::Break));
        assert_eq!(timer.phase, Phase::Focus);
        assert_eq!(timer.session, 2);
        assert!(!timer.running(), "the next focus waits");
    }

    #[test]
    fn pausing_keeps_the_time_left() {
        let start = Instant::now();
        let mut timer = Pomodoro::default();
        timer.toggle(start);
        timer.toggle(start + Duration::from_secs(90));
        assert!(!timer.running());
        assert_eq!(
            format_left(timer.left(start + Duration::from_secs(500))),
            "23:30"
        );
        timer.reset();
        assert!(timer.idle());
    }
}
