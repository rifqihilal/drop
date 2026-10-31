//! Events from an iCalendar feed (a `.ics` link, as Google Calendar and
//! Outlook give out). A worker thread fetches it every quarter hour and
//! posts the events around today to the notch window.
//!
//! Covers what calendars commonly use: timed and all-day events, daily,
//! weekly (with days), monthly and yearly repeats with an interval, a count
//! or an end date, skipped dates, and moved or cancelled single occurrences.
//! Times given in UTC are moved to the computer's time zone; times in a named
//! zone are taken as local.

use std::ffi::c_void;
use std::sync::mpsc::{channel, RecvTimeoutError, Sender};
use std::time::Duration;

use windows::Win32::Foundation::{HWND, LPARAM, WPARAM};
use windows::Win32::UI::WindowsAndMessaging::{PostMessageW, WM_APP};

use crate::config::FEEDS;
use crate::{calendar, net, prayer};

/// New events. LPARAM: a `Box<Vec<Event>>`.
pub const EVENTS_MSG: u32 = WM_APP + 13;
const REFRESH: Duration = Duration::from_secs(15 * 60);
const RETRY: Duration = Duration::from_secs(5 * 60);
/// Days kept before and after today: the calendar shows a year ahead and a
/// few months back.
const DAYS_BEFORE: i64 = 100;
const DAYS_AFTER: i64 = 370;
/// Bound on the steps through one repeating event.
const MAX_STEPS: i64 = 5000;

#[derive(Clone, Debug, PartialEq)]
pub struct Event {
    /// Days since 1970-01-01, local.
    pub day: i64,
    /// Local minutes after midnight; None for an all-day event.
    pub minute: Option<u32>,
    pub summary: String,
    /// Which of the feeds it came from, which gives it its colour.
    pub source: usize,
}

/// `events` that fall in a month, leaving out those before `after`, a day
/// and a minute.
pub fn in_month(
    events: &[Event],
    (year, month): (i32, u32),
    after: Option<(i64, u32)>,
) -> Vec<&Event> {
    events
        .iter()
        .filter(|event| {
            let (y, m, _) = calendar::civil_from_days(event.day);
            (y, m) == (year, month)
        })
        .filter(|event| {
            after.is_none_or(|(day, minute)| {
                event.day > day
                    || event.day == day && event.minute.is_none_or(|start| start >= minute)
            })
        })
        .collect()
}

fn by_time(a: &Event, b: &Event) -> std::cmp::Ordering {
    let key = |event: &Event| (event.day, event.minute.map_or(-1, i64::from));
    key(a).cmp(&key(b))
}

#[derive(Clone, Copy, Debug, PartialEq)]
enum Frequency {
    Daily,
    Weekly,
    Monthly,
    Yearly,
}

#[derive(Clone, Debug, PartialEq)]
struct Rule {
    frequency: Frequency,
    interval: i64,
    count: Option<u32>,
    /// Last day an occurrence may fall on.
    until: Option<i64>,
    /// Weekdays (Monday = 0) for weekly repeats; empty means the start's.
    days: Vec<i64>,
}

#[derive(Default)]
struct Raw {
    uid: String,
    summary: String,
    start: Option<(i64, Option<u32>)>,
    rule: Option<Rule>,
    skipped: Vec<i64>,
    /// Set on a changed single occurrence: the day it replaces.
    replaces: Option<i64>,
    cancelled: bool,
}

/// Events from `from` to `to` (days, inclusive), sorted by time.
/// `offset_minutes` is how far the local clock is ahead of UTC.
pub fn parse(text: &str, from: i64, to: i64, offset_minutes: i32) -> Vec<Event> {
    let mut raws = Vec::new();
    let mut current: Option<Raw> = None;
    // Depth inside components within an event, such as its alarms.
    let mut nested = 0;
    for line in unfold(text) {
        let Some((head, value)) = line.split_once(':') else {
            continue;
        };
        let mut parts = head.split(';');
        let name = parts.next().unwrap_or("").to_ascii_uppercase();
        let params = parts.collect::<Vec<_>>().join(";").to_ascii_uppercase();
        let event = value.eq_ignore_ascii_case("VEVENT");
        if current.is_some() && !event {
            match name.as_str() {
                "BEGIN" => nested += 1,
                "END" => {
                    nested -= 1;
                    continue;
                }
                _ => {}
            }
            if nested > 0 {
                continue;
            }
        }
        match (name.as_str(), current.as_mut()) {
            ("BEGIN", None) if value.eq_ignore_ascii_case("VEVENT") => {
                current = Some(Raw::default())
            }
            ("END", Some(_)) if value.eq_ignore_ascii_case("VEVENT") => raws.extend(current.take()),
            ("UID", Some(raw)) => raw.uid = value.to_string(),
            ("SUMMARY", Some(raw)) => raw.summary = unescape(value),
            ("DTSTART", Some(raw)) => raw.start = when(&params, value, offset_minutes),
            ("RRULE", Some(raw)) => raw.rule = rule(value),
            ("EXDATE", Some(raw)) => raw.skipped.extend(
                value
                    .split(',')
                    .filter_map(|value| when(&params, value, offset_minutes))
                    .map(|(day, _)| day),
            ),
            ("RECURRENCE-ID", Some(raw)) => {
                raw.replaces = when(&params, value, offset_minutes).map(|(day, _)| day)
            }
            ("STATUS", Some(raw)) => raw.cancelled = value.eq_ignore_ascii_case("CANCELLED"),
            _ => {}
        }
    }

    // A changed occurrence takes its series' place on that day.
    let replaced: Vec<(String, i64)> = raws
        .iter()
        .filter_map(|raw| Some((raw.uid.clone(), raw.replaces?)))
        .collect();
    let mut events = Vec::new();
    for raw in &raws {
        let Some((start, minute)) = raw.start else {
            continue;
        };
        if raw.cancelled {
            continue;
        }
        let summary = if raw.summary.is_empty() {
            "(Tanpa judul)".to_string()
        } else {
            raw.summary.clone()
        };
        let skip = |day: i64| {
            raw.skipped.contains(&day)
                || (raw.replaces.is_none()
                    && replaced
                        .iter()
                        .any(|(uid, moved)| *uid == raw.uid && *moved == day))
        };
        let days = match (&raw.rule, raw.replaces) {
            (Some(rule), None) => occurrences(rule, start, from, to),
            _ => vec![start]
                .into_iter()
                .filter(|day| (from..=to).contains(day))
                .collect(),
        };
        events.extend(days.into_iter().filter(|&day| !skip(day)).map(|day| Event {
            day,
            minute,
            summary: summary.clone(),
            source: 0,
        }));
    }
    events.sort_by(by_time);
    events
}

/// Joins folded lines: a line starting with a space or tab continues the last.
fn unfold(text: &str) -> Vec<String> {
    let mut lines: Vec<String> = Vec::new();
    for line in text.trim_start_matches('\u{feff}').split('\n') {
        let line = line.strip_suffix('\r').unwrap_or(line);
        match (line.strip_prefix([' ', '\t']), lines.last_mut()) {
            (Some(rest), Some(last)) => last.push_str(rest),
            _ => lines.push(line.to_string()),
        }
    }
    lines
}

fn unescape(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut chars = text.chars();
    while let Some(c) = chars.next() {
        if c != '\\' {
            out.push(c);
            continue;
        }
        match chars.next() {
            Some('n' | 'N') => out.push(' '),
            Some(other) => out.push(other),
            None => {}
        }
    }
    out.trim().to_string()
}

/// A date (`20261030`) or date-time (`20261030T090000`, `...Z` for UTC) as
/// a local day and, for date-times, minutes after midnight.
fn when(params: &str, value: &str, offset_minutes: i32) -> Option<(i64, Option<u32>)> {
    let value = value.trim();
    let number = |range: std::ops::Range<usize>| value.get(range)?.parse::<u32>().ok();
    let day = calendar::days_from_civil(number(0..4)? as i32, number(4..6)?, number(6..8)?);
    if params.contains("VALUE=DATE") && !params.contains("VALUE=DATE-TIME")
        || value.get(8..9) != Some("T")
    {
        return Some((day, None));
    }
    let mut total = day * 1440 + i64::from(number(9..11)? * 60 + number(11..13)?);
    if value.ends_with('Z') {
        total += i64::from(offset_minutes);
    }
    Some((total.div_euclid(1440), Some(total.rem_euclid(1440) as u32)))
}

fn rule(value: &str) -> Option<Rule> {
    let mut rule = Rule {
        frequency: Frequency::Daily,
        interval: 1,
        count: None,
        until: None,
        days: Vec::new(),
    };
    let mut frequency = None;
    for part in value.split(';') {
        let (key, value) = part.split_once('=')?;
        match key.to_ascii_uppercase().as_str() {
            "FREQ" => {
                frequency = Some(match value.to_ascii_uppercase().as_str() {
                    "DAILY" => Frequency::Daily,
                    "WEEKLY" => Frequency::Weekly,
                    "MONTHLY" => Frequency::Monthly,
                    "YEARLY" => Frequency::Yearly,
                    // Hourly and finer repeats are not shown.
                    _ => return None,
                })
            }
            "INTERVAL" => rule.interval = value.parse().unwrap_or(1).max(1),
            "COUNT" => rule.count = value.parse().ok(),
            "UNTIL" => rule.until = when("", value, 0).map(|(day, _)| day),
            "BYDAY" => {
                const NAMES: [&str; 7] = ["MO", "TU", "WE", "TH", "FR", "SA", "SU"];
                rule.days = value
                    .split(',')
                    .filter_map(|day| {
                        // "MO", or "1MO" / "-1FR" in monthly rules (not handled).
                        let name = day.get(day.len().checked_sub(2)?..)?.to_ascii_uppercase();
                        NAMES.iter().position(|n| *n == name).map(|i| i as i64)
                    })
                    .collect();
                rule.days.sort_unstable();
            }
            _ => {}
        }
    }
    rule.frequency = frequency?;
    Some(rule)
}

/// Days a repeating event falls on between `from` and `to`, counting from
/// its first occurrence on `start`.
fn occurrences(rule: &Rule, start: i64, from: i64, to: i64) -> Vec<i64> {
    let last = rule.until.map_or(to, |until| until.min(to));
    let mut found = Vec::new();
    let mut seen = 0;
    // Takes the next occurrence; false once there are no more.
    let mut take = |day: i64| -> bool {
        if day < start {
            return true;
        }
        if rule.count.is_some_and(|count| seen >= count) || day > last {
            return false;
        }
        seen += 1;
        if day >= from {
            found.push(day);
        }
        true
    };
    let (year, month, date) = calendar::civil_from_days(start);
    match rule.frequency {
        Frequency::Daily => {
            for step in 0..MAX_STEPS {
                if !take(start + step * rule.interval) {
                    break;
                }
            }
        }
        Frequency::Weekly => {
            let weekday = (start + 3).rem_euclid(7);
            let days = if rule.days.is_empty() {
                vec![weekday]
            } else {
                rule.days.clone()
            };
            let monday = start - weekday;
            'weeks: for step in 0..MAX_STEPS {
                for day in &days {
                    if !take(monday + step * 7 * rule.interval + day) {
                        break 'weeks;
                    }
                }
            }
        }
        Frequency::Monthly | Frequency::Yearly => {
            let months = if rule.frequency == Frequency::Monthly {
                rule.interval
            } else {
                12 * rule.interval
            };
            for step in 0..MAX_STEPS {
                let (y, m) = calendar::shift(year, month, (step * months) as i32);
                if calendar::days_from_civil(y, m, 1) > last {
                    break;
                }
                // The 31st or 29 February: skipped where the month lacks it.
                if date <= calendar::days_in_month(y, m)
                    && !take(calendar::days_from_civil(y, m, date))
                {
                    break;
                }
            }
        }
    }
    found
}

/// The feeds to fetch; blank ones are skipped.
pub struct IcalWatch {
    feeds: Sender<[String; FEEDS]>,
}

impl IcalWatch {
    pub fn start(hwnd: HWND) -> Self {
        let (feeds, inbox) = channel::<[String; FEEDS]>();
        let target = hwnd.0 as isize;
        std::thread::spawn(move || {
            let mut urls: [String; FEEDS] = Default::default();
            // What each feed gave last, kept while it cannot be reached.
            let mut found: [Vec<Event>; FEEDS] = Default::default();
            let mut wait = REFRESH;
            loop {
                match inbox.recv_timeout(wait) {
                    Ok(new) => urls = new,
                    Err(RecvTimeoutError::Timeout) => {}
                    Err(RecvTimeoutError::Disconnected) => break,
                }
                wait = REFRESH;
                for (source, url) in urls.iter().enumerate() {
                    if url.trim().is_empty() {
                        found[source].clear();
                    } else if let Some(events) = fetch(url, source) {
                        found[source] = events;
                    } else {
                        wait = RETRY;
                    }
                }
                let mut events: Vec<Event> = found.iter().flatten().cloned().collect();
                events.sort_by(by_time);
                post(target, events);
            }
        });
        Self { feeds }
    }

    /// Fetches now from `urls`.
    pub fn set_feeds(&self, urls: [String; FEEDS]) {
        let _ = self.feeds.send(urls);
    }
}

fn fetch(url: &str, source: usize) -> Option<Vec<Event>> {
    let body = net::get_url(url)?;
    let text = String::from_utf8_lossy(&body);
    if !text.contains("BEGIN:VCALENDAR") {
        return None;
    }
    let (year, month, day) = prayer::today();
    let today = calendar::days_from_civil(year as i32, month as u32, day as u32);
    let offset = (prayer::utc_offset_hours() * 60.0).round() as i32;
    let mut events = parse(&text, today - DAYS_BEFORE, today + DAYS_AFTER, offset);
    for event in &mut events {
        event.source = source;
    }
    Some(events)
}

fn post(target: isize, events: Vec<Event>) {
    let payload = Box::into_raw(Box::new(events));
    unsafe {
        let posted = PostMessageW(
            Some(HWND(target as *mut c_void)),
            EVENTS_MSG,
            WPARAM(0),
            LPARAM(payload as isize),
        );
        if posted.is_err() {
            drop(Box::from_raw(payload));
        }
    }
}

/// Takes ownership of the events carried by an [`EVENTS_MSG`].
///
/// # Safety
/// `lparam` must come from an [`EVENTS_MSG`] and be taken only once.
pub unsafe fn take_events(lparam: LPARAM) -> Vec<Event> {
    *Box::from_raw(lparam.0 as *mut Vec<Event>)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn day(year: i32, month: u32, date: u32) -> i64 {
        calendar::days_from_civil(year, month, date)
    }

    fn event(day: i64, minute: Option<u32>) -> Event {
        Event {
            day,
            minute,
            summary: String::new(),
            source: 0,
        }
    }

    #[test]
    fn a_month_holds_only_its_own_events() {
        let events = [
            event(day(2026, 10, 31), None),
            event(day(2026, 11, 1), Some(600)),
            event(day(2026, 11, 2), None),
            event(day(2026, 12, 1), None),
        ];
        let november = in_month(&events, (2026, 11), None);
        assert_eq!(november.len(), 2);
    }

    #[test]
    fn a_month_can_start_from_now() {
        let today = day(2026, 11, 1);
        let events = [
            event(today, Some(500)),
            event(today, Some(700)),
            event(today, None),
            event(today + 1, Some(1)),
        ];
        let left = in_month(&events, (2026, 11), Some((today, 600)));
        assert_eq!(left.len(), 3);
        assert_eq!(left[0].minute, Some(700));
    }

    fn feed(events: &str) -> String {
        format!("BEGIN:VCALENDAR\r\nVERSION:2.0\r\n{events}END:VCALENDAR\r\n")
    }

    fn days(events: &[Event]) -> Vec<(u32, u32)> {
        events
            .iter()
            .map(|event| {
                let (_, month, date) = calendar::civil_from_days(event.day);
                (month, date)
            })
            .collect()
    }

    #[test]
    fn civil_days_round_trip() {
        for n in [-1000, 0, 365, 20_000, 20_756] {
            let (y, m, d) = calendar::civil_from_days(n);
            assert_eq!(day(y, m, d), n);
        }
    }

    #[test]
    fn reads_timed_all_day_and_folded_events() {
        let text = feed(
            "BEGIN:VEVENT\r\nUID:a\r\nDTSTART:20261030T013000Z\r\nSUMMARY:Rapat\\, tim\r\n \
             mingguan\r\nEND:VEVENT\r\n\
             BEGIN:VEVENT\r\nUID:b\r\nDTSTART;VALUE=DATE:20261030\r\nSUMMARY:Libur\r\n\
             BEGIN:VALARM\r\nSUMMARY:Alarm\r\nEND:VALARM\r\nEND:VEVENT\r\n",
        );
        // UTC+9: 01:30Z is 10:30 local.
        let events = parse(&text, day(2026, 10, 1), day(2026, 11, 30), 9 * 60);
        assert_eq!(events.len(), 2);
        assert_eq!(events[0].summary, "Libur");
        assert_eq!(events[0].minute, None);
        assert_eq!(events[1].summary, "Rapat, timmingguan");
        assert_eq!(events[1].minute, Some(10 * 60 + 30));
    }

    #[test]
    fn utc_times_can_move_to_the_next_day() {
        let text = feed("BEGIN:VEVENT\r\nDTSTART:20261030T200000Z\r\nSUMMARY:X\r\nEND:VEVENT\r\n");
        let events = parse(&text, day(2026, 10, 1), day(2026, 11, 30), 9 * 60);
        assert_eq!(days(&events), [(10, 31)]);
        assert_eq!(events[0].minute, Some(5 * 60));
    }

    #[test]
    fn weekly_on_days_with_a_count() {
        // Monday 2 November 2026, on Mondays and Wednesdays, four times.
        let text = feed(
            "BEGIN:VEVENT\r\nUID:k\r\nDTSTART;TZID=Asia/Jayapura:20261102T080000\r\n\
             RRULE:FREQ=WEEKLY;BYDAY=MO,WE;COUNT=4\r\nSUMMARY:Kuliah\r\nEND:VEVENT\r\n",
        );
        let events = parse(&text, day(2026, 10, 1), day(2026, 12, 31), 9 * 60);
        assert_eq!(days(&events), [(11, 2), (11, 4), (11, 9), (11, 11)]);
        assert_eq!(events[0].minute, Some(8 * 60));
    }

    #[test]
    fn skipped_moved_and_cancelled_occurrences() {
        let text = feed(
            "BEGIN:VEVENT\r\nUID:s\r\nDTSTART:20261026T090000\r\n\
             RRULE:FREQ=DAILY;UNTIL=20261030T235959Z\r\nEXDATE:20261027T090000\r\n\
             SUMMARY:Stand-up\r\nEND:VEVENT\r\n\
             BEGIN:VEVENT\r\nUID:s\r\nRECURRENCE-ID:20261028T090000\r\n\
             DTSTART:20261028T130000\r\nSUMMARY:Stand-up\r\nEND:VEVENT\r\n\
             BEGIN:VEVENT\r\nUID:s\r\nRECURRENCE-ID:20261029T090000\r\n\
             DTSTART:20261029T090000\r\nSTATUS:CANCELLED\r\nEND:VEVENT\r\n",
        );
        let events = parse(&text, day(2026, 10, 1), day(2026, 11, 30), 0);
        assert_eq!(days(&events), [(10, 26), (10, 28), (10, 30)]);
        assert_eq!(events[1].minute, Some(13 * 60));
    }

    #[test]
    fn monthly_skips_short_months_and_yearly_repeats() {
        let text = feed(
            "BEGIN:VEVENT\r\nDTSTART;VALUE=DATE:20260131\r\nRRULE:FREQ=MONTHLY\r\n\
             SUMMARY:Gajian\r\nEND:VEVENT\r\n\
             BEGIN:VEVENT\r\nDTSTART;VALUE=DATE:20001105\r\nRRULE:FREQ=YEARLY\r\n\
             SUMMARY:Ulang tahun\r\nEND:VEVENT\r\n",
        );
        let events = parse(&text, day(2026, 10, 1), day(2026, 12, 31), 0);
        assert_eq!(days(&events), [(10, 31), (11, 5), (12, 31)]);
    }
}
