//! The quick countdown and the stopwatch, kept free of any drawing.

use std::time::{Duration, Instant};

/// The minutes the quick timers offer.
pub const QUICK_MINUTES: [u64; 3] = [1, 5, 10];

/// A countdown of a chosen number of minutes.
pub struct Countdown {
    total: Duration,
    /// When it ends, while running.
    ends_at: Option<Instant>,
    /// Time left while stopped.
    left: Duration,
}

impl Countdown {
    pub fn new(minutes: u64) -> Self {
        let total = Duration::from_secs(minutes * 60);
        Self {
            total,
            ends_at: None,
            left: total,
        }
    }

    /// Chooses the length; the countdown waits for its start.
    pub fn set(&mut self, minutes: u64) {
        *self = Self::new(minutes);
    }

    pub fn running(&self) -> bool {
        self.ends_at.is_some()
    }

    /// Waiting to start, at its full length.
    pub fn idle(&self) -> bool {
        !self.running() && self.left == self.total
    }

    /// The minutes it counts from.
    pub fn minutes(&self) -> u64 {
        self.total.as_secs() / 60
    }

    pub fn left(&self, now: Instant) -> Duration {
        match self.ends_at {
            Some(end) => end.saturating_duration_since(now),
            None => self.left,
        }
    }

    /// How much of it has gone, 0..1.
    pub fn progress(&self, now: Instant) -> f32 {
        1.0 - self.left(now).as_secs_f32() / self.total.as_secs_f32()
    }

    /// Starts or resumes a stopped countdown, and pauses a running one.
    pub fn toggle(&mut self, now: Instant) {
        match self.ends_at.take() {
            Some(end) => self.left = end.saturating_duration_since(now),
            None => self.ends_at = Some(now + self.left),
        }
    }

    /// Back to the full length, waiting to start.
    pub fn reset(&mut self) {
        self.ends_at = None;
        self.left = self.total;
    }

    /// Whether the countdown just ran out, in which case it waits again.
    pub fn finish(&mut self, now: Instant) -> bool {
        match self.ends_at {
            Some(end) if now >= end => {
                self.reset();
                true
            }
            _ => false,
        }
    }
}

#[derive(Default)]
pub struct Stopwatch {
    /// Time counted before the current run.
    carried: Duration,
    since: Option<Instant>,
}

impl Stopwatch {
    pub fn running(&self) -> bool {
        self.since.is_some()
    }

    /// Never started since the last reset.
    pub fn idle(&self) -> bool {
        !self.running() && self.carried.is_zero()
    }

    pub fn elapsed(&self, now: Instant) -> Duration {
        self.carried
            + self
                .since
                .map_or(Duration::ZERO, |since| now.saturating_duration_since(since))
    }

    pub fn toggle(&mut self, now: Instant) {
        match self.since.take() {
            Some(since) => self.carried += now.saturating_duration_since(since),
            None => self.since = Some(now),
        }
    }

    pub fn reset(&mut self) {
        *self = Self::default();
    }

    /// How far into the current minute it is, 0..1: the ring goes round once
    /// a minute.
    pub fn progress(&self, now: Instant) -> f32 {
        (self.elapsed(now).as_secs() % 60) as f32 / 60.0
    }
}

/// "4:59" for a countdown, rounding up so the last second shows 0:01; "1:02:03"
/// from an hour on.
pub fn format_clock(time: Duration, round_up: bool) -> String {
    let seconds = if round_up {
        time.as_millis().div_ceil(1000) as u64
    } else {
        time.as_secs()
    };
    if seconds >= 3600 {
        format!(
            "{}:{:02}:{:02}",
            seconds / 3600,
            seconds / 60 % 60,
            seconds % 60
        )
    } else {
        format!("{}:{:02}", seconds / 60, seconds % 60)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn choosing_minutes_does_not_start() {
        let start = Instant::now();
        let mut timer = Countdown::new(5);
        timer.set(10);
        assert!(timer.idle());
        assert!(!timer.running());
        assert_eq!(format_clock(timer.left(start), true), "10:00");
        assert!(!timer.finish(start + Duration::from_secs(3600)));
    }

    #[test]
    fn a_countdown_runs_out_once() {
        let start = Instant::now();
        let mut timer = Countdown::new(5);
        timer.toggle(start);
        assert!(timer.running());
        assert!(!timer.idle());
        assert!(!timer.finish(start + Duration::from_secs(299)));
        assert!(timer.finish(start + Duration::from_secs(300)));
        assert!(timer.idle(), "it waits again at its length");
        assert_eq!(timer.minutes(), 5);
        assert!(!timer.finish(start + Duration::from_secs(301)));
    }

    #[test]
    fn a_countdown_keeps_its_time_while_paused() {
        let start = Instant::now();
        let mut timer = Countdown::new(1);
        timer.toggle(start);
        timer.toggle(start + Duration::from_secs(20));
        assert!(!timer.running());
        assert!(!timer.idle(), "a paused countdown still shows");
        let later = start + Duration::from_secs(500);
        assert_eq!(format_clock(timer.left(later), true), "0:40");
        timer.toggle(later);
        assert!(timer.finish(later + Duration::from_secs(40)));
    }

    #[test]
    fn the_stopwatch_adds_its_runs() {
        let start = Instant::now();
        let mut watch = Stopwatch::default();
        assert!(watch.idle());
        watch.toggle(start);
        watch.toggle(start + Duration::from_secs(65));
        assert!(!watch.idle());
        let later = start + Duration::from_secs(900);
        assert_eq!(format_clock(watch.elapsed(later), false), "1:05");
        watch.toggle(later);
        assert_eq!(
            format_clock(watch.elapsed(later + Duration::from_secs(5)), false),
            "1:10"
        );
        assert!((watch.progress(later + Duration::from_secs(5)) - 10.0 / 60.0).abs() < 0.01);
        watch.reset();
        assert!(watch.idle());
    }

    #[test]
    fn long_times_show_hours() {
        assert_eq!(format_clock(Duration::from_secs(3723), false), "1:02:03");
        assert_eq!(format_clock(Duration::from_millis(100), true), "0:01");
    }
}
