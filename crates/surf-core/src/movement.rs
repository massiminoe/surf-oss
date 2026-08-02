//! Movement primitives and player/config state (research 01 §2–§4).

use crate::math::{Angle, Vec3};

/// Trace / Categorize epsilon from Source.
pub const DIST_EPSILON: f32 = crate::trace::DIST_EPSILON;
pub const STOP_EPSILON: f32 = 0.1;
pub const NON_JUMP_VELOCITY: f32 = 140.0;
pub const GROUND_PROBE: f32 = 2.0;
pub const WALKABLE_NORMAL_Z: f32 = 0.7;
pub const MAX_CLIP_PLANES: usize = 5;
pub const AIR_SPEED_CAP: f32 = 30.0;
pub const COORD_RESOLUTION: f32 = 1.0 / 32.0;

/// CS:S standing / duck hulls (Momentum-faithful). Origin at feet.
#[derive(Clone, Copy, Debug)]
pub struct Hull {
    pub mins: Vec3,
    pub maxs: Vec3,
    pub eye_height: f32,
}

impl Hull {
    pub const fn css_stand() -> Self {
        Self {
            mins: Vec3::new(-16.0, -16.0, 0.0),
            maxs: Vec3::new(16.0, 16.0, 62.0),
            eye_height: 64.0,
        }
    }

    pub const fn css_duck() -> Self {
        Self {
            mins: Vec3::new(-16.0, -16.0, 0.0),
            maxs: Vec3::new(16.0, 16.0, 45.0),
            eye_height: 47.0,
        }
    }
}

/// Toggleable engine-bug fixes (research 01 §9). Defaults match Momentum "fixed CS:S".
#[derive(Clone, Copy, Debug)]
pub struct BugFixes {
    /// Force surfaceFriction = 1.0 while airborne (kill deadstrafe 0.25).
    pub fix_deadstrafe: bool,
    /// Don't zero velocity on stuck/full-fraction precision failures while airborne.
    pub fix_ramp: bool,
    /// On proximity landing, ClipVelocity before zeroing vz (downhill boost fix).
    pub fix_slope: bool,
}

impl Default for BugFixes {
    fn default() -> Self {
        Self {
            fix_deadstrafe: true,
            fix_ramp: true,
            fix_slope: true,
        }
    }
}

/// Movevars — Momentum-style "fixed CS:S surf" defaults.
#[derive(Clone, Copy, Debug)]
pub struct MoveVars {
    pub gravity: f32,
    pub friction: f32,
    pub stopspeed: f32,
    pub accelerate: f32,
    pub airaccelerate: f32,
    pub maxspeed: f32,
    pub maxvelocity: f32,
    pub stepsize: f32,
    pub bounce: f32,
    pub tick_interval: f32,
    /// √(2·gravity·57) for CS jump.
    pub jump_impulse: f32,
    pub air_wishspeed_cap: f32,
    pub forward_speed: f32,
    pub side_speed: f32,
    pub autobhop: bool,
    /// When true, clamp speed to 1.1×maxspeed on jump (stock CS). Off by default.
    pub bhop_clamp: bool,
    pub fixes: BugFixes,
}

impl Default for MoveVars {
    fn default() -> Self {
        Self::momentum_surf()
    }
}

impl MoveVars {
    pub fn momentum_surf() -> Self {
        Self {
            gravity: 800.0,
            friction: 4.0,
            stopspeed: 75.0,
            accelerate: 5.0,
            airaccelerate: 150.0,
            maxspeed: 260.0,
            maxvelocity: 3500.0,
            stepsize: 18.0,
            bounce: 0.0,
            tick_interval: 0.015,
            jump_impulse: 301.993_377,
            air_wishspeed_cap: AIR_SPEED_CAP,
            forward_speed: 450.0,
            side_speed: 450.0,
            autobhop: true,
            bhop_clamp: false,
            fixes: BugFixes::default(),
        }
    }
}

/// Per-tick input (dt comes from MoveVars).
#[derive(Clone, Copy, Debug, Default)]
pub struct UserCmd {
    /// View angles at this tick (degrees).
    pub viewangles: Angle,
    /// -1..1 wish forward (will be scaled by forward_speed).
    pub forward_move: f32,
    /// -1..1 wish side (+right).
    pub side_move: f32,
    pub jump: bool,
    pub duck: bool,
}

/// Simulated player. Origin at feet.
#[derive(Clone, Debug)]
pub struct PlayerState {
    pub origin: Vec3,
    pub velocity: Vec3,
    pub viewangles: Angle,
    pub grounded: bool,
    pub ducked: bool,
    /// Buttons from previous tick (for jump edge / pogo).
    pub old_jump: bool,
    pub surface_friction: f32,
    /// Walkable ground normal when grounded (for slope fix).
    pub ground_normal: Vec3,
}

impl Default for PlayerState {
    fn default() -> Self {
        Self {
            origin: Vec3::ZERO,
            velocity: Vec3::ZERO,
            viewangles: Angle::ZERO,
            grounded: false,
            ducked: false,
            old_jump: false,
            surface_friction: 1.0,
            ground_normal: Vec3::Z,
        }
    }
}

impl PlayerState {
    pub fn hull(&self) -> Hull {
        if self.ducked {
            Hull::css_duck()
        } else {
            Hull::css_stand()
        }
    }

    pub fn eye_position(&self) -> Vec3 {
        let h = self.hull();
        self.origin + Vec3::new(0.0, 0.0, h.eye_height)
    }
}

// ---------------------------------------------------------------------------
// Pure helpers (unit-testable)
// ---------------------------------------------------------------------------

pub fn check_velocity(v: &mut Vec3, origin: &mut Vec3, maxvelocity: f32) {
    for c in [&mut v.x, &mut v.y, &mut v.z] {
        if c.is_nan() {
            *c = 0.0;
        }
        *c = c.clamp(-maxvelocity, maxvelocity);
    }
    for c in [&mut origin.x, &mut origin.y, &mut origin.z] {
        if c.is_nan() {
            *c = 0.0;
        }
    }
}

pub fn friction(velocity: &mut Vec3, grounded: bool, surface_friction: f32, vars: &MoveVars) {
    let speed = velocity.length();
    if speed < 0.1 {
        return;
    }
    let mut drop = 0.0;
    if grounded {
        let friction = vars.friction * surface_friction;
        let control = speed.max(vars.stopspeed);
        drop = control * friction * vars.tick_interval;
    }
    let newspeed = (speed - drop).max(0.0);
    *velocity *= newspeed / speed;
}

pub fn accelerate(
    velocity: &mut Vec3,
    wishdir: Vec3,
    wishspeed: f32,
    accel: f32,
    surface_friction: f32,
    dt: f32,
) {
    let currentspeed = velocity.dot(wishdir);
    let addspeed = wishspeed - currentspeed;
    if addspeed <= 0.0 {
        return;
    }
    let mut accelspeed = accel * dt * wishspeed * surface_friction;
    if accelspeed > addspeed {
        accelspeed = addspeed;
    }
    *velocity += wishdir * accelspeed;
}

pub fn air_accelerate(
    velocity: &mut Vec3,
    wishdir: Vec3,
    wishspeed: f32,
    accel: f32,
    surface_friction: f32,
    dt: f32,
    air_cap: f32,
) {
    let wishspd = wishspeed.min(air_cap);
    let currentspeed = velocity.dot(wishdir);
    let addspeed = wishspd - currentspeed;
    if addspeed <= 0.0 {
        return;
    }
    // Uses *uncapped* wishspeed for accelspeed magnitude.
    let mut accelspeed = accel * wishspeed * dt * surface_friction;
    if accelspeed > addspeed {
        accelspeed = addspeed;
    }
    *velocity += wishdir * accelspeed;
}

/// Clip velocity onto a plane. Overbounce 1.0 for player movement.
pub fn clip_velocity(inn: Vec3, normal: Vec3, overbounce: f32) -> Vec3 {
    let backoff = inn.dot(normal) * overbounce;
    let mut out = inn - normal * backoff;
    let adjust = out.dot(normal);
    if adjust < 0.0 {
        out -= normal * adjust;
    }
    out
}

/// Build horizontal wishdir / wishspeed from view + move inputs.
pub fn wish_move(viewangles: Angle, fmove: f32, smove: f32, maxspeed: f32) -> (Vec3, f32) {
    let (mut forward, mut right, _) = viewangles.vectors();
    forward.z = 0.0;
    right.z = 0.0;
    forward = forward.normalize();
    right = right.normalize();

    let mut wishvel = forward * fmove + right * smove;
    wishvel.z = 0.0;
    let mut wishspeed = wishvel.length();
    if wishspeed > maxspeed && wishspeed > 0.0 {
        wishvel *= maxspeed / wishspeed;
        wishspeed = maxspeed;
    }
    let wishdir = wishvel.normalize();
    (wishdir, wishspeed)
}

/// Clamp (fmove, smove) 2D magnitude to maxspeed (CheckParameters subset).
pub fn clamp_input_speed(fmove: f32, smove: f32, maxspeed: f32) -> (f32, f32) {
    let speed_sq = fmove * fmove + smove * smove;
    if speed_sq > maxspeed * maxspeed && speed_sq > 0.0 {
        let scale = maxspeed / speed_sq.sqrt();
        (fmove * scale, smove * scale)
    } else {
        (fmove, smove)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn clip_preserves_speed_on_parallel_glide() {
        // Velocity already in plane (dot normal ≈ 0): |v| unchanged.
        let n = Vec3::new(0.0, 0.5, 0.866_025_4).normalize(); // ~60° ramp
        let along = Vec3::new(1.0, 0.0, 0.0); // horizontal along ramp edge
        // Make truly parallel: remove normal component.
        let v = along - n * along.dot(n);
        let v = v.normalize() * 1000.0;
        let out = clip_velocity(v, n, 1.0);
        assert!((out.length() - v.length()).abs() < 0.01);
    }

    #[test]
    fn perpendicular_strafe_gain_formula() {
        // Instant regime: p=0 → Δ(v²)=900.
        let mut v = Vec3::new(500.0, 0.0, 0.0);
        let wishdir = Vec3::new(0.0, 1.0, 0.0);
        let before = v.length_squared();
        air_accelerate(&mut v, wishdir, 260.0, 150.0, 1.0, 0.015, 30.0);
        let after = v.length_squared();
        assert!((after - before - 900.0).abs() < 0.1, "Δv²={}", after - before);
    }

    #[test]
    fn friction_bleeds_250_toward_zero() {
        let vars = MoveVars::momentum_surf();
        let mut v = Vec3::new(250.0, 0.0, 0.0);
        // Enough grounded ticks to nearly stop.
        for _ in 0..200 {
            friction(&mut v, true, 1.0, &vars);
        }
        assert!(v.length() < 1.0, "speed left {}", v.length());
    }
}
