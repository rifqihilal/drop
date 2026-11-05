//! Spring physics for the notch shape and the idle eyes.

use std::time::{Duration, Instant};

use crate::notifications::AppNotice;

/// Notch size while the computer is in use (logical pixels).
pub const COMPACT: (f32, f32) = (156.0, 28.0);
/// The closed notch at the small and large settings.
pub const COMPACT_SMALL: (f32, f32) = (136.0, 24.0);
pub const COMPACT_LARGE: (f32, f32) = (184.0, 34.0);
/// Logical pixels between the floating notch and the screen's top.
pub const FLOAT_GAP: f32 = 8.0;
/// Notch size in the minimal setting: a thin bar that leaves browser tabs free.
pub const MINIMAL: (f32, f32) = (120.0, 6.0);
/// Notch size while hovered in click-to-open mode: widened for the date.
pub const PEEK: (f32, f32) = (360.0, 30.0);
/// Notch size while the computer is idle and the eyes are showing.
pub const IDLE: (f32, f32) = (164.0, 40.0);
/// Notch size while showing the volume or brightness level.
pub const LEVEL: (f32, f32) = (220.0, 36.0);
/// Notch width range and height while showing a notification.
pub const NOTICE_WIDTH: (f32, f32) = (200.0, 380.0);
pub const NOTICE_HEIGHT: f32 = 62.0;
/// Notch width limits and height while a new song is announced: a row with
/// the cover and date, and a line for its name.
pub const SONG_BANNER: (f32, f32) = (360.0, 58.0);
const SONG_BANNER_MAX: f32 = 480.0;
/// Notch size in lyric mode at its narrowest. A song keeps one width, set
/// once by its longest line, up to the most.
pub const LYRIC_STRIP: (f32, f32) = (280.0, 30.0);
pub const LYRIC_STRIP_MAX: f32 = 540.0;
/// The music card's extra width in lyric mode, for the lines.
pub const LYRIC_CARD_EXTRA: f32 = 120.0;
/// Room the window keeps for a card overshooting as it opens.
pub const CARD_BOUNCE: (f32, f32) = (30.0, 16.0);
/// Notch size while showing a notification from another app.
pub const APP_BANNER: (f32, f32) = (420.0, 112.0);
/// Notch size while music plays: album art and level bars.
pub const MUSIC_STRIP: (f32, f32) = (196.0, 30.0);
/// Every open card is this wide: a page between two side arrows.
pub const CARD_WIDTH: f32 = 400.0 + 2.0 * CARD_SIDE;
/// Room for the side arrows on each side of a page.
pub const CARD_SIDE: f32 = 30.0;
/// Pages sit this far down, under the card's header row.
pub const CARD_TOP: f32 = 22.0;

/// Every open card is this size, so the arrows stay put between pages.
pub const CARD: (f32, f32) = (CARD_WIDTH, CARD_TOP + 164.0);
/// The open card grown downward to hold the page icons.
pub const CARD_NAV: (f32, f32) = (CARD_WIDTH, CARD.1 + 26.0);
/// Notch size while files are dragged over it.
pub const DROP_ZONE: (f32, f32) = (320.0, 84.0);
/// Height of one task on the task card.
pub const TODO_ROW: f32 = 28.0;
/// Events listed beside the month at most.
pub const AGENDA_ROWS: usize = 3;
/// Notch size while the Pomodoro timer runs.
pub const TIMER_STRIP: (f32, f32) = (196.0, 30.0);
/// The same with a song playing, whose cover and bars join the timer.
pub const TIMER_MUSIC_STRIP: (f32, f32) = (216.0, 30.0);
/// Notch size while files download: a glyph, a name and its bytes.
pub const DOWNLOAD_STRIP: (f32, f32) = (320.0, 30.0);
/// Notch size while a prayer reminder shows: only wider than the clock.
pub const PRAYER_BANNER: (f32, f32) = (276.0, 30.0);
/// Damping of the notch's shape: close to no overshoot, and for a card a
/// little less, which makes it bounce.
const DAMPING: f32 = 30.0;
const CARD_DAMPING: f32 = 23.0;
/// Stiffness and damping of the liquid's colour fading after a drop: slow,
/// with no bounce.
const LIQUID_FADE: (f32, f32) = (2.2, 3.0);
/// How long a ripple runs, and how far it reaches.
const RIPPLE_SECS: f32 = 2.0;
const RIPPLE_REACH: f32 = 430.0;
/// Stiffness and damping of a notice's ring as it fills.
const METER: (f32, f32) = (26.0, 10.0);
/// How long a notice's ring beats.
const BEAT_SECS: f32 = 1.4;
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
/// A drowsy blink is slow, as if the lids were heavy.
const DROWSY_BLINK_SECS: f32 = 0.7;
/// Seconds shown before the eyes turn drowsy, and before they fall asleep.
const DROWSY_AFTER: f32 = 30.0;
const ASLEEP_AFTER: f32 = 60.0;
/// How long a sleeping "z" takes to float away.
const Z_SECS: f32 = 2.4;
/// How long sleepy eyes stay wide open after being woken.
const STARTLE: Duration = Duration::from_millis(800);
/// How long the eyes then greet the user before the notch closes.
const GREET: Duration = Duration::from_millis(1800);
/// How high the eyes hop in their greeting.
const HOP: f32 = 5.0;

/// The idle eyes' moods: glancing and blinking at first, then drowsy, then
/// asleep. Woken, sleepy eyes are startled, and all of them greet the user.
pub struct Eyes {
    /// 0 = open, 1 = closed.
    pub blink: f32,
    /// -1 = looking left, 1 = looking right.
    pub look: Spring,
    /// 0 = awake, 0.5 = drowsy, 1 = asleep.
    pub sleep: Spring,
    /// How far the eyes have opened wide from being woken, 0..1.
    pub surprise: Spring,
    /// How far the "z" has floated, 0..1, while one shows.
    pub z: Option<f32>,
    /// How far the eyes are lifted by their greeting.
    pub hop: f32,
    /// How visible the greeting's word is, 0..1.
    pub greeting: f32,
    since: Instant,
    /// When the greeting starts, once the eyes have been woken.
    greet_at: Option<Instant>,
    blink_secs: f32,
    z_start: Option<Instant>,
    next_z: Instant,
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
            sleep: Spring::new(0.0, 10.0, 7.0, 0.002),
            surprise: Spring::new(0.0, 320.0, 20.0, 0.002),
            z: None,
            hop: 0.0,
            greeting: 0.0,
            since: now,
            greet_at: None,
            blink_secs: BLINK_SECS,
            z_start: None,
            next_z: now,
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
        self.sleep = Spring::new(0.0, 10.0, 7.0, 0.002);
        self.surprise = Spring::new(0.0, 320.0, 20.0, 0.002);
        self.z = None;
        self.z_start = None;
        self.hop = 0.0;
        self.greeting = 0.0;
        self.since = now;
        self.greet_at = None;
        self.next_blink = now + self.rng.secs(0.6, 1.2);
        self.next_glance = now + self.rng.secs(1.5, 3.0);
    }

    /// Wakes the eyes: sleepy ones open wide first, then all of them greet.
    /// Returns how long that takes.
    fn rouse(&mut self, now: Instant) -> Duration {
        let startle = if self.sleep.value < 0.3 {
            Duration::ZERO
        } else {
            self.surprise.target = 1.0;
            STARTLE
        };
        self.greet_at = Some(now + startle);
        self.sleep.value = 0.0;
        self.sleep.target = 0.0;
        self.look.target = 0.0;
        self.blink = 0.0;
        self.blink_start = None;
        self.z = None;
        self.z_start = None;
        startle + GREET
    }

    /// Two hops that fade away, with the word showing meanwhile.
    fn step_greeting(&mut self, now: Instant, start: Instant) {
        if now < start {
            return;
        }
        self.surprise.target = 0.0;
        let t = ((now - start).as_secs_f32() / GREET.as_secs_f32()).min(1.0);
        self.hop = HOP * (std::f32::consts::TAU * t).sin().abs() * (1.0 - t);
        self.greeting = (t / 0.15).min((1.0 - t) / 0.2).clamp(0.0, 1.0);
    }

    /// The mood after `shown` seconds on screen: see [`Eyes::sleep`].
    fn mood(shown: f32) -> f32 {
        if shown >= ASLEEP_AFTER {
            1.0
        } else if shown >= DROWSY_AFTER {
            0.5
        } else {
            0.0
        }
    }

    fn step(&mut self, now: Instant, dt: f32) -> bool {
        let mut moving = self.sleep.step(dt);
        moving |= self.surprise.step(dt);
        if let Some(start) = self.greet_at {
            self.step_greeting(now, start);
            return self.look.step(dt) || moving || now >= start;
        }
        let mood = Self::mood((now - self.since).as_secs_f32());
        self.sleep.target = mood;
        if mood == 1.0 {
            self.blink = 0.0;
            self.blink_start = None;
            self.look.target = 0.0;
            moving |= self.look.step(dt);
            return self.step_z(now) || moving;
        }

        if let Some(start) = self.blink_start {
            let t = (now - start).as_secs_f32() / self.blink_secs;
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
            self.blink_secs = if mood > 0.0 {
                DROWSY_BLINK_SECS
            } else {
                BLINK_SECS
            };
            // Occasionally blink twice in a row, like people do.
            self.next_blink = if mood > 0.0 {
                now + self.rng.secs(2.5, 4.0)
            } else if self.rng.next_f32() < 0.2 {
                now + Duration::from_millis(260)
            } else {
                now + self.rng.secs(2.5, 6.0)
            };
            moving = true;
        }

        if mood > 0.0 {
            self.look.target = 0.0;
        } else if now >= self.next_glance {
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

    /// Floats a "z" up every few seconds while asleep.
    fn step_z(&mut self, now: Instant) -> bool {
        let Some(start) = self.z_start else {
            if self.sleep.value > 0.95 && now >= self.next_z {
                self.z_start = Some(now);
                self.z = Some(0.0);
                return true;
            }
            return false;
        };
        let t = (now - start).as_secs_f32() / Z_SECS;
        if t >= 1.0 {
            self.z = None;
            self.z_start = None;
            self.next_z = now + self.rng.secs(1.0, 2.0);
        } else {
            self.z = Some(t);
        }
        true
    }
}

/// How long the bell shakes, and the wait before it shakes again.
const BELL_SECS: f32 = 0.9;
const BELL_REST: Duration = Duration::from_secs(15);

/// The bell's tilt in degrees `t` seconds into a shake, dying away.
fn bell_swing(t: f32) -> f32 {
    16.0 * (-3.5 * t).exp() * (std::f32::consts::TAU * 6.0 * t).sin()
}

/// The closed notch's bell, which shakes as it appears and now and then after.
pub struct Bell {
    /// Tilt in degrees.
    pub angle: f32,
    started: Option<Instant>,
    next: Instant,
}

impl Bell {
    pub fn new(now: Instant) -> Self {
        Self {
            angle: 0.0,
            started: None,
            next: now,
        }
    }

    /// Advances the shake while the bell `shown`. Returns true while it moves.
    pub fn step(&mut self, now: Instant, shown: bool) -> bool {
        if !shown {
            self.angle = 0.0;
            self.next = now;
            return self.started.take().is_some();
        }
        if self.started.is_none() && now >= self.next {
            self.started = Some(now);
        }
        let Some(start) = self.started else {
            return false;
        };
        let t = (now - start).as_secs_f32();
        if t >= BELL_SECS {
            self.angle = 0.0;
            self.started = None;
            self.next = now + BELL_REST;
        } else {
            self.angle = bell_swing(t);
        }
        true
    }
}

/// A ripple's radius and strength `age` seconds in: quick at first, then
/// slowing as it dies away.
fn ripple_ring(age: f32) -> (f32, f32) {
    let left = (1.0 - age / RIPPLE_SECS).clamp(0.0, 1.0);
    (RIPPLE_REACH * (1.0 - left * left), left * left)
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
    /// A notification from another app, which a click opens.
    App(AppNotice),
    /// A prayer reminder: the icon on the left, this text on the right.
    /// The prayer with its time, and how long until it.
    Prayer {
        title: String,
        wait: String,
    },
    /// A new song: its title and artist under the clock.
    Song(String),
}

/// Colour of a notice's ring and accent icon.
#[derive(Clone, Copy)]
pub enum Accent {
    Green,
    Red,
    Blue,
    Teal,
}

/// How a notice's icon moves, by what it stands for.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Motion {
    /// Swells and settles, as a signal arriving.
    Pulse,
    /// Closes twice, as eyes do.
    Blink,
    /// Sways from side to side, as a step.
    Walk,
    /// Bounces up and down, as a drop.
    Bob,
    /// Rocks fast, as a ringing alarm.
    Shake,
    /// Fades and returns, as a flash.
    Flash,
}

/// Where an icon stands in its motion.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Pose {
    pub dx: f32,
    pub dy: f32,
    /// Degrees clockwise.
    pub angle: f32,
    pub scale_x: f32,
    pub scale_y: f32,
    pub alpha: f32,
}

impl Pose {
    const REST: Self = Self {
        dx: 0.0,
        dy: 0.0,
        angle: 0.0,
        scale_x: 1.0,
        scale_y: 1.0,
        alpha: 1.0,
    };
}

impl Motion {
    /// The pose `t` of the way through the motion, 0..1. It starts and ends
    /// at rest.
    pub fn pose(self, t: f32) -> Pose {
        use std::f32::consts::{PI, TAU};
        let t = t.clamp(0.0, 1.0);
        let fade = 1.0 - t;
        match self {
            Self::Pulse => {
                let swell = 1.0 + 0.18 * (TAU * t).sin().abs() * fade.sqrt();
                Pose {
                    scale_x: swell,
                    scale_y: swell,
                    ..Pose::REST
                }
            }
            Self::Blink => {
                let close = |from: f32| 1.0 - ((t - from) / 0.1).abs();
                Pose {
                    scale_y: 1.0 - 0.9 * close(0.25).max(close(0.65)).max(0.0),
                    ..Pose::REST
                }
            }
            Self::Walk => {
                let step = (TAU * 3.0 * t).sin() * fade;
                Pose {
                    dx: 3.0 * step,
                    angle: 10.0 * step,
                    ..Pose::REST
                }
            }
            Self::Bob => Pose {
                dy: -4.0 * (TAU * 2.0 * t).sin().abs() * fade,
                ..Pose::REST
            },
            Self::Shake => Pose {
                angle: 16.0 * (TAU * 5.0 * t).sin() * fade,
                ..Pose::REST
            },
            Self::Flash => Pose {
                alpha: 0.35 + 0.65 * (PI * 3.0 * t).cos().abs(),
                ..Pose::REST
            },
        }
    }
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
    pub motion: Motion,
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
    /// The card and the widened notch bounce as they open.
    pub bounce: bool,
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
    /// How far the page dots have turned into page icons, 0..1.
    pub nav_visible: Spring,
    /// How far the notch has widened to show the date, 0..1.
    pub peek_visible: Spring,
    /// How far the music card's volume slider has opened, 0..1.
    pub volume_visible: Spring,
    /// How far a notice's ring has filled, 0..1.
    pub notice_meter: Spring,
    /// How far a notice's beat has gone, 0..1, while it lasts.
    pub notice_beat: Option<f32>,
    /// How visible the dark water is while something is dragged over the
    /// notch, 0..1.
    pub liquid_glow: Spring,
    /// Where something landed, on the canvas, for the ripples to start from.
    pub ripple_at: (f32, f32),
    /// Seconds the ripples have run, and when each starts in them.
    liquid_time: f32,
    ripples: Vec<f32>,
    idle: bool,
    /// The eyes stay until this moment after being woken.
    waking: Option<Instant>,
    banner_active: bool,
    /// Size of the live strip (music or timer), if one shows.
    strip_size: Option<(f32, f32)>,
    /// Size of the open card (music or status), if one is open.
    card_size: Option<(f32, f32)>,
    /// Size with nothing else to show.
    resting: (f32, f32),
    /// The closed notch's size relative to the usual one; what it shows,
    /// the live strip and the wide notch scale with it.
    pub scale: f32,
    /// Hovered in click-to-open mode.
    peek: bool,
}

impl Notch {
    pub fn new(now: Instant, seed: u64) -> Self {
        Self {
            bounce: true,
            width: Spring::new(COMPACT.0, 240.0, DAMPING, 0.05),
            height: Spring::new(COMPACT.1, 240.0, DAMPING, 0.05),
            eyes_visible: Spring::new(0.0, 260.0, 32.0, 0.002),
            eyes: Eyes::new(now, seed),
            banner_visible: Spring::new(0.0, 320.0, 34.0, 0.002),
            banner: None,
            music_visible: Spring::new(0.0, 300.0, 34.0, 0.002),
            card_visible: Spring::new(0.0, 300.0, 34.0, 0.002),
            nav_visible: Spring::new(0.0, 130.0, 24.0, 0.002),
            peek_visible: Spring::new(0.0, 130.0, 24.0, 0.002),
            volume_visible: Spring::new(0.0, 200.0, 28.0, 0.002),
            notice_meter: Spring::new(0.0, METER.0, METER.1, 0.002),
            notice_beat: None,
            liquid_glow: Spring::new(0.0, 60.0, 15.0, 0.002),
            ripple_at: (0.0, 0.0),
            liquid_time: 0.0,
            ripples: Vec::new(),
            idle: false,
            waking: None,
            banner_active: false,
            strip_size: None,
            card_size: None,
            resting: COMPACT,
            scale: 1.0,
            peek: false,
        }
    }

    pub fn set_idle(&mut self, idle: bool, now: Instant) {
        if idle == self.idle {
            return;
        }
        self.idle = idle;
        if idle {
            self.waking = None;
            self.eyes.wake(now);
        } else if self.eyes_visible.value > 0.5 {
            self.waking = Some(now + self.eyes.rouse(now));
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
        // The ring fills from empty, and beats meanwhile.
        self.notice_meter = Spring::new(0.0, METER.0, METER.1, 0.002);
        self.notice_meter.target = notice.progress;
        self.notice_beat = Some(0.0);
        self.banner = Some(Banner {
            content: Content::Notice(notice),
            size: (width.clamp(NOTICE_WIDTH.0, NOTICE_WIDTH.1), NOTICE_HEIGHT),
            until: now + duration,
        });
        self.activate();
    }

    /// Announces a new song.
    pub fn show_song(&mut self, song: String, width: f32, duration: Duration, now: Instant) {
        self.banner = Some(Banner {
            content: Content::Song(song),
            size: (width.clamp(SONG_BANNER.0, SONG_BANNER_MAX), SONG_BANNER.1),
            until: now + duration,
        });
        self.activate();
    }

    /// Shows a prayer reminder.
    pub fn show_prayer(&mut self, title: String, wait: String, duration: Duration, now: Instant) {
        self.banner = Some(Banner {
            content: Content::Prayer { title, wait },
            size: (PRAYER_BANNER.0 * self.scale, PRAYER_BANNER.1 * self.scale),
            until: now + duration,
        });
        self.activate();
    }

    /// Shows a notification from another app.
    pub fn show_app(&mut self, notice: AppNotice, duration: Duration, now: Instant) {
        self.banner = Some(Banner {
            content: Content::App(notice),
            size: APP_BANNER,
            until: now + duration,
        });
        self.activate();
    }

    /// The app notification showing now, if any.
    pub fn app_banner(&self) -> Option<&AppNotice> {
        match self.banner.as_ref().filter(|_| self.banner_active)? {
            Banner {
                content: Content::App(notice),
                ..
            } => Some(notice),
            _ => None,
        }
    }

    /// Takes the banner down now, as if its time had run out.
    pub fn end_banner(&mut self, now: Instant) {
        if let Some(banner) = self.banner.as_mut() {
            banner.until = now;
        }
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
        self.notice_meter.target = notice.progress;
        banner.content = Content::Notice(notice);
        banner.size = (width.clamp(NOTICE_WIDTH.0, NOTICE_WIDTH.1), NOTICE_HEIGHT);
        banner.until = banner.until.max(now + NOTICE_REFRESH_HOLD);
        self.retarget();
        true
    }

    /// Fills the showing notice's ring, for a device that reports no level.
    pub fn fill_notice_meter(&mut self) {
        self.notice_meter.target = 1.0;
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
            || self.liquid_glow.value > 0.0
    }

    /// Advances every animation. Returns true while anything is still moving.
    pub fn step(&mut self, now: Instant, dt: f32) -> bool {
        if self.banner_active && self.banner.as_ref().is_some_and(|b| now >= b.until) {
            self.banner_active = false;
            self.retarget();
        }
        if self.waking.is_some_and(|until| now >= until) {
            self.waking = None;
            self.retarget();
        }

        let mut moving = self.width.step(dt);
        moving |= self.height.step(dt);
        moving |= self.eyes_visible.step(dt);
        moving |= self.banner_visible.step(dt);
        moving |= self.music_visible.step(dt);
        moving |= self.card_visible.step(dt);
        moving |= self.nav_visible.step(dt);
        moving |= self.peek_visible.step(dt);
        moving |= self.volume_visible.step(dt);
        moving |= self.liquid_glow.step(dt);
        moving |= self.notice_meter.step(dt);
        if let Some(beat) = self.notice_beat {
            let beat = beat + dt / BEAT_SECS;
            self.notice_beat = (beat < 1.0).then_some(beat);
            moving = true;
        }
        if !self.ripples.is_empty() {
            self.liquid_time += dt;
            moving = true;
            let time = self.liquid_time;
            self.ripples.retain(|start| time - start < RIPPLE_SECS);
        }
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
        let peek = self.peek && !banner && card.is_none();
        let strip = self
            .strip_size
            .filter(|_| !banner && card.is_none() && !peek);
        let eyes = (self.idle || self.waking.is_some())
            && !banner
            && card.is_none()
            && strip.is_none()
            && !peek;
        let grown = |(w, h): (f32, f32)| (w * self.scale, h * self.scale);
        let (w, h) = match (&self.banner, card, strip) {
            (Some(shown), _, _) if banner => shown.size,
            (_, Some(size), _) => size,
            (_, None, Some(size)) => grown(size),
            _ if peek => grown(PEEK),
            _ if eyes => IDLE,
            _ => self.resting,
        };
        self.width.target = w;
        self.height.target = h;
        // A card or the widened notch bounces open; everything else settles
        // without one.
        let damping = if self.bounce && (card.is_some() || peek) {
            CARD_DAMPING
        } else {
            DAMPING
        };
        self.width.damping = damping;
        self.height.damping = damping;
        let on = |visible: bool| if visible { 1.0 } else { 0.0 };
        self.banner_visible.target = on(banner);
        self.card_visible.target = on(card.is_some());
        self.music_visible.target = on(strip.is_some());
        self.eyes_visible.target = on(eyes);
        self.peek_visible.target = on(peek);
    }

    /// Rests at `size`, or as the thin bar when `minimal`.
    pub fn set_resting(&mut self, size: (f32, f32), minimal: bool) {
        self.scale = size.1 / COMPACT.1;
        self.resting = if minimal { MINIMAL } else { size };
        self.retarget();
    }

    /// Widens the notch to show the date, or lets it rest again.
    pub fn set_peek(&mut self, peek: bool) {
        if peek != self.peek {
            self.peek = peek;
            self.retarget();
        }
    }

    /// Turns the notch dark blue while something is dragged over it, or
    /// clears it when that leaves.
    pub fn set_liquid(&mut self, over: bool) {
        self.liquid_glow.target = if over { 1.0 } else { 0.0 };
        self.liquid_glow.stiffness = 60.0;
        self.liquid_glow.damping = 15.0;
    }

    /// Something was dropped: rings run out from where it landed, and the
    /// colour slowly goes, leaving the notch black.
    pub fn settle_liquid(&mut self) {
        let time = self.liquid_time;
        self.ripples.extend([time, time + 0.22, time + 0.44]);
        self.liquid_glow.value = self.liquid_glow.value.max(1.0);
        self.liquid_glow.target = 0.0;
        self.liquid_glow.stiffness = LIQUID_FADE.0;
        self.liquid_glow.damping = LIQUID_FADE.1;
    }

    /// The ripples showing now, each as its radius and strength.
    pub fn ripple_rings(&self) -> Vec<(f32, f32)> {
        let time = self.liquid_time;
        self.ripples
            .iter()
            .filter(|&&start| start <= time)
            .map(|start| ripple_ring(time - start))
            .collect()
    }

    /// Opens or closes the music card's volume slider.
    pub fn set_volume(&mut self, open: bool) {
        self.volume_visible.target = if open { 1.0 } else { 0.0 };
    }

    /// Turns the page dots into page icons, or back.
    pub fn set_nav(&mut self, open: bool) {
        self.nav_visible.target = if open { 1.0 } else { 0.0 };
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_bell_shakes_then_rests() {
        let now = Instant::now();
        let mut bell = Bell::new(now);
        assert!(bell.step(now, true));
        assert!(bell.step(now + Duration::from_millis(40), true));
        assert!(bell.angle.abs() > 5.0);
        assert!(bell.step(now + Duration::from_secs(1), true));
        assert_eq!(bell.angle, 0.0);
        assert!(!bell.step(now + Duration::from_secs(2), true));
        assert!(bell.step(now + Duration::from_secs(1) + BELL_REST, true));
    }

    #[test]
    fn a_hidden_bell_shakes_when_it_comes_back() {
        let now = Instant::now();
        let mut bell = Bell::new(now);
        bell.step(now, true);
        assert!(bell.step(now, false));
        assert!(!bell.step(now, false));
        assert!(bell.step(now, true));
    }

    #[test]
    fn a_drop_sends_rings_out_and_fades_to_black() {
        let now = Instant::now();
        let mut notch = Notch::new(now, 7);
        notch.set_liquid(true);
        for _ in 0..120 {
            notch.step(now, 1.0 / 60.0);
        }
        assert!(notch.liquid_glow.value > 0.95);
        assert!(notch.ripple_rings().is_empty());
        notch.set_liquid(false);
        notch.settle_liquid();
        for _ in 0..45 {
            notch.step(now, 1.0 / 60.0);
        }
        assert!(notch.liquid_glow.value > 0.3);
        assert!(notch.ripple_rings().len() >= 3);
        for _ in 0..1200 {
            notch.step(now, 1.0 / 60.0);
        }
        assert_eq!(notch.liquid_glow.value, 0.0);
        assert!(notch.ripple_rings().is_empty());
    }

    #[test]
    fn ripples_spread_and_die_away() {
        let (first, strong) = ripple_ring(0.2);
        let (later, weak) = ripple_ring(1.2);
        assert!(later > first);
        assert!(weak < strong);
        assert_eq!(ripple_ring(RIPPLE_SECS).1, 0.0);
    }

    fn notice(progress: f32) -> Notice {
        Notice {
            icon: "",
            motion: Motion::Pulse,
            accent: Accent::Blue,
            progress,
            title: "A".to_string(),
            subtitle: String::new(),
            trailing: Trailing::None,
            dim: false,
        }
    }

    #[test]
    fn a_notice_ring_fills_to_its_level_and_stops_beating() {
        let now = Instant::now();
        let mut notch = Notch::new(now, 7);
        notch.show_notice(notice(0.7), 200.0, NOTICE_DURATION, now);
        assert_eq!(notch.notice_meter.value, 0.0);
        assert!(notch.notice_beat.is_some());
        for _ in 0..180 {
            notch.step(now, 1.0 / 60.0);
        }
        assert!((notch.notice_meter.value - 0.7).abs() < 0.01);
        assert!(notch.notice_beat.is_none());
    }

    #[test]
    fn a_refreshed_notice_moves_its_ring_on() {
        let now = Instant::now();
        let mut notch = Notch::new(now, 7);
        notch.show_notice(notice(0.0), 200.0, NOTICE_DURATION, now);
        assert!(notch.refresh_notice(notice(0.5), 200.0, now));
        assert_eq!(notch.notice_meter.target, 0.5);
    }

    #[test]
    fn icons_start_and_end_at_rest() {
        let motions = [
            Motion::Pulse,
            Motion::Blink,
            Motion::Walk,
            Motion::Bob,
            Motion::Shake,
            Motion::Flash,
        ];
        for motion in motions {
            for t in [0.0, 1.0] {
                let pose = motion.pose(t);
                assert!(pose.dx.abs() < 0.01 && pose.dy.abs() < 0.01, "{motion:?}");
                assert!(pose.angle.abs() < 0.01, "{motion:?}");
                assert!((pose.scale_y - 1.0).abs() < 0.01, "{motion:?}");
                assert!((pose.alpha - 1.0).abs() < 0.01, "{motion:?}");
            }
        }
    }

    #[test]
    fn eyes_close_while_blinking_and_steps_sway() {
        assert!(Motion::Blink.pose(0.25).scale_y < 0.2);
        assert_eq!(Motion::Blink.pose(0.45).scale_y, 1.0);
        assert!(Motion::Walk.pose(1.0 / 12.0).dx > 1.0);
    }

    #[test]
    fn eyes_tire_with_time() {
        assert_eq!(Eyes::mood(0.0), 0.0);
        assert_eq!(Eyes::mood(DROWSY_AFTER), 0.5);
        assert_eq!(Eyes::mood(ASLEEP_AFTER), 1.0);
    }

    #[test]
    fn only_sleepy_eyes_are_startled() {
        let now = Instant::now();
        let mut eyes = Eyes::new(now, 7);
        assert_eq!(eyes.rouse(now), GREET);
        assert_eq!(eyes.surprise.target, 0.0);
        eyes.sleep.value = 0.6;
        assert_eq!(eyes.rouse(now), STARTLE + GREET);
        assert_eq!(eyes.sleep.value, 0.0);
        assert_eq!(eyes.surprise.target, 1.0);
    }

    #[test]
    fn the_greeting_hops_and_ends_still() {
        let now = Instant::now();
        let mut eyes = Eyes::new(now, 7);
        eyes.step_greeting(now + GREET / 4, now);
        assert!(eyes.hop > 2.0);
        assert!(eyes.greeting > 0.9);
        eyes.step_greeting(now + GREET, now);
        assert!(eyes.hop.abs() < 0.01);
        assert_eq!(eyes.greeting, 0.0);
    }

    #[test]
    fn a_woken_notch_keeps_its_eyes_a_moment() {
        let now = Instant::now();
        let mut notch = Notch::new(now, 7);
        notch.set_idle(true, now);
        notch.eyes_visible.value = 1.0;
        notch.eyes.sleep.value = 1.0;
        notch.set_idle(false, now);
        assert_eq!(notch.eyes_visible.target, 1.0);
        notch.step(now + STARTLE + GREET, 0.016);
        assert_eq!(notch.eyes_visible.target, 0.0);
    }
}
