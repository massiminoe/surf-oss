//! Golden-master: fixed input stream → exact f32 positions.

use surf_core::brush::{Brush, World};
use surf_core::math::Vec3;
use surf_core::movement::{MoveVars, PlayerState, UserCmd};
use surf_core::tick;

fn floor() -> World {
    World::new(vec![Brush::aabb(
        Vec3::new(-2048.0, -2048.0, -128.0),
        Vec3::new(2048.0, 2048.0, 0.0),
    )])
}

#[test]
fn standing_jump_positions_are_deterministic() {
    let world = floor();
    let vars = MoveVars::momentum_surf();
    let mut p = PlayerState {
        origin: Vec3::new(0.0, 0.0, 0.0),
        grounded: true,
        ..PlayerState::default()
    };
    // Settle onto floor.
    for _ in 0..20 {
        p = tick(&world, &p, &UserCmd::default(), &vars);
    }
    assert!(p.grounded);

    let mut origins = Vec::new();
    for i in 0..40 {
        let cmd = UserCmd {
            jump: i == 0 || i > 0, // hold jump (autobhop); only first grounded tick jumps
            ..UserCmd::default()
        };
        p = tick(&world, &p, &cmd, &vars);
        origins.push((p.origin.x, p.origin.y, p.origin.z));
    }

    // Re-run and require bit-identical positions.
    let mut p2 = PlayerState {
        origin: Vec3::new(0.0, 0.0, 0.0),
        grounded: true,
        ..PlayerState::default()
    };
    for _ in 0..20 {
        p2 = tick(&world, &p2, &UserCmd::default(), &vars);
    }
    for i in 0..40 {
        let cmd = UserCmd {
            jump: true,
            ..UserCmd::default()
        };
        p2 = tick(&world, &p2, &cmd, &vars);
        let (x, y, z) = origins[i];
        assert_eq!(p2.origin.x.to_bits(), x.to_bits());
        assert_eq!(p2.origin.y.to_bits(), y.to_bits());
        assert_eq!(p2.origin.z.to_bits(), z.to_bits());
    }
}
