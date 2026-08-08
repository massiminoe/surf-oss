//! Turns per-tick player state into discrete audio events.
//!
//! Lives here rather than in the app so the fiddly parts — bhop chaining, and
//! not firing a spurious landing the tick after a wipe teleports you — are
//! unit-testable without a window or an audio device.

use crate::voice::AudioEvent;

/// Below this, ramp contact is a scrape rather than a catch worth announcing.
const CATCH_MIN_SPEED: f32 = 200.0;

/// Two landings closer together than this count as a chain. A full-height CS
/// jump is airborne for ~0.75 s, so this has to be comfortably above that.
const BHOP_WINDOW: f32 = 1.0;

/// Highest chain index the click steps to before holding.
const MAX_BHOP_STEP: u8 = 8;

/// What the sim knows at the end of a tick.
#[derive(Clone, Copy, Debug)]
pub struct Observation {
    pub dt: f32,
    /// Horizontal speed, u/s.
    pub speed: f32,
    /// Touching a surf ramp (non-walkable face).
    pub on_ramp: bool,
    /// Standing on walkable ground.
    pub grounded: bool,
    /// Kill-z or fail teleport fired this tick.
    pub wiped: bool,
}

/// Up to three events from one tick, without allocating.
#[derive(Default, Clone, Copy, Debug)]
pub struct Emitted {
    items: [Option<AudioEvent>; 3],
}

impl Emitted {
    fn push(&mut self, ev: AudioEvent) {
        if let Some(slot) = self.items.iter_mut().find(|s| s.is_none()) {
            *slot = Some(ev);
        }
    }

    pub fn iter(&self) -> impl Iterator<Item = AudioEvent> + '_ {
        self.items.iter().filter_map(|s| *s)
    }

    pub fn len(&self) -> usize {
        self.items.iter().filter(|s| s.is_some()).count()
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }
}

#[derive(Clone, Copy, Debug)]
pub struct EventDetector {
    was_on_ramp: bool,
    was_grounded: bool,
    since_land: f32,
    bhop_step: u8,
    started: bool,
}

impl Default for EventDetector {
    fn default() -> Self {
        Self {
            was_on_ramp: false,
            was_grounded: true,
            since_land: f32::MAX / 4.0,
            bhop_step: 0,
            started: false,
        }
    }
}

impl EventDetector {
    /// Forget edge history — used on a full reset so the next tick can't fire a
    /// landing or a catch left over from the previous run.
    pub fn resync(&mut self, on_ramp: bool, grounded: bool) {
        self.was_on_ramp = on_ramp;
        self.was_grounded = grounded;
        self.bhop_step = 0;
        self.since_land = f32::MAX / 4.0;
        self.started = true;
    }

    pub fn observe(&mut self, o: Observation) -> Emitted {
        let mut out = Emitted::default();
        self.since_land = (self.since_land + o.dt).min(f32::MAX / 4.0);

        // First tick only establishes a baseline; a spawn is not a landing.
        if !self.started {
            self.resync(o.on_ramp, o.grounded);
            return out;
        }

        if o.wiped {
            // The teleport itself is the event. Everything else this tick is an
            // artefact of being moved, so swallow it and re-baseline.
            out.push(AudioEvent::Wipe);
            self.resync(o.on_ramp, o.grounded);
            return out;
        }

        if o.on_ramp && !self.was_on_ramp && o.speed >= CATCH_MIN_SPEED {
            out.push(AudioEvent::Catch { speed: o.speed });
        } else if !o.on_ramp && self.was_on_ramp && o.speed >= CATCH_MIN_SPEED {
            out.push(AudioEvent::Release { speed: o.speed });
        }

        if o.grounded && !self.was_grounded {
            self.bhop_step = if self.since_land <= BHOP_WINDOW {
                (self.bhop_step + 1).min(MAX_BHOP_STEP)
            } else {
                0
            };
            self.since_land = 0.0;
            out.push(AudioEvent::Land {
                step: self.bhop_step,
                speed: o.speed,
            });
        }

        self.was_on_ramp = o.on_ramp;
        self.was_grounded = o.grounded;
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const DT: f32 = 0.015;

    fn obs(speed: f32, on_ramp: bool, grounded: bool) -> Observation {
        Observation {
            dt: DT,
            speed,
            on_ramp,
            grounded,
            wiped: false,
        }
    }

    fn collect(d: &mut EventDetector, o: Observation) -> Vec<AudioEvent> {
        d.observe(o).iter().collect()
    }

    #[test]
    fn first_tick_is_a_baseline_not_a_landing() {
        let mut d = EventDetector::default();
        let evs = collect(&mut d, obs(0.0, false, true));
        assert!(evs.is_empty(), "spawn emitted {evs:?}");
    }

    #[test]
    fn ramp_entry_and_exit_emit_catch_then_release() {
        let mut d = EventDetector::default();
        collect(&mut d, obs(800.0, false, false));

        let on = collect(&mut d, obs(800.0, true, false));
        assert_eq!(on, vec![AudioEvent::Catch { speed: 800.0 }]);

        // Held contact emits nothing further.
        assert!(collect(&mut d, obs(900.0, true, false)).is_empty());

        let off = collect(&mut d, obs(1000.0, false, false));
        assert_eq!(off, vec![AudioEvent::Release { speed: 1000.0 }]);
    }

    #[test]
    fn slow_scrapes_do_not_announce_themselves() {
        let mut d = EventDetector::default();
        collect(&mut d, obs(50.0, false, false));
        assert!(collect(&mut d, obs(50.0, true, false)).is_empty());
    }

    #[test]
    fn consecutive_landings_step_the_chain_and_a_gap_resets_it() {
        let mut d = EventDetector::default();
        collect(&mut d, obs(600.0, false, true));

        let mut steps = Vec::new();
        for _ in 0..4 {
            // Airborne for ~0.75 s, then land.
            for _ in 0..50 {
                collect(&mut d, obs(600.0, false, false));
            }
            for ev in collect(&mut d, obs(600.0, false, true)) {
                if let AudioEvent::Land { step, .. } = ev {
                    steps.push(step);
                }
            }
        }
        assert_eq!(steps, vec![0, 1, 2, 3], "chain should climb");

        // Long pause on the ground, then a fresh landing restarts at 0.
        for _ in 0..200 {
            collect(&mut d, obs(600.0, false, true));
        }
        for _ in 0..50 {
            collect(&mut d, obs(600.0, false, false));
        }
        let evs = collect(&mut d, obs(600.0, false, true));
        assert_eq!(evs, vec![AudioEvent::Land { step: 0, speed: 600.0 }]);
    }

    #[test]
    fn chain_saturates_rather_than_running_away() {
        let mut d = EventDetector::default();
        collect(&mut d, obs(600.0, false, true));
        let mut last = 0;
        for _ in 0..30 {
            for _ in 0..40 {
                collect(&mut d, obs(600.0, false, false));
            }
            for ev in collect(&mut d, obs(600.0, false, true)) {
                if let AudioEvent::Land { step, .. } = ev {
                    last = step;
                }
            }
        }
        assert_eq!(last, MAX_BHOP_STEP);
    }

    #[test]
    fn wipe_swallows_the_teleport_artefacts() {
        let mut d = EventDetector::default();
        // Surfing fast, airborne.
        collect(&mut d, obs(1500.0, true, false));

        // Kill-z: teleported to spawn, on the ground, off the ramp. Naively this
        // would read as a release *and* a landing on the same tick.
        let evs = collect(
            &mut d,
            Observation {
                dt: DT,
                speed: 0.0,
                on_ramp: false,
                grounded: true,
                wiped: true,
            },
        );
        assert_eq!(evs, vec![AudioEvent::Wipe]);

        // And the tick after is quiet, not a phantom landing.
        assert!(collect(&mut d, obs(0.0, false, true)).is_empty());
    }

    #[test]
    fn resync_prevents_a_phantom_event_after_reset() {
        let mut d = EventDetector::default();
        collect(&mut d, obs(1500.0, true, false));
        d.resync(false, true);
        assert!(collect(&mut d, obs(0.0, false, true)).is_empty());
    }

    #[test]
    fn emitted_holds_at_most_three_and_reports_its_length() {
        let mut e = Emitted::default();
        assert!(e.is_empty());
        for _ in 0..5 {
            e.push(AudioEvent::Rearm);
        }
        assert_eq!(e.len(), 3);
        assert_eq!(e.iter().count(), 3);
    }
}
