//! Prayer times by the Kemenag RI method, computed locally from the sun's
//! position: Subuh at 20° below the horizon, Isya at 18°, Ashar when shadows
//! reach their length plus the object's (Syafi'i), and a few minutes of
//! ihtiyat added to each time.

use windows::Win32::System::SystemInformation::GetLocalTime;
use windows::Win32::System::SystemServices::TIME_ZONE_ID_DAYLIGHT;
use windows::Win32::System::Time::{GetTimeZoneInformation, TIME_ZONE_INFORMATION};

/// The five prayers Drop reminds about, in order.
pub const NAMES: [&str; 5] = ["Subuh", "Dzuhur", "Ashar", "Maghrib", "Isya"];

const FAJR_ANGLE: f64 = 20.0;
const ISHA_ANGLE: f64 = 18.0;
/// Shadow length factor for Ashar (Syafi'i).
const ASR_FACTOR: f64 = 1.0;
/// Safety margin added to each time, in minutes, as in the published
/// Kemenag schedules (Dzuhur and Maghrib get a longer one).
const IHTIYAT: [f64; 5] = [2.0, 3.0, 2.0, 3.0, 2.0];
/// How long before a prayer the closed notch counts down to it.
const SOON_MINUTES: u32 = 30;

#[derive(Clone, Copy, PartialEq, Debug)]
pub struct Location {
    pub latitude: f64,
    pub longitude: f64,
    /// Metres above sea level; lowers the horizon a little.
    pub elevation: f64,
}

/// Minutes after local midnight for each of [`NAMES`].
pub type Schedule = [u32; 5];

/// The day's prayer times at `location`, for a time zone `utc_offset` hours
/// ahead of UTC.
pub fn schedule(year: i32, month: u32, day: u32, location: Location, utc_offset: f64) -> Schedule {
    let lat = location.latitude;
    let jd = julian_day(year, month, day) - location.longitude / (15.0 * 24.0);
    let horizon = 0.833 + 0.0347 * location.elevation.max(0.0).sqrt();

    // Each time depends on the sun's position at that time, so start from
    // rough guesses (hours) and refine.
    let mut t = [5.0, 12.0, 13.0, 18.0, 18.0];
    for _ in 0..3 {
        let at = |hours: f64| jd + hours / 24.0;
        t = [
            sun_angle_time(FAJR_ANGLE, at(t[0]), lat, true),
            mid_day(at(t[1])),
            asr_time(at(t[2]), lat),
            sun_angle_time(horizon, at(t[3]), lat, false),
            sun_angle_time(ISHA_ANGLE, at(t[4]), lat, false),
        ];
    }

    std::array::from_fn(|i| {
        let local = t[i] + utc_offset - location.longitude / 15.0;
        // Round up so the time shown is never early.
        ((local * 60.0 + IHTIYAT[i]).ceil().rem_euclid(24.0 * 60.0)) as u32
    })
}

fn julian_day(year: i32, month: u32, day: u32) -> f64 {
    let (mut y, mut m) = (year as f64, month as f64);
    if m <= 2.0 {
        y -= 1.0;
        m += 12.0;
    }
    let a = (y / 100.0).floor();
    let b = 2.0 - a + (a / 4.0).floor();
    (365.25 * (y + 4716.0)).floor() + (30.6001 * (m + 1.0)).floor() + day as f64 + b - 1524.5
}

/// Sun declination (degrees) and equation of time (hours) at Julian day `jd`.
fn sun_position(jd: f64) -> (f64, f64) {
    let d = jd - 2451545.0;
    let g = fix_angle(357.529 + 0.98560028 * d);
    let q = fix_angle(280.459 + 0.98564736 * d);
    let l = fix_angle(q + 1.915 * sin(g) + 0.020 * sin(2.0 * g));
    let e = 23.439 - 0.00000036 * d;
    let declination = asin(sin(e) * sin(l));
    let right_ascension = fix_hour(atan2(cos(e) * sin(l), cos(l)) / 15.0);
    let equation = q / 15.0 - right_ascension;
    (declination, equation)
}

/// Solar noon, in hours UTC at longitude 0 (adjusted by the caller).
fn mid_day(jd: f64) -> f64 {
    let (_, equation) = sun_position(jd);
    fix_hour(12.0 - equation)
}

/// When the sun is `angle` degrees below the horizon, before noon when
/// `morning` is set.
fn sun_angle_time(angle: f64, jd: f64, lat: f64, morning: bool) -> f64 {
    let (declination, _) = sun_position(jd);
    let noon = mid_day(jd);
    let cos_hour = (-sin(angle) - sin(declination) * sin(lat)) / (cos(declination) * cos(lat));
    let hours = acos(cos_hour.clamp(-1.0, 1.0)) / 15.0;
    if morning {
        noon - hours
    } else {
        noon + hours
    }
}

fn asr_time(jd: f64, lat: f64) -> f64 {
    let (declination, _) = sun_position(jd);
    let angle = -acot(ASR_FACTOR + tan((lat - declination).abs()));
    sun_angle_time(angle, jd, lat, false)
}

// Trigonometry in degrees.
fn sin(d: f64) -> f64 {
    d.to_radians().sin()
}
fn cos(d: f64) -> f64 {
    d.to_radians().cos()
}
fn tan(d: f64) -> f64 {
    d.to_radians().tan()
}
fn asin(x: f64) -> f64 {
    x.asin().to_degrees()
}
fn acos(x: f64) -> f64 {
    x.acos().to_degrees()
}
fn atan2(y: f64, x: f64) -> f64 {
    y.atan2(x).to_degrees()
}
fn acot(x: f64) -> f64 {
    (1.0 / x).atan().to_degrees()
}
fn fix_angle(a: f64) -> f64 {
    a.rem_euclid(360.0)
}
fn fix_hour(h: f64) -> f64 {
    h.rem_euclid(24.0)
}

/// Local (year, month, day).
pub fn today() -> (u16, u16, u16) {
    let time = unsafe { GetLocalTime() };
    (time.wYear, time.wMonth, time.wDay)
}

/// Hours the local clock is ahead of UTC, including daylight saving.
pub fn utc_offset_hours() -> f64 {
    let mut zone = TIME_ZONE_INFORMATION::default();
    let kind = unsafe { GetTimeZoneInformation(&mut zone) };
    let bias = if kind == TIME_ZONE_ID_DAYLIGHT {
        zone.Bias + zone.DaylightBias
    } else {
        zone.Bias + zone.StandardBias
    };
    -(bias as f64) / 60.0
}

/// The schedule for today at `location`, in the computer's time zone.
pub fn today_at(location: Location) -> Schedule {
    let (year, month, day) = today();
    schedule(
        year as i32,
        month as u32,
        day as u32,
        location,
        utc_offset_hours(),
    )
}

/// The day after (`year`, `month`, `day`).
pub fn next_day(year: u16, month: u16, day: u16) -> (u16, u16, u16) {
    if u32::from(day) < crate::calendar::days_in_month(year as i32, month as u32) {
        (year, month, day + 1)
    } else if month < 12 {
        (year, month + 1, 1)
    } else {
        (year + 1, 1, 1)
    }
}

/// Where `now` falls among the day's prayers: 0 at Subuh, 4 at Isya, and in
/// between by the time passed from one prayer to the next.
pub fn day_position(times: &Schedule, now: u32) -> f32 {
    if now <= times[0] {
        return 0.0;
    }
    for (index, pair) in times.windows(2).enumerate() {
        if now < pair[1] {
            let span = pair[1].saturating_sub(pair[0]).max(1);
            return index as f32 + (now - pair[0]) as f32 / span as f32;
        }
    }
    4.0
}

/// The prayer coming up within [`SOON_MINUTES`], and the minutes until it.
pub fn soon(times: &Schedule, now: u32) -> Option<(usize, u32)> {
    let next = times.iter().position(|&time| time >= now)?;
    let wait = times[next] - now;
    (wait <= SOON_MINUTES).then_some((next, wait))
}

/// How far `now` is from the last prayer to the next one, 0..1. `next` is in
/// minutes after today's midnight, so tomorrow's Subuh is past 24 hours.
pub fn wait_progress(times: &Schedule, now: u32, next: u32) -> f32 {
    let last = times
        .iter()
        .rev()
        .find(|&&time| time <= now)
        .map_or(i64::from(times[4]) - 24 * 60, |&time| i64::from(time));
    let span = (i64::from(next) - last).max(1);
    ((i64::from(now) - last) as f32 / span as f32).clamp(0.0, 1.0)
}

/// "1 jam 12 mnt", "12 mnt", "< 1 mnt".
pub fn format_wait(minutes: u32) -> String {
    match (minutes / 60, minutes % 60) {
        (0, 0) => "< 1 mnt".to_string(),
        (0, m) => format!("{m} mnt"),
        (h, 0) => format!("{h} jam"),
        (h, m) => format!("{h} jam {m} mnt"),
    }
}

/// "04:09"
pub fn format_time(minutes: u32) -> String {
    format!("{:02}:{:02}", minutes / 60, minutes % 60)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn soon_covers_the_last_half_hour() {
        let times = [300, 720, 900, 1080, 1150];
        assert_eq!(soon(&times, 689), None);
        assert_eq!(soon(&times, 690), Some((1, 30)));
        assert_eq!(soon(&times, 720), Some((1, 0)));
        assert_eq!(soon(&times, 721), None);
        assert_eq!(soon(&times, 1200), None);
    }

    fn hm(text: &str) -> u32 {
        let (h, m) = text.split_once(':').unwrap();
        h.parse::<u32>().unwrap() * 60 + m.parse::<u32>().unwrap()
    }

    /// Compares against the published Kemenag schedule, allowing a minute
    /// for differences in the city coordinates used.
    fn check(location: Location, utc_offset: f64, date: (i32, u32, u32), expected: [&str; 5]) {
        let got = schedule(date.0, date.1, date.2, location, utc_offset);
        for (i, want) in expected.iter().enumerate() {
            let diff = got[i] as i32 - hm(want) as i32;
            assert!(
                diff.abs() <= 1,
                "{} on {:?}: got {}, Kemenag {}",
                NAMES[i],
                date,
                format_time(got[i]),
                want
            );
        }
    }

    const JAKARTA: Location = Location {
        latitude: -6.1754,
        longitude: 106.8272,
        elevation: 0.0,
    };

    #[test]
    fn jakarta_matches_kemenag() {
        check(
            JAKARTA,
            7.0,
            (2026, 10, 30),
            ["04:09", "11:40", "14:54", "17:49", "19:00"],
        );
        check(
            JAKARTA,
            7.0,
            (2026, 6, 21),
            ["04:40", "11:58", "15:19", "17:51", "19:05"],
        );
    }

    #[test]
    fn makassar_matches_kemenag() {
        let makassar = Location {
            latitude: -5.1477,
            longitude: 119.4327,
            elevation: 0.0,
        };
        check(
            makassar,
            8.0,
            (2026, 10, 30),
            ["04:20", "11:49", "15:04", "17:58", "19:08"],
        );
    }

    #[test]
    fn eastern_indonesia_matches_kemenag() {
        let ambon = Location {
            latitude: -3.6954,
            longitude: 128.1814,
            elevation: 0.0,
        };
        check(
            ambon,
            9.0,
            (2026, 10, 30),
            ["04:47", "12:14", "15:31", "18:21", "19:32"],
        );
        let jayapura = Location {
            latitude: -2.5337,
            longitude: 140.7181,
            elevation: 0.0,
        };
        check(
            jayapura,
            9.0,
            (2026, 10, 30),
            ["03:58", "11:24", "14:41", "17:30", "18:40"],
        );
        // Aimas, against Kemenag's schedule for Kabupaten Sorong.
        let aimas = Location {
            latitude: -0.95698,
            longitude: 131.33534,
            elevation: 9.0,
        };
        check(
            aimas,
            9.0,
            (2026, 10, 30),
            ["04:37", "12:02", "15:20", "18:06", "19:16"],
        );
    }

    #[test]
    fn follows_the_day_between_prayers() {
        let times = [240, 720, 900, 1080, 1140];
        assert_eq!(day_position(&times, 100), 0.0);
        assert_eq!(day_position(&times, 480), 0.5);
        assert_eq!(day_position(&times, 1080), 3.0);
        assert_eq!(day_position(&times, 1300), 4.0);
    }

    #[test]
    fn measures_the_wait_from_the_last_prayer() {
        let times = [240, 720, 900, 1080, 1140];
        assert_eq!(wait_progress(&times, 480, 720), 0.5);
        // After Isya, on the way to tomorrow's Subuh.
        assert_eq!(wait_progress(&times, 1140, 240 + 24 * 60), 0.0);
        // Before Subuh, the wait began at yesterday's Isya.
        assert_eq!(wait_progress(&times, 105, 240), 0.75);
    }

    #[test]
    fn rolls_over_months_and_years() {
        assert_eq!(next_day(2026, 10, 30), (2026, 10, 31));
        assert_eq!(next_day(2026, 10, 31), (2026, 11, 1));
        assert_eq!(next_day(2026, 12, 31), (2027, 1, 1));
        assert_eq!(next_day(2028, 2, 28), (2028, 2, 29));
    }

    #[test]
    fn formats_waits() {
        assert_eq!(format_wait(0), "< 1 mnt");
        assert_eq!(format_wait(12), "12 mnt");
        assert_eq!(format_wait(60), "1 jam");
        assert_eq!(format_wait(72), "1 jam 12 mnt");
    }

    #[test]
    fn medan_matches_kemenag() {
        let medan = Location {
            latitude: 3.5952,
            longitude: 98.6722,
            elevation: 0.0,
        };
        check(
            medan,
            7.0,
            (2026, 10, 30),
            ["04:53", "12:12", "15:32", "18:12", "19:22"],
        );
    }
}
