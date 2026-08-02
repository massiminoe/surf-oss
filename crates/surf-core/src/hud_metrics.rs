//! Feel-check metrics (not gameplay state). Used by the M0 HUD.

use crate::math::Vec3;

/// Air-strafe sync vs ideal perpendicular gain (Δv² = 900/tick at the 30-cap).
///
/// Returns 0..100. Meaningful on flat air strafes; on ramps ClipVelocity makes
/// it a rough "am I adding speed" read, not a true sync%.
pub fn air_strafe_sync(prev_velocity: Vec3, new_velocity: Vec3, wishing: bool, airborne: bool) -> f32 {
    if !airborne || !wishing {
        return 0.0;
    }
    let prev_h = prev_velocity.length_2d();
    let new_h = new_velocity.length_2d();
    let gain = new_h - prev_h;
    let ideal = (prev_h * prev_h + 900.0).sqrt() - prev_h;
    if ideal < 0.01 {
        return 0.0;
    }
    (gain / ideal * 100.0).clamp(0.0, 100.0)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::movement::{air_accelerate, MoveVars};

    #[test]
    fn perfect_perp_strafe_is_near_100() {
        let vars = MoveVars::momentum_surf();
        let mut v = Vec3::new(500.0, 0.0, 0.0);
        let prev = v;
        air_accelerate(
            &mut v,
            Vec3::new(0.0, 1.0, 0.0),
            260.0,
            vars.airaccelerate,
            1.0,
            vars.tick_interval,
            vars.air_wishspeed_cap,
        );
        let sync = air_strafe_sync(prev, v, true, true);
        assert!(sync > 99.0, "sync={sync}");
    }
}
