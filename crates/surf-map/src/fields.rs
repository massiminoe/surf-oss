//! Touch-edge trigger effects: boosters, anti-gravity pads, name gates.
//!
//! `trigger_push` and `trigger_gravity` are stateless — you either stand in one
//! or you don't. The effects in this module are not: a map builds a booster out
//! of `trigger_multiple` + `OnEndTouch !activator,AddOutput,basevelocity …`, so
//! reproducing it needs to know the tick the player *left* the volume, and a
//! one-shot boost needs to remember that the player has already been renamed.
//! `FieldState` is that memory; it lives outside `tick()`, feeding it the same
//! `basevelocity` / `gravity_scale` inputs a push volume does.
//!
//! It is small and copyable on purpose: the resim harness and the app each keep
//! their own, so a replay reproduces the boosts it recorded.

use surf_core::math::Vec3;
use surf_core::movement::Hull;

use crate::entities::{FieldAction, FieldTrigger};

/// What the map wants done to the player this tick.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct FieldEffects {
    /// Basevelocity still being asserted — carried by the move, not kept.
    pub basevelocity: Vec3,
    /// Basevelocity nothing asserts any more: fold it into velocity for good.
    pub released: Vec3,
    /// Gravity multiplier latched by the last output to set one.
    pub gravity_scale: f32,
}

impl Default for FieldEffects {
    fn default() -> Self {
        Self {
            basevelocity: Vec3::ZERO,
            released: Vec3::ZERO,
            gravity_scale: 1.0,
        }
    }
}

#[derive(Clone, Debug)]
pub struct FieldState {
    /// Per-trigger: were we inside it last tick?
    touching: Vec<bool>,
    /// The player's `targetname`. Maps rename the player to gate a boost behind
    /// a `filter_activator_name` — that is how tendies' 1337 start boost fires
    /// once per run and re-arms only when you re-enter the start zone.
    name: String,
    /// Basevelocity asserted last tick, waiting to be released.
    pending: Vec3,
    gravity_scale: f32,
}

impl Default for FieldState {
    fn default() -> Self {
        Self {
            touching: Vec::new(),
            name: String::new(),
            pending: Vec3::ZERO,
            gravity_scale: 1.0,
        }
    }
}

impl FieldState {
    pub fn new() -> Self {
        Self::default()
    }

    /// Forget everything. Called on respawn: a fresh run re-arms every one-shot.
    pub fn reset(&mut self) {
        *self = Self::default();
    }

    pub fn player_name(&self) -> &str {
        &self.name
    }

    /// Advance one tick. `asserted_push` is this tick's continuous
    /// `trigger_push` sum, which shares the basevelocity slot.
    pub fn update(
        &mut self,
        triggers: &[FieldTrigger],
        origin: Vec3,
        asserted_push: Vec3,
    ) -> FieldEffects {
        if self.touching.len() != triggers.len() {
            self.touching = vec![false; triggers.len()];
        }

        // Hull overlap, not a point probe: booster volumes are routinely a few
        // units thick and a centre sample tunnels straight through them.
        let hull = Hull::css_stand();
        let mins = origin + hull.mins;
        let maxs = origin + hull.maxs;

        let mut released = Vec3::ZERO;
        for (i, trigger) in triggers.iter().enumerate() {
            let was = self.touching[i];
            let inside = overlaps(trigger, mins, maxs);
            // The filter decides who the trigger *starts* touching. Once you are
            // in its touch list you stay there until you physically leave —
            // otherwise renaming yourself mid-volume (which is exactly what
            // these triggers do) would fire OnEndTouch on the spot.
            let allowed = trigger.filter.as_ref().is_none_or(|f| f.passes(&self.name));

            if inside && !was && allowed {
                for action in &trigger.on_start {
                    self.apply(action, &mut released);
                }
                self.touching[i] = true;
            } else if !inside && was {
                for action in &trigger.on_end {
                    self.apply(action, &mut released);
                }
                self.touching[i] = false;
            }
        }

        // Source re-asserts basevelocity every tick a push volume touches you;
        // the tick it stops, the pending amount is folded into velocity.
        if asserted_push == Vec3::ZERO {
            released += self.pending;
            self.pending = Vec3::ZERO;
        } else {
            // `tick` consumes basevelocity.z during StartGravity and zeroes it,
            // so only the horizontal part is still pending on release.
            self.pending = Vec3::new(asserted_push.x, asserted_push.y, 0.0);
        }

        FieldEffects {
            basevelocity: asserted_push,
            released,
            gravity_scale: self.gravity_scale,
        }
    }

    fn apply(&mut self, action: &FieldAction, released: &mut Vec3) {
        match action {
            // Nothing ever re-asserts an AddOutput basevelocity, so it releases
            // on the spot — that is what makes it read as a one-shot impulse.
            FieldAction::BaseVelocity(v) => *released += *v,
            FieldAction::Gravity(g) => self.gravity_scale = *g,
            FieldAction::TargetName(n) => self.name = n.clone(),
        }
    }
}

/// Source's box-vs-box trigger test is exact. A padded one keeps the player
/// "inside" for an extra tick after they have left, which shifts a booster's
/// payout a tick late — visible against tendies' WR, where the hull clears the
/// 4u-thick slab by less than a unit.
pub(crate) const TOUCH_PAD: f32 = 0.0;

fn overlaps(trigger: &FieldTrigger, mins: Vec3, maxs: Vec3) -> bool {
    let expanded = trigger.bounds.expand(1.0);
    if !crate::aabb_overlap(mins, maxs, expanded.mins, expanded.maxs) {
        return false;
    }
    trigger
        .brushes
        .iter()
        .any(|b| crate::brush_intersects_aabb(b, mins, maxs, TOUCH_PAD))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::entities::NameFilter;
    use surf_core::{Aabb, Brush};

    fn box_trigger(
        mins: Vec3,
        maxs: Vec3,
        on_start: Vec<FieldAction>,
        on_end: Vec<FieldAction>,
        filter: Option<NameFilter>,
    ) -> FieldTrigger {
        FieldTrigger {
            brushes: vec![Brush::aabb(mins, maxs)],
            bounds: Aabb::from_mins_maxs(mins, maxs),
            on_start,
            on_end,
            filter,
        }
    }

    /// tendies' start boost, entity for entity: a boost on leaving, gated by a
    /// negated name filter the trigger itself sets. Must fire exactly once.
    #[test]
    fn a_name_gated_boost_fires_once_and_rearms_only_on_rename() {
        let boost = box_trigger(
            Vec3::new(-64.0, -64.0, 0.0),
            Vec3::new(64.0, 64.0, 128.0),
            vec![FieldAction::TargetName("bhopped".into())],
            vec![FieldAction::BaseVelocity(Vec3::new(1337.0, 0.0, 0.0))],
            Some(NameFilter {
                name: "bhopped".into(),
                negated: true,
            }),
        );
        let triggers = [boost];
        let inside = Vec3::new(0.0, 0.0, 8.0);
        let outside = Vec3::new(1000.0, 0.0, 8.0);
        let mut state = FieldState::new();

        assert_eq!(state.update(&triggers, inside, Vec3::ZERO).released, Vec3::ZERO);
        assert_eq!(state.player_name(), "bhopped");
        let out = state.update(&triggers, outside, Vec3::ZERO);
        assert_eq!(out.released, Vec3::new(1337.0, 0.0, 0.0));

        // Second lap: the filter now rejects us, so no boost.
        state.update(&triggers, inside, Vec3::ZERO);
        let out = state.update(&triggers, outside, Vec3::ZERO);
        assert_eq!(out.released, Vec3::ZERO);

        // Re-entering the start zone renames us back, re-arming the boost.
        state.name = "default".into();
        state.update(&triggers, inside, Vec3::ZERO);
        let out = state.update(&triggers, outside, Vec3::ZERO);
        assert_eq!(out.released, Vec3::new(1337.0, 0.0, 0.0));
    }

    /// lovetunnel's launch pads: gravity is latched on entry, restored on exit.
    #[test]
    fn gravity_outputs_latch_until_something_sets_them_back() {
        let pad = box_trigger(
            Vec3::new(-64.0, -64.0, 0.0),
            Vec3::new(64.0, 64.0, 128.0),
            vec![FieldAction::Gravity(-100.0)],
            vec![FieldAction::Gravity(1.0)],
            None,
        );
        let triggers = [pad];
        let inside = Vec3::new(0.0, 0.0, 8.0);
        let outside = Vec3::new(1000.0, 0.0, 8.0);
        let mut state = FieldState::new();

        assert_eq!(state.update(&triggers, outside, Vec3::ZERO).gravity_scale, 1.0);
        assert_eq!(state.update(&triggers, inside, Vec3::ZERO).gravity_scale, -100.0);
        // Latched: still inverted on later ticks inside.
        assert_eq!(state.update(&triggers, inside, Vec3::ZERO).gravity_scale, -100.0);
        assert_eq!(state.update(&triggers, outside, Vec3::ZERO).gravity_scale, 1.0);
    }

    /// A push carries you while you are in it and pays out when you leave.
    #[test]
    fn push_basevelocity_releases_on_exit_horizontally_only() {
        let mut state = FieldState::new();
        let push = Vec3::new(-3500.0, 0.0, 1000.0);
        let at = Vec3::ZERO;

        let eff = state.update(&[], at, push);
        assert_eq!(eff.basevelocity, push);
        assert_eq!(eff.released, Vec3::ZERO);

        // Still inside: nothing paid out yet.
        assert_eq!(state.update(&[], at, push).released, Vec3::ZERO);

        // Out: the horizontal part folds in. Z was already spent by StartGravity
        // on each tick inside, so it must not be paid twice.
        let eff = state.update(&[], at, Vec3::ZERO);
        assert_eq!(eff.released, Vec3::new(-3500.0, 0.0, 0.0));
        assert_eq!(state.update(&[], at, Vec3::ZERO).released, Vec3::ZERO);
    }
}
