//! Spring physics for the notch shape and the idle eyes.

use std::time::{Duration, Instant};

/// Notch size while the computer is in use (logical pixels).
pub const COMPACT: (f32, f32) = (132.0, 28.0);
/// Notch size while the computer is idle and the eyes are showing.
pub const IDLE: (f32, f32) = (164.0, 40.0);
/// Notch size while showing the volume or brightness level.
pub const LEVEL: (f32, f32) = (220.0, 36.0);
/// Notch width range and height while showing a notification.
pub const NOTICE_WIDTH: (f32, f32) = (200.0, 380.0);
pub const NOTICE_HEIGHT: f32 = 62.0;
/// Notch size while music plays: album art and level bars.
pub const MUSIC_STRIP: (f32, f32) = (196.0, 30.0);
/// Notch size of the full music card.
pub const MUSIC_CARD: (f32, f32) = (380.0, 112.0);
/// Notch size of the status card: clock, weather and battery.
pub const STATUS_CARD: (f32, f32) = (340.0, 112.0);
/// Notch size of the calendar and Pomodoro card.
pub const CALENDAR_CARD: (f32, f32) = (400.0, 198.0);
/// Notch size while the Pomodoro timer runs.
pub const TIMER_STRIP: (f32, f32) = (170.0, 30.0);
/// How long the level stays up after the last change.
const LEVEL_DURATION: Duration = Duration::from_millis(1600);
/// How long a notification stays up by default.
pub const NOTICE_DURATION: Duration = Duration::from_millis(3500);
/// How long a notice stays up after its content is refreshed.
const NOTICE_REFRESH_HOLD: Duration = Duration::from_millis(2500);

/// A damped spring that eases `value` toward `target`.
pub struct Spring {
    pub value: f32,
    pub target: f32,
    velocity: f32,
    stiffness: f32,
    damping: f32,
    epsilon: f32,
}

impl Spring {
    pub fn new(value: f32, stiffness: f32, damping: f32, epsilon: f32) -> Self {
        Self {
            value,
            target: value,
            velocity: 0.0,
            stiffness,
            damping,
            epsilon,
        }
    }

    /// Advances the spring by `dt` seconds. Returns true while it is still moving.
    pub fn step(&mut self, dt: f32) -> bool {
        if self.is_settled() {
            return false;
        }
        // Fixed substeps keep the integration stable when a frame arrives late.
        let mut remaining = dt;
        while remaining > 0.0 {
            let h = remaining.min(1.0 / 240.0);
            let force = -self.stiffness * (self.value - self.target) - self.damping * self.velocity;
            self.velocity += force * h;
            self.value += self.velocity * h;
            remaining -= h;
        }
        if (self.value - self.target).abs() < self.epsilon
            && self.velocity.abs() < self.epsilon * 10.0
        {
            self.value = self.target;
            self.velocity = 0.0;
        }
        true
    }

    fn is_settled(&self) -> bool {
        self.value == self.target && self.velocity == 0.0
    }
}

/// Small xorshift generator: the eyes only need variety, not quality.
struct Rng(u64);

impl Rng {
    fn next_f32(&mut self) -> f32 {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 7;
        self.0 ^= self.0 << 17;
        (self.0 >> 40) as f32 / (1u64 << 24) as f32
    }

    fn range(&mut self, min: f32, max: f32) -> f32 {
        min + (max - min) * self.next_f32()
    }

    fn secs(&mut self, min: f32, max: f32) -> Duration {
        Duration::from_secs_f32(self.range(min, max))
    }
}

const BLINK_SECS: f32 = 0.16;

/// Blinking and glancing for the idle eyes.
pub struct Eyes {
    /// 0 = open, 1 = closed.
    pub blink: f32,
    /// -1 = looking left, 1 = looking right.
    pub look: Spring,
    blink_start: Option<Instant>,
    next_blink: Instant,
    next_glance: Instant,
    rng: Rng,
}

impl Eyes {
    fn new(now: Instant, seed: u64) -> Self {
        Self {
            blink: 0.0,
            look: Spring::new(0.0, 150.0, 17.0, 0.002),
            blink_start: None,
            next_blink: now,
            next_glance: now,
            rng: Rng(seed | 1),
        }
    }

    /// Starts from a neutral face with a blink soon after the eyes appear.
    fn wake(&mut self, now: Instant) {
        self.blink = 0.0;
        self.blink_start = None;
        self.look = Spring::new(0.0, 150.0, 17.0, 0.002);
        self.next_blink = now + self.rng.secs(0.6, 1.2);
        self.next_glance = now + self.rng.secs(1.5, 3.0);
    }

    fn step(&mut self, now: Instant, dt: f32) -> bool {
        let mut moving = false;

        if let Some(start) = self.blink_start {
            let t = (now - start).as_secs_f32() / BLINK_SECS;
            if t >= 1.0 {
                self.blink = 0.0;
                self.blink_start = None;
            } else {
                // Close then open, eased so the lid snaps shut and lifts softly.
                let tri = 1.0 - (2.0 * t - 1.0).abs();
                self.blink = tri * tri * (3.0 - 2.0 * tri);
            }
            moving = true;
        } else if now >= self.next_blink {
            self.blink_start = Some(now);
            // Occasionally blink twice in a row, like people do.
            self.next_blink = if self.rng.next_f32() < 0.2 {
                now + Duration::from_millis(260)
            } else {
                now + self.rng.secs(2.5, 6.0)
            };
            moving = true;
        }

        if now >= self.next_glance {
            if self.look.target != 0.0 {
                self.look.target = 0.0;
                self.next_glance = now + self.rng.secs(2.0, 5.0);
            } else {
                let side = if self.rng.next_f32() < 0.5 { -1.0 } else { 1.0 };
                self.look.target = side * self.rng.range(0.6, 1.0);
                self.next_glance = now + self.rng.secs(0.8, 1.8);
            }
        }
        moving |= self.look.step(dt);
        moving
    }
}

#[derive(Clone, Copy, PartialEq)]
pub enum LevelKind {
    Volume,
    Brightness,
}

/// What a banner shows.
pub enum Content {
    /// A volume or brightness level. `level` (0..1) is eased so the bar
    /// slides between key presses.
    Level {
        kind: LevelKind,
        level: Spring,
        muted: bool,
    },
    Notice(Notice),
}

/// Colour of a notice's ring and accent icon.
#[derive(Clone, Copy)]
pub enum Accent {
    Green,
    Red,
    Blue,
    Teal,
}

/// What sits on the right of a notice, if anything.
pub enum Trailing {
    None,
    /// Large text, such as a battery level or a time.
    Text(String),
    /// An icon in a glowing circle, such as headphones.
    Icon(&'static str),
}

/// A two-line notification card: a ring with an icon, a title over a
/// subtitle, and an optional trailing element.
pub struct Notice {
    pub icon: &'static str,
    pub accent: Accent,
    /// How much of the ring is filled, 0..1.
    pub progress: f32,
    pub title: String,
    pub subtitle: String,
    pub trailing: Trailing,
    /// Greys the ring out, for things that went away.
    pub dim: bool,
}

/// Temporary content that takes over the notch.
pub struct Banner {
    pub content: Content,
    /// Notch size while this banner shows; notices fit their content.
    size: (f32, f32),
    until: Instant,
}

/// The notch's animated state.
pub struct Notch {
    pub width: Spring,
    pub height: Spring,
    /// How visible the eyes are, 0..1.
    pub eyes_visible: Spring,
    pub eyes: Eyes,
    /// How visible the banner is, 0..1.
    pub banner_visible: Spring,
    /// Kept while fading out so the last content stays drawn.
    pub banner: Option<Banner>,
    /// How visible the playing-music strip (art and bars) is, 0..1.
    pub music_visible: Spring,
    /// How visible the full music card is, 0..1.
    pub card_visible: Spring,
    idle: bool,
    banner_active: bool,
    /// Size of the live strip (music or timer), if one shows.
    strip_size: Option<(f32, f32)>,
    /// Size of the open card (music or status), if one is open.
    card_size: Option<(f32, f32)>,
}

impl Notch {
    pub fn new(now: Instant, seed: u64) -> Self {
        Self {
            width: Spring::new(COMPACT.0, 380.0, 30.0, 0.05),
            height: Spring::new(COMPACT.1, 380.0, 30.0, 0.05),
            eyes_visible: Spring::new(0.0, 260.0, 32.0, 0.002),
            eyes: Eyes::new(now, seed),
            banner_visible: Spring::new(0.0, 320.0, 34.0, 0.002),
            banner: None,
            music_visible: Spring::new(0.0, 300.0, 34.0, 0.002),
            card_visible: Spring::new(0.0, 300.0, 34.0, 0.002),
            idle: false,
            banner_active: false,
            strip_size: None,
            card_size: None,
        }
    }

    pub fn set_idle(&mut self, idle: bool, now: Instant) {
        if idle == self.idle {
            return;
        }
        self.idle = idle;
        if idle {
            self.eyes.wake(now);
        }
        self.retarget();
    }

    pub fn show_level(&mut self, kind: LevelKind, value: f32, is_muted: bool, now: Instant) {
        let until = now + LEVEL_DURATION;
        if let Some(Banner {
            content:
                Content::Level {
                    kind: shown,
                    level,
                    muted,
                },
            until: shown_until,
            ..
        }) = &mut self.banner
        {
            if *shown == kind {
                level.target = value;
                *muted = is_muted;
                *shown_until = until;
                self.activate();
                return;
            }
        }
        self.banner = Some(Banner {
            content: Content::Level {
                kind,
                level: Spring::new(value, 500.0, 40.0, 0.001),
                muted: is_muted,
            },
            size: LEVEL,
            until,
        });
        self.activate();
    }

    /// Shows a notice in a notch `width` wide (measured by the renderer).
    pub fn show_notice(&mut self, notice: Notice, width: f32, duration: Duration, now: Instant) {
        self.banner = Some(Banner {
            content: Content::Notice(notice),
            size: (width.clamp(NOTICE_WIDTH.0, NOTICE_WIDTH.1), NOTICE_HEIGHT),
            until: now + duration,
        });
        self.activate();
    }

    /// Swaps in new content for the notice still showing under the same title,
    /// without restarting it, and keeps it up a little longer so the change
    /// can be read. Returns false when that notice is no longer showing.
    pub fn refresh_notice(&mut self, notice: Notice, width: f32, now: Instant) -> bool {
        let Some(banner) = self.banner.as_mut().filter(|_| self.banner_active) else {
            return false;
        };
        let Content::Notice(shown) = &banner.content else {
            return false;
        };
        if shown.title != notice.title {
            return false;
        }
        banner.content = Content::Notice(notice);
        banner.size = (width.clamp(NOTICE_WIDTH.0, NOTICE_WIDTH.1), NOTICE_HEIGHT);
        banner.until = banner.until.max(now + NOTICE_REFRESH_HOLD);
        self.retarget();
        true
    }

    fn activate(&mut self) {
        self.banner_active = true;
        self.retarget();
    }

    /// True while anything other than the clock needs periodic wake-ups.
    pub fn busy(&self) -> bool {
        self.eyes_visible.value > 0.01
            || self.idle
            || self.banner.is_some()
            || self.card_visible.value > 0.01
    }

    /// Advances every animation. Returns true while anything is still moving.
    pub fn step(&mut self, now: Instant, dt: f32) -> bool {
        if self.banner_active && self.banner.as_ref().is_some_and(|b| now >= b.until) {
            self.banner_active = false;
            self.retarget();
        }

        let mut moving = self.width.step(dt);
        moving |= self.height.step(dt);
        moving |= self.eyes_visible.step(dt);
        moving |= self.banner_visible.step(dt);
        moving |= self.music_visible.step(dt);
        moving |= self.card_visible.step(dt);
        if let Some(Banner {
            content: Content::Level { level, .. },
            ..
        }) = &mut self.banner
        {
            moving |= level.step(dt);
        }
        if !self.banner_active && self.banner_visible.value == 0.0 {
            self.banner = None;
        }
        if self.eyes_visible.value > 0.01 || self.idle {
            moving |= self.eyes.step(now, dt);
        }
        moving
    }

    /// Picks the shape and content for the current state. A banner wins over
    /// the eyes so a key press or notification while idle is still seen.
    /// Order of precedence: banner, card, live strip, eyes, clock.
    fn retarget(&mut self) {
        let banner = self.banner_active;
        let card = self.card_size.filter(|_| !banner);
        let strip = self.strip_size.filter(|_| !banner && card.is_none());
        let eyes = self.idle && !banner && card.is_none() && strip.is_none();
        let (w, h) = match (&self.banner, card, strip) {
            (Some(shown), _, _) if banner => shown.size,
            (_, Some(size), _) | (_, None, Some(size)) => size,
            _ if eyes => IDLE,
            _ => COMPACT,
        };
        self.width.target = w;
        self.height.target = h;
        let on = |visible: bool| if visible { 1.0 } else { 0.0 };
        self.banner_visible.target = on(banner);
        self.card_visible.target = on(card.is_some());
        self.music_visible.target = on(strip.is_some());
        self.eyes_visible.target = on(eyes);
    }

    /// Shows a live strip and/or opens a card, each of the given size.
    pub fn set_live(&mut self, strip: Option<(f32, f32)>, card: Option<(f32, f32)>) {
        if (strip, card) != (self.strip_size, self.card_size) {
            self.strip_size = strip;
            self.card_size = card;
            self.retarget();
        }
    }
}
