//! Per-tick movement pipeline (research 01 §3). Pure function.

use crate::brush::World;
use crate::math::Vec3;
use crate::movement::{
    air_accelerate, check_velocity, clamp_input_speed, clip_velocity, friction, wish_move, Hull,
    MoveVars, PlayerState, UserCmd, CROUCH_STUCK_NUDGES, DUCK_SPEED_MULTIPLIER, GROUND_PROBE,
    MAX_CLIP_PLANES, NON_JUMP_VELOCITY, STOP_EPSILON, WALKABLE_NORMAL_Z,
};
use crate::trace::{point_contents_box, trace_box, TraceResult};

/// True when the player hull is sitting on a surfable slope (steep, not walkable).
///
/// Replay `grounded` is almost always false on ramps (and KSF flags omit it), so
/// trail coloring probes the world instead. We sweep the full hull from above
/// (avoids startsolid in the wedge) and accept hits whose expanded plane is
/// close to the feet origin.
pub fn is_on_surf_ramp(world: &World, origin: Vec3, hull: &Hull) -> bool {
    let rampish = |nz: f32| nz > 0.05 && nz < WALKABLE_NORMAL_Z;
    let near_plane = |hit: &crate::trace::TraceHit, origin: Vec3| {
        let d = hit.normal.dot(origin) - hit.dist;
        // Outside or slightly penetrating the expanded hull plane.
        d < 10.0 && d > -20.0
    };

    let start = origin + Vec3::new(0.0, 0.0, 24.0);
    let end = origin - Vec3::new(0.0, 0.0, 10.0);
    let tr = trace_box(world, start, end, hull.mins, hull.maxs);
    if let Some(hit) = tr.hit {
        if rampish(hit.normal.z) && near_plane(&hit, origin) {
            return true;
        }
    }

    // Lateral probes: surfing contact is often into the face, not straight down.
    const LATERAL: f32 = 14.0;
    let lateral = [
        Vec3::new(0.0, LATERAL, -2.0),
        Vec3::new(0.0, -LATERAL, -2.0),
        Vec3::new(LATERAL, 0.0, -2.0),
        Vec3::new(-LATERAL, 0.0, -2.0),
    ];
    let lift = origin + Vec3::new(0.0, 0.0, 8.0);
    for delta in lateral {
        let tr = trace_box(world, lift, lift + delta, hull.mins, hull.maxs);
        if let Some(hit) = tr.hit {
            if rampish(hit.normal.z) && near_plane(&hit, origin) {
                return true;
            }
        }
    }
    false
}

/// CS:S stand vs duck hull height (feet→top). Air duck raises origin by this.
#[inline]
fn duck_view_delta() -> f32 {
    let stand = Hull::css_stand();
    let duck = Hull::css_duck();
    (stand.maxs.z - stand.mins.z) - (duck.maxs.z - duck.mins.z)
}

/// Determinism boundary: one Source-style movement tick.
pub fn tick(world: &World, state: &PlayerState, cmd: &UserCmd, vars: &MoveVars) -> PlayerState {
    let mut p = state.clone();
    p.viewangles = cmd.viewangles;

    // PlayerMove order: Duck() before FullWalkMove (research 01 §3 / §6).
    // Instant hull change (no view-spline timers yet); air offset is the crouch-jump.
    apply_duck(world, &mut p, cmd.duck);

    let hull = p.hull();
    // CheckParameters: scale wish move, clamp to maxspeed.
    let mut fmove = cmd.forward_move * vars.forward_speed;
    let mut smove = cmd.side_move * vars.side_speed;
    if p.ducked && p.grounded {
        fmove *= DUCK_SPEED_MULTIPLIER;
        smove *= DUCK_SPEED_MULTIPLIER;
    }
    let (fmove, smove) = clamp_input_speed(fmove, smove, vars.maxspeed);

    // FullWalkMove (dry land only).
    start_gravity(&mut p, vars);

    let jumped = if cmd.jump {
        check_jump_button(&mut p, cmd, vars)
    } else {
        p.old_jump = false;
        false
    };
    let _ = jumped;

    if p.grounded {
        p.velocity.z = 0.0;
        friction(&mut p.velocity, true, p.surface_friction, vars);
    }

    check_velocity(&mut p.velocity, &mut p.origin, vars.maxvelocity);

    if p.grounded {
        walk_move(world, &mut p, fmove, smove, vars, &hull);
    } else {
        air_move(world, &mut p, fmove, smove, vars, &hull);
    }

    categorize_position(world, &mut p, vars, &hull);
    check_velocity(&mut p.velocity, &mut p.origin, vars.maxvelocity);

    finish_gravity(&mut p, vars);

    if p.grounded {
        p.velocity.z = 0.0;
    }

    p.old_jump = cmd.jump;
    p
}

fn start_gravity(p: &mut PlayerState, vars: &MoveVars) {
    p.velocity.z -= vars.gravity * 0.5 * vars.tick_interval;
    check_velocity(&mut p.velocity, &mut p.origin, vars.maxvelocity);
}

fn finish_gravity(p: &mut PlayerState, vars: &MoveVars) {
    p.velocity.z -= vars.gravity * 0.5 * vars.tick_interval;
    check_velocity(&mut p.velocity, &mut p.origin, vars.maxvelocity);
}

fn check_jump_button(p: &mut PlayerState, cmd: &UserCmd, vars: &MoveVars) -> bool {
    if !p.grounded {
        p.old_jump = true;
        return false;
    }
    if !vars.autobhop && p.old_jump {
        return false;
    }

    if vars.bhop_clamp {
        let max_scaled = 1.1 * vars.maxspeed;
        let spd = p.velocity.length();
        if max_scaled > 0.0 && spd > max_scaled {
            p.velocity *= max_scaled / spd;
        }
    }

    // Leave ground before friction can apply.
    p.grounded = false;
    p.ground_normal = Vec3::Z;

    // Standing: add; ducked: set (CS). We force add for consistency with Momentum CS modes.
    p.velocity.z += vars.jump_impulse;

    finish_gravity(p, vars);
    p.old_jump = true;
    let _ = cmd;
    true
}

fn air_move(
    world: &World,
    p: &mut PlayerState,
    fmove: f32,
    smove: f32,
    vars: &MoveVars,
    hull: &Hull,
) {
    let (wishdir, wishspeed) = wish_move(p.viewangles, fmove, smove, vars.maxspeed);
    air_accelerate(
        &mut p.velocity,
        wishdir,
        wishspeed,
        vars.airaccelerate,
        p.surface_friction,
        vars.tick_interval,
        vars.air_wishspeed_cap,
    );
    try_player_move(world, p, vars, hull);
}

fn walk_move(
    world: &World,
    p: &mut PlayerState,
    fmove: f32,
    smove: f32,
    vars: &MoveVars,
    hull: &Hull,
) {
    let (wishdir, wishspeed) = wish_move(p.viewangles, fmove, smove, vars.maxspeed);
    p.velocity.z = 0.0;
    crate::movement::accelerate(
        &mut p.velocity,
        wishdir,
        wishspeed,
        vars.accelerate,
        p.surface_friction,
        vars.tick_interval,
    );
    p.velocity.z = 0.0;

    if p.velocity.length() < 1.0 {
        p.velocity = Vec3::ZERO;
        stay_on_ground(world, p, vars, hull);
        return;
    }

    let dest = Vec3::new(
        p.origin.x + p.velocity.x * vars.tick_interval,
        p.origin.y + p.velocity.y * vars.tick_interval,
        p.origin.z,
    );
    let tr = trace_player(world, p.origin, dest, hull);
    if tr.fraction == 1.0 {
        p.origin = tr.endpos;
        stay_on_ground(world, p, vars, hull);
        return;
    }

    // Step up over obstacles.
    step_move(world, p, dest, vars, hull);
    stay_on_ground(world, p, vars, hull);
}

fn stay_on_ground(world: &World, p: &mut PlayerState, vars: &MoveVars, hull: &Hull) {
    let up = p.origin + Vec3::new(0.0, 0.0, 2.0);
    let tr_up = trace_player(world, p.origin, up, hull);
    let start = tr_up.endpos;
    let down = start - Vec3::new(0.0, 0.0, vars.stepsize);
    let tr = trace_player(world, start, down, hull);
    if tr.fraction > 0.0
        && tr.fraction < 1.0
        && !tr.startsolid
        && tr.hit.map(|h| h.normal.z >= WALKABLE_NORMAL_Z).unwrap_or(false)
    {
        if (p.origin.z - tr.endpos.z).abs() > 0.5 * (1.0 / 32.0) {
            p.origin = tr.endpos;
        }
    }
}

fn step_move(world: &World, p: &mut PlayerState, _dest: Vec3, vars: &MoveVars, hull: &Hull) {
    let original_origin = p.origin;
    let original_velocity = p.velocity;

    // Attempt A: slide
    try_player_move(world, p, vars, hull);
    let down_origin = p.origin;
    let down_velocity = p.velocity;
    let down_dist = (down_origin.xy() - original_origin.xy()).length_2d();

    // Attempt B: step up then slide then step down
    p.origin = original_origin;
    p.velocity = original_velocity;

    let step_end = p.origin + Vec3::new(0.0, 0.0, vars.stepsize + crate::trace::DIST_EPSILON);
    let tr_up = trace_player(world, p.origin, step_end, hull);
    p.origin = tr_up.endpos;
    try_player_move(world, p, vars, hull);

    let down_end = p.origin - Vec3::new(0.0, 0.0, vars.stepsize + crate::trace::DIST_EPSILON);
    let tr_down = trace_player(world, p.origin, down_end, hull);
    if tr_down.hit.map(|h| h.normal.z < WALKABLE_NORMAL_Z).unwrap_or(true) {
        // Landing not walkable — prefer A.
        p.origin = down_origin;
        p.velocity = down_velocity;
        return;
    }
    p.origin = tr_down.endpos;

    let up_dist = (p.origin.xy() - original_origin.xy()).length_2d();
    if down_dist > up_dist {
        p.origin = down_origin;
        p.velocity = down_velocity;
    } else {
        // Keep B's position; restore A's vertical velocity (Source quirk).
        p.velocity.z = down_velocity.z;
    }
}

fn try_player_move(world: &World, p: &mut PlayerState, vars: &MoveVars, hull: &Hull) {
    let numbumps = if vars.fixes.fix_ramp { 8 } else { 4 };
    let mut original_velocity = p.velocity;
    let primal_velocity = p.velocity;
    let mut time_left = vars.tick_interval;
    let mut all_fraction = 0.0;
    let mut planes: [Vec3; MAX_CLIP_PLANES] = [Vec3::ZERO; MAX_CLIP_PLANES];
    let mut numplanes = 0usize;

    for _bump in 0..numbumps {
        if p.velocity.length() == 0.0 {
            break;
        }
        let end = p.origin + p.velocity * time_left;
        let pm = trace_player(world, p.origin, end, hull);
        all_fraction += pm.fraction;

        if pm.allsolid {
            if vars.fixes.fix_ramp && !p.grounded {
                // Nudge along first plane normal instead of hard zero.
                p.origin += Vec3::Z * 0.2;
            } else {
                p.velocity = Vec3::ZERO;
            }
            return;
        }

        if pm.fraction > 0.0 {
            if pm.fraction == 1.0 {
                // Unswept re-test at endpos (terrain precision hack).
                let stuck = trace_player(world, pm.endpos, pm.endpos, hull);
                if stuck.startsolid || stuck.fraction != 1.0 {
                    if vars.fixes.fix_ramp && !p.grounded {
                        // Skip zeroing — stay put this bump.
                        break;
                    }
                    p.velocity = Vec3::ZERO;
                    break;
                }
            }
            p.origin = pm.endpos;
            original_velocity = p.velocity;
            numplanes = 0;
        }

        if pm.fraction == 1.0 {
            break;
        }

        let normal = match pm.hit {
            Some(h) => h.normal,
            None => break,
        };

        time_left -= time_left * pm.fraction;

        if numplanes >= MAX_CLIP_PLANES {
            if !(vars.fixes.fix_ramp && !p.grounded) {
                p.velocity = Vec3::ZERO;
            }
            break;
        }
        planes[numplanes] = normal;
        numplanes += 1;

        if numplanes == 1 && !p.grounded {
            let overbounce = if planes[0].z > WALKABLE_NORMAL_Z {
                1.0
            } else {
                1.0 + vars.bounce * (1.0 - p.surface_friction)
            };
            let new_velocity = clip_velocity(original_velocity, planes[0], overbounce);
            p.velocity = new_velocity;
            original_velocity = new_velocity;
        } else {
            let mut i = 0;
            while i < numplanes {
                p.velocity = clip_velocity(original_velocity, planes[i], 1.0);
                let mut ok = true;
                for (j, plane) in planes.iter().enumerate().take(numplanes) {
                    if j == i {
                        continue;
                    }
                    if p.velocity.dot(*plane) < 0.0 {
                        ok = false;
                        break;
                    }
                }
                if ok {
                    break;
                }
                i += 1;
            }
            if i == numplanes {
                if numplanes != 2 {
                    if !(vars.fixes.fix_ramp && !p.grounded) {
                        p.velocity = Vec3::ZERO;
                    }
                    break;
                }
                let mut dir = planes[0].cross(planes[1]);
                dir = dir.normalize();
                let d = dir.dot(p.velocity);
                p.velocity = dir * d;
            }
            if p.velocity.dot(primal_velocity) <= 0.0 {
                if !(vars.fixes.fix_ramp && !p.grounded) {
                    p.velocity = Vec3::ZERO;
                }
                break;
            }
        }
    }

    if all_fraction == 0.0 {
        if !(vars.fixes.fix_ramp && !p.grounded) {
            p.velocity = Vec3::ZERO;
        }
    }

    // Eat near-zero components.
    if p.velocity.x.abs() < STOP_EPSILON {
        p.velocity.x = 0.0;
    }
    if p.velocity.y.abs() < STOP_EPSILON {
        p.velocity.y = 0.0;
    }
    if p.velocity.z.abs() < STOP_EPSILON {
        p.velocity.z = 0.0;
    }
}

fn categorize_position(world: &World, p: &mut PlayerState, vars: &MoveVars, hull: &Hull) {
    p.surface_friction = 1.0;

    let moving_up_rapidly = p.velocity.z > NON_JUMP_VELOCITY;
    if moving_up_rapidly {
        set_ground(p, false, Vec3::Z, None, vars);
        if p.velocity.z > 0.0 && !vars.fixes.fix_deadstrafe {
            p.surface_friction = 0.25;
        }
        return;
    }

    let point = p.origin - Vec3::new(0.0, 0.0, GROUND_PROBE);
    let pm = trace_player(world, p.origin, point, hull);

    let walkable = pm
        .hit
        .map(|h| h.normal.z >= WALKABLE_NORMAL_Z)
        .unwrap_or(false)
        && !pm.startsolid
        && pm.fraction < 1.0;

    if !walkable {
        set_ground(p, false, Vec3::Z, None, vars);
        if p.velocity.z > 0.0 && !vars.fixes.fix_deadstrafe {
            p.surface_friction = 0.25;
        }
        return;
    }

    let normal = pm.hit.unwrap().normal;
    set_ground(p, true, normal, Some(pm), vars);
}

fn set_ground(
    p: &mut PlayerState,
    grounded: bool,
    normal: Vec3,
    land_trace: Option<TraceResult>,
    vars: &MoveVars,
) {
    let was_grounded = p.grounded;
    if grounded {
        if !was_grounded {
            // Landing.
            if vars.fixes.fix_slope {
                if let Some(ref tr) = land_trace {
                    if let Some(hit) = tr.hit {
                        let clipped = clip_velocity(p.velocity, hit.normal, 1.0);
                        let old_h = p.velocity.length_2d_squared();
                        let new_h = clipped.length_2d_squared();
                        if new_h > old_h {
                            p.velocity.x = clipped.x;
                            p.velocity.y = clipped.y;
                        }
                    }
                }
            }
            p.velocity.z = 0.0;
        }
        p.grounded = true;
        p.ground_normal = normal;
    } else {
        p.grounded = false;
        p.ground_normal = Vec3::Z;
    }
}

#[inline]
fn trace_player(world: &World, start: Vec3, end: Vec3, hull: &Hull) -> TraceResult {
    trace_box(world, start, end, hull.mins, hull.maxs)
}

/// Hold-to-duck with Source latch: releasing under a low ceiling keeps you ducked
/// until `can_unduck` succeeds.
fn apply_duck(world: &World, p: &mut PlayerState, want_duck: bool) {
    if want_duck {
        if !p.ducked {
            finish_duck(world, p);
        }
    } else if p.ducked && can_unduck(world, p) {
        finish_unduck(p);
    }
}

fn unduck_origin(p: &PlayerState) -> Vec3 {
    if p.grounded {
        // Hull mins.z are both 0 — feet stay planted.
        p.origin
    } else {
        // Mirror air FinishDuck: drop feet so the head stays put.
        p.origin - Vec3::new(0.0, 0.0, duck_view_delta())
    }
}

fn can_unduck(world: &World, p: &PlayerState) -> bool {
    let stand = Hull::css_stand();
    let new_origin = unduck_origin(p);
    let tr = trace_box(world, p.origin, new_origin, stand.mins, stand.maxs);
    !tr.startsolid && tr.fraction >= 1.0
}

fn finish_duck(world: &World, p: &mut PlayerState) {
    if !p.grounded {
        p.origin += Vec3::new(0.0, 0.0, duck_view_delta());
    }
    p.ducked = true;
    fix_crouch_stuck(world, p);
}

fn finish_unduck(p: &mut PlayerState) {
    p.origin = unduck_origin(p);
    p.ducked = false;
}

/// If the ducked hull is startsolid, walk origin up 1u at a time (SDK crouch unstick).
fn fix_crouch_stuck(world: &World, p: &mut PlayerState) {
    let hull = Hull::css_duck();
    if !point_contents_box(world, p.origin, hull.mins, hull.maxs) {
        return;
    }
    let start = p.origin;
    for _ in 0..CROUCH_STUCK_NUDGES {
        p.origin.z += 1.0;
        if !point_contents_box(world, p.origin, hull.mins, hull.maxs) {
            return;
        }
    }
    p.origin = start;
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::brush::Brush;
    use crate::graybox;
    use crate::math::Angle;
    use crate::movement::MoveVars;

    fn flat_world() -> World {
        World::new(vec![Brush::aabb(
            Vec3::new(-2048.0, -2048.0, -128.0),
            Vec3::new(2048.0, 2048.0, 0.0),
        )])
    }

    fn stand_on_floor(world: &World, vars: &MoveVars) -> PlayerState {
        let mut p = PlayerState {
            origin: Vec3::new(0.0, 0.0, 16.0),
            ..PlayerState::default()
        };
        // Settle.
        for _ in 0..30 {
            let cmd = UserCmd::default();
            p = tick(world, &p, &cmd, vars);
        }
        assert!(p.grounded, "should be grounded after settle");
        p
    }

    #[test]
    fn jump_apex_near_57() {
        let world = flat_world();
        let vars = MoveVars::momentum_surf();
        let mut p = stand_on_floor(&world, &vars);
        p.velocity = Vec3::ZERO;

        let mut cmd = UserCmd {
            jump: true,
            ..UserCmd::default()
        };
        p = tick(&world, &p, &cmd, &vars);
        cmd.jump = true; // autobhop held; airborne so no re-jump
        let mut max_z = p.origin.z;
        for _ in 0..200 {
            p = tick(&world, &p, &cmd, &vars);
            max_z = max_z.max(p.origin.z);
            if p.grounded && p.velocity.z == 0.0 && max_z > 10.0 {
                break;
            }
        }
        // Continuous √(2g·57) impulse → 57u apex; discrete half-gravity + jump-tick
        // FinishGravity yields a few units less (Source-faithful). VDC jump-on ≈ 52.
        assert!(
            max_z > 52.0 && max_z < 58.0,
            "jump apex {max_z} (expected ~54–57 discrete, 57 continuous)"
        );
        // Impulse itself must match the CS formula exactly.
        assert!((vars.jump_impulse * vars.jump_impulse / (2.0 * vars.gravity) - 57.0).abs() < 1e-3);
    }

    #[test]
    fn perpendicular_strafe_in_tick() {
        let world = World::empty();
        let vars = MoveVars::momentum_surf();
        let mut p = PlayerState {
            origin: Vec3::new(0.0, 0.0, 256.0),
            velocity: Vec3::new(500.0, 0.0, 0.0),
            grounded: false,
            viewangles: Angle::new(0.0, 0.0, 0.0), // facing +X; wish +Y via side
            ..PlayerState::default()
        };
        let before = p.velocity.length_2d_squared();
        // yaw 0 → forward +X, right is -Y (Source AngleVectors).
        let (f, r, _) = Angle::new(0.0, 0.0, 0.0).vectors();
        assert!(f.x > 0.9, "forward should be +X, got {f:?}");
        let cmd = UserCmd {
            viewangles: Angle::new(0.0, 0.0, 0.0),
            // Perpendicular wish: if right.y < 0, side_move -1 → +Y.
            side_move: if r.y < 0.0 { -1.0 } else { 1.0 },
            ..UserCmd::default()
        };
        p = tick(&world, &p, &cmd, &vars);
        let after = p.velocity.length_2d_squared();
        // Gravity affects z only; horizontal Δv² should be ~900.
        assert!(
            (after - before - 900.0).abs() < 1.0,
            "Δv² = {} (want 900)",
            after - before
        );
    }

    #[test]
    fn player_on_ramp_stays_airborne() {
        let gb = graybox::surf_ramp_arena();
        let vars = MoveVars::momentum_surf();
        // Place feet just above the +Y ramp face and let physics settle against it.
        let y = 100.0;
        let z_face = y * 3.0_f32.sqrt();
        let mut p = PlayerState {
            origin: Vec3::new(512.0, y, z_face + 4.0),
            velocity: Vec3::new(400.0, -50.0, 0.0), // into the ramp
            grounded: false,
            viewangles: Angle::new(0.0, 0.0, 0.0),
            ..PlayerState::default()
        };
        // Facing +X, +Y ramp: wish into ramp (+Y) → side_move -1 (Source right = -Y).
        let cmd = UserCmd {
            viewangles: Angle::new(0.0, 0.0, 0.0),
            side_move: -1.0,
            ..UserCmd::default()
        };
        for _ in 0..30 {
            p = tick(&gb.world, &p, &cmd, &vars);
        }
        assert!(
            !p.grounded,
            "must stay airborne on 60° ramp; origin={:?} vz={}",
            p.origin,
            p.velocity.z
        );
        assert!(p.origin.z > 10.0, "should still be on the ramp face");
    }

    #[test]
    fn air_duck_raises_feet_by_hull_delta() {
        let world = World::empty();
        let vars = MoveVars::momentum_surf();
        let start = PlayerState {
            origin: Vec3::new(0.0, 0.0, 256.0),
            velocity: Vec3::ZERO,
            grounded: false,
            ..PlayerState::default()
        };
        let ducked = tick(
            &world,
            &start,
            &UserCmd {
                duck: true,
                ..UserCmd::default()
            },
            &vars,
        );
        let standing = tick(&world, &start, &UserCmd::default(), &vars);
        assert!(ducked.ducked);
        let delta = duck_view_delta();
        assert!((delta - 17.0).abs() < 1e-3, "CS:S delta should be 17, got {delta}");
        // Same gravity/move; duck alone accounts for +delta on origin.z.
        assert!(
            (ducked.origin.z - standing.origin.z - delta).abs() < 1e-3,
            "air duck should raise feet by {delta}; ducked={} stand={}",
            ducked.origin.z,
            standing.origin.z
        );
    }

    #[test]
    fn ground_duck_keeps_feet_and_crops_wishspeed() {
        let world = flat_world();
        let vars = MoveVars::momentum_surf();
        let settled = stand_on_floor(&world, &vars);
        let z_before = settled.origin.z;
        let fwd = UserCmd {
            forward_move: 1.0,
            ..UserCmd::default()
        };
        let standing = tick(&world, &settled, &fwd, &vars);
        let ducked = tick(
            &world,
            &settled,
            &UserCmd {
                duck: true,
                forward_move: 1.0,
                ..UserCmd::default()
            },
            &vars,
        );
        assert!(ducked.ducked);
        assert!(
            (ducked.origin.z - z_before).abs() < 0.5,
            "ground duck keeps feet planted"
        );
        // Crop is ×0.34 on cl_forwardspeed (450→153) before maxspeed clamp (260).
        let ratio = ducked.velocity.length_2d() / standing.velocity.length_2d();
        assert!(
            (ratio - 153.0 / 260.0).abs() < 0.05,
            "ducked/standing first-tick speed ratio {ratio} (want ~153/260)"
        );
    }

    #[test]
    fn low_ceiling_latches_duck_after_release() {
        // Floor at z=0, ceiling at 56 — duck hull (45) fits, stand (62) does not.
        let world = World::new(vec![
            Brush::aabb(
                Vec3::new(-2048.0, -2048.0, -128.0),
                Vec3::new(2048.0, 2048.0, 0.0),
            ),
            Brush::aabb(
                Vec3::new(-2048.0, -2048.0, 56.0),
                Vec3::new(2048.0, 2048.0, 96.0),
            ),
        ]);
        let vars = MoveVars::momentum_surf();
        let duck_cmd = UserCmd {
            duck: true,
            ..UserCmd::default()
        };
        // Must start ducked — standing hull can't settle under this ceiling.
        let mut p = PlayerState {
            origin: Vec3::new(0.0, 0.0, 2.0),
            ducked: true,
            ..PlayerState::default()
        };
        for _ in 0..30 {
            p = tick(&world, &p, &duck_cmd, &vars);
        }
        assert!(
            p.grounded && p.ducked,
            "settle ducked under ceiling; grounded={} origin={:?}",
            p.grounded,
            p.origin
        );
        // Release duck — standing hull doesn't fit → stay latched.
        p = tick(&world, &p, &UserCmd::default(), &vars);
        assert!(p.ducked, "must stay ducked under low ceiling after release");
    }

    #[test]
    fn air_unduck_restores_origin_when_clear() {
        let world = World::empty();
        let vars = MoveVars::momentum_surf();
        let start = PlayerState {
            origin: Vec3::new(0.0, 0.0, 400.0),
            velocity: Vec3::ZERO,
            grounded: false,
            ..PlayerState::default()
        };
        let after_duck = tick(
            &world,
            &start,
            &UserCmd {
                duck: true,
                ..UserCmd::default()
            },
            &vars,
        );
        assert!(after_duck.ducked);
        let unducked = tick(&world, &after_duck, &UserCmd::default(), &vars);
        let stayed_up = tick(
            &world,
            &after_duck,
            &UserCmd {
                duck: true,
                ..UserCmd::default()
            },
            &vars,
        );
        assert!(!unducked.ducked);
        let delta = duck_view_delta();
        assert!(
            (stayed_up.origin.z - unducked.origin.z - delta).abs() < 1e-3,
            "releasing duck in clear air should drop feet by {delta}"
        );
    }
}
