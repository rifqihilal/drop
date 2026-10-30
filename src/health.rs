//! Health reminders: drink water, rest the eyes, stretch. Each counts the
//! minutes the computer is in use; a long enough break away from it starts
//! every count over.

/// Minutes without input that count as a break.
pub const BREAK_MINUTES: u32 = 5;

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Kind {
    Water,
    Eyes,
    Stretch,
}

pub const KINDS: [Kind; 3] = [Kind::Water, Kind::Eyes, Kind::Stretch];

/// Which reminders are on and how many minutes of use each waits.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Plan {
    pub water: Option<u32>,
    pub eyes: Option<u32>,
    pub stretch: Option<u32>,
}

impl Plan {
    pub fn interval(&self, kind: Kind) -> Option<u32> {
        match kind {
            Kind::Water => self.water,
            Kind::Eyes => self.eyes,
            Kind::Stretch => self.stretch,
        }
    }
}

#[derive(Default)]
pub struct Health {
    /// Minutes of use since each reminder last went off, or since a break.
    used: [u32; 3],
}

impl Health {
    /// Called once a minute. `idle_minutes` is how long there has been no
    /// input; `quiet` holds reminders back (a full-screen app) until it ends.
    /// Returns the reminder that is due, at most one a minute.
    pub fn minute(&mut self, plan: &Plan, idle_minutes: u32, quiet: bool) -> Option<Kind> {
        if idle_minutes >= BREAK_MINUTES {
            self.used = [0; 3];
            return None;
        }
        if idle_minutes > 0 {
            return None;
        }
        for (used, kind) in self.used.iter_mut().zip(KINDS) {
            if plan.interval(kind).is_some() {
                *used += 1;
            }
        }
        if quiet {
            return None;
        }
        let due = KINDS.iter().position(|&kind| {
            plan.interval(kind)
                .is_some_and(|every| self.used[kind as usize] >= every)
        })?;
        self.used[due] = 0;
        Some(KINDS[due])
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const PLAN: Plan = Plan {
        water: Some(3),
        eyes: None,
        stretch: Some(5),
    };

    fn run(health: &mut Health, minutes: u32, idle: u32, quiet: bool) -> Vec<Kind> {
        (0..minutes)
            .filter_map(|_| health.minute(&PLAN, idle, quiet))
            .collect()
    }

    #[test]
    fn reminds_after_minutes_of_use() {
        let mut health = Health::default();
        assert_eq!(run(&mut health, 2, 0, false), []);
        assert_eq!(run(&mut health, 1, 0, false), [Kind::Water]);
        assert_eq!(run(&mut health, 2, 0, false), [Kind::Stretch]);
        assert_eq!(run(&mut health, 1, 0, false), [Kind::Water]);
    }

    #[test]
    fn a_break_starts_over() {
        let mut health = Health::default();
        run(&mut health, 2, 0, false);
        // A short pause neither counts nor resets.
        assert_eq!(run(&mut health, 3, 2, false), []);
        assert_eq!(run(&mut health, 1, 0, false), [Kind::Water]);
        run(&mut health, 2, 0, false);
        run(&mut health, 1, BREAK_MINUTES, false);
        assert_eq!(run(&mut health, 2, 0, false), []);
    }

    #[test]
    fn full_screen_holds_reminders_back() {
        let mut health = Health::default();
        assert_eq!(run(&mut health, 4, 0, true), []);
        assert_eq!(run(&mut health, 1, 0, false), [Kind::Water]);
        assert_eq!(run(&mut health, 1, 0, false), [Kind::Stretch]);
    }
}
