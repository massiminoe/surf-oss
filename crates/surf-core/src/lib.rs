//! Pure f32 Source-style movement physics. Zero I/O, zero unsafe.
//!
//! `tick` is the determinism boundary: same `(world, state, cmd, vars)` → same
//! next state, bit-for-bit.

pub mod brush;
pub mod graybox;
pub mod hud_metrics;
pub mod math;
pub mod mesh_collide;
pub mod movement;
pub mod tick;
pub mod trace;

pub use brush::{Aabb, Brush, Plane, World};
pub use hud_metrics::air_strafe_sync;
pub use math::{Angle, Vec3};
pub use mesh_collide::CollisionTri;
pub use movement::{apply_base_velocity_momentum, BugFixes, Hull, MoveVars, PlayerState, UserCmd};
pub use tick::{is_on_surf_ramp, tick};
pub use trace::{TraceHit, TraceResult};
