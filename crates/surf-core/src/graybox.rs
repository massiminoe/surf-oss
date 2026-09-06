//! Hand-built graybox worlds for M0 feel-check (no BSP).

use crate::brush::{Aabb, Brush, Plane, World};
use crate::math::Vec3;

/// Flat-shaded / textured triangle for the renderer.
#[derive(Clone, Copy, Debug)]
pub struct Tri {
    pub a: Vec3,
    pub b: Vec3,
    pub c: Vec3,
    /// RGB 0..1 fallback / tint when `tex == 0`.
    pub color: [f32; 3],
    pub uv_a: [f32; 2],
    pub uv_b: [f32; 2],
    pub uv_c: [f32; 2],
    /// Lightmap atlas UVs (0..1). White stub when unset.
    pub lm_a: [f32; 2],
    pub lm_b: [f32; 2],
    pub lm_c: [f32; 2],
    /// Texture-array layer. `0` = solid `color` (no albedo).
    pub tex: u32,
    /// Second texture-array layer for two-texture displacement blends
    /// (`WorldVertexTransition`); `0` = none. Mixed by `alpha` per vertex.
    pub tex2: u32,
    /// Per-vertex blend weight toward `tex2` (0..1).
    pub alpha: [f32; 3],
    /// Per-vertex linear light multiplier, applied on top of the lightmap
    /// sample. World faces carry 1.0 (the lightmap is their light); static
    /// props carry their baked per-vertex light (their lightmap texel is the
    /// reserved "L = 1" texel).
    pub light: [[f32; 3]; 3],
}

impl Tri {
    pub fn flat(a: Vec3, b: Vec3, c: Vec3, color: [f32; 3]) -> Self {
        Self {
            a,
            b,
            c,
            color,
            uv_a: [0.0, 0.0],
            uv_b: [0.0, 0.0],
            uv_c: [0.0, 0.0],
            lm_a: [0.0, 0.0],
            lm_b: [0.0, 0.0],
            lm_c: [0.0, 0.0],
            tex: 0,
            tex2: 0,
            alpha: [0.0; 3],
            light: [[1.0; 3]; 3],
        }
    }
}

#[derive(Clone, Debug)]
pub struct GrayboxMesh {
    pub tris: Vec<Tri>,
}

#[derive(Clone, Debug)]
pub struct GrayboxWorld {
    pub world: World,
    pub mesh: GrayboxMesh,
    pub spawn_origin: Vec3,
    pub spawn_yaw: f32,
}

/// Mini surf course for feel-check:
/// high drop-in → long V-channel → landing → one-sided side-ramp → end pad.
pub fn surf_ramp_arena() -> GrayboxWorld {
    let mut brushes = Vec::new();
    let mut tris = Vec::new();

    // --- Stage 1: long V (60°) ------------------------------------------------
    let s1_x0 = 0.0;
    let s1_x1 = 2800.0;
    let s1_half = 320.0;
    let s1_h = s1_half * 3.0_f32.sqrt(); // ~554
    let s1_z0 = 0.0;

    // Kill floor under the valley (fail = soft reset by falling a long way;
    // a real tele comes in M2 — for now a deep pit + side catch pads).
    let pit = Brush::aabb(
        Vec3::new(-512.0, -2048.0, -1024.0),
        Vec3::new(5600.0, 2048.0, -16.0),
    );
    brushes.push(pit);
    push_aabb_tris(
        &mut tris,
        Vec3::new(-512.0, -2048.0, -1024.0),
        Vec3::new(5600.0, 2048.0, -16.0),
        [0.22, 0.22, 0.25],
    );

    // High spawn tower overlooking the +Y face of stage 1.
    let spawn_mins = Vec3::new(-384.0, 180.0, 420.0);
    let spawn_maxs = Vec3::new(-64.0, 340.0, 436.0);
    brushes.push(Brush::aabb(spawn_mins, spawn_maxs));
    push_aabb_tris(&mut tris, spawn_mins, spawn_maxs, [0.55, 0.58, 0.62]);

    // Spawn backstop so you don't fall off backward.
    let back_mins = Vec3::new(-400.0, 180.0, 420.0);
    let back_maxs = Vec3::new(-384.0, 340.0, 520.0);
    brushes.push(Brush::aabb(back_mins, back_maxs));
    push_aabb_tris(&mut tris, back_mins, back_maxs, [0.40, 0.42, 0.46]);

    // Stage 1 V faces (striped along X for speed read).
    push_v_ramp(
        &mut brushes,
        &mut tris,
        s1_x0,
        s1_x1,
        s1_z0,
        s1_half,
        s1_h,
        [0.45, 0.62, 0.90],
        [0.90, 0.55, 0.45],
        8,
    );

    // Tall side walls outside the V (visual guide + keep you from flying out sideways).
    let wall_h = s1_h + 128.0;
    let wall_t = 32.0;
    let w_r = (
        Vec3::new(s1_x0, s1_half, s1_z0),
        Vec3::new(s1_x1, s1_half + wall_t, s1_z0 + wall_h),
    );
    brushes.push(Brush::aabb(w_r.0, w_r.1));
    push_aabb_tris(&mut tris, w_r.0, w_r.1, [0.32, 0.34, 0.38]);
    let w_l = (
        Vec3::new(s1_x0, -s1_half - wall_t, s1_z0),
        Vec3::new(s1_x1, -s1_half, s1_z0 + wall_h),
    );
    brushes.push(Brush::aabb(w_l.0, w_l.1));
    push_aabb_tris(&mut tris, w_l.0, w_l.1, [0.32, 0.34, 0.38]);

    // --- Mid landing after stage 1 -------------------------------------------
    let mid_x0 = s1_x1;
    let mid_x1 = s1_x1 + 384.0;
    let mid_mins = Vec3::new(mid_x0, -192.0, 0.0);
    let mid_maxs = Vec3::new(mid_x1, 192.0, 24.0);
    brushes.push(Brush::aabb(mid_mins, mid_maxs));
    push_aabb_tris(&mut tris, mid_mins, mid_maxs, [0.50, 0.52, 0.48]);

    // --- Stage 2: one-sided ramp (board left, wall on +Y) ---------------------
    // Raised start so you jump/drop onto it from mid pad; runs along +X.
    let s2_x0 = mid_x1;
    let s2_x1 = mid_x1 + 2200.0;
    let s2_half = 280.0;
    let s2_h = s2_half * 3.0_f32.sqrt();
    let s2_z0 = 64.0; // slightly raised valley

    // Only the +Y face (one-wall style). Valley sits on a thin floor strip.
    let strip = (
        Vec3::new(s2_x0, -64.0, s2_z0 - 24.0),
        Vec3::new(s2_x1, 64.0, s2_z0),
    );
    brushes.push(Brush::aabb(strip.0, strip.1));
    push_aabb_tris(&mut tris, strip.0, strip.1, [0.38, 0.40, 0.36]);

    push_ramp_segmented(
        &mut brushes,
        &mut tris,
        s2_x0,
        s2_x1,
        s2_z0,
        0.0,
        s2_half,
        s2_h,
        [0.50, 0.78, 0.70],
        6,
    );

    // Outer wall on stage 2
    let w2 = (
        Vec3::new(s2_x0, s2_half, s2_z0),
        Vec3::new(s2_x1, s2_half + 32.0, s2_z0 + s2_h + 64.0),
    );
    brushes.push(Brush::aabb(w2.0, w2.1));
    push_aabb_tris(&mut tris, w2.0, w2.1, [0.30, 0.36, 0.34]);

    // --- Stage 3: short opposing V into end pad --------------------------------
    let s3_x0 = s2_x1 + 128.0;
    let s3_x1 = s3_x0 + 1200.0;
    let s3_half = 240.0;
    let s3_h = s3_half * 3.0_f32.sqrt();
    let s3_z0 = 0.0;

    // Small bridge pad between s2 and s3
    let bridge = (
        Vec3::new(s2_x1, -96.0, 40.0),
        Vec3::new(s3_x0, 96.0, 56.0),
    );
    brushes.push(Brush::aabb(bridge.0, bridge.1));
    push_aabb_tris(&mut tris, bridge.0, bridge.1, [0.55, 0.50, 0.40]);

    push_v_ramp(
        &mut brushes,
        &mut tris,
        s3_x0,
        s3_x1,
        s3_z0,
        s3_half,
        s3_h,
        [0.70, 0.55, 0.90],
        [0.90, 0.75, 0.40],
        4,
    );

    // End pad
    let end = (
        Vec3::new(s3_x1, -256.0, 0.0),
        Vec3::new(s3_x1 + 384.0, 256.0, 32.0),
    );
    brushes.push(Brush::aabb(end.0, end.1));
    push_aabb_tris(&mut tris, end.0, end.1, [0.35, 0.70, 0.45]);

    GrayboxWorld {
        world: World::new(brushes),
        mesh: GrayboxMesh { tris },
        // Standing on spawn, looking +X down the course; drop onto +Y face.
        spawn_origin: Vec3::new(-200.0, 260.0, 436.0),
        spawn_yaw: 0.0,
    }
}

fn push_v_ramp(
    brushes: &mut Vec<Brush>,
    tris: &mut Vec<Tri>,
    x0: f32,
    x1: f32,
    z0: f32,
    half: f32,
    h: f32,
    color_pos_y: [f32; 3],
    color_neg_y: [f32; 3],
    segments: u32,
) {
    push_ramp_segmented(brushes, tris, x0, x1, z0, 0.0, half, h, color_pos_y, segments);
    push_ramp_segmented(brushes, tris, x0, x1, z0, 0.0, -half, h, color_neg_y, segments);
}

fn push_ramp_segmented(
    brushes: &mut Vec<Brush>,
    tris: &mut Vec<Tri>,
    x0: f32,
    x1: f32,
    z_valley: f32,
    y_valley: f32,
    y_edge: f32,
    z_rise: f32,
    color: [f32; 3],
    segments: u32,
) {
    let segments = segments.max(1);
    let dx = (x1 - x0) / segments as f32;
    for i in 0..segments {
        let a = x0 + dx * i as f32;
        let b = a + dx;
        // Alternate stripe shade for speed reading.
        let shade = if i % 2 == 0 { 1.0 } else { 0.82 };
        let c = [color[0] * shade, color[1] * shade, color[2] * shade];
        let (brush, mesh) = ramp_brush(a, b, z_valley, y_valley, y_edge, z_rise, c);
        brushes.push(brush);
        tris.extend(mesh);
    }
}

/// Convex wedge under a ramp face from valley line to raised edge.
fn ramp_brush(
    x0: f32,
    x1: f32,
    z_valley: f32,
    y_valley: f32,
    y_edge: f32,
    z_rise: f32,
    color: [f32; 3],
) -> (Brush, Vec<Tri>) {
    let z_edge = z_valley + z_rise;
    let v00 = Vec3::new(x0, y_valley, z_valley);
    let v10 = Vec3::new(x1, y_valley, z_valley);
    let e0 = Vec3::new(x0, y_edge, z_edge);
    let e1 = Vec3::new(x1, y_edge, z_edge);

    let mut face_n = (v10 - v00).cross(e0 - v00).normalize();
    let toward_valley = Vec3::new(0.0, y_valley - y_edge, 0.0);
    if face_n.dot(toward_valley) < 0.0 {
        face_n = -face_n;
    }

    let y_min = y_valley.min(y_edge) - 1.0;
    let y_max = y_valley.max(y_edge) + 1.0;
    let mins = Vec3::new(x0 - 1.0, y_min, z_valley - 32.0);
    let maxs = Vec3::new(x1 + 1.0, y_max, z_edge + 1.0);

    let mut planes = Brush::aabb(mins, maxs).planes;
    planes.retain(|p| !(p.normal.z > 0.9));
    planes.push(Plane {
        normal: face_n,
        dist: face_n.dot(v00),
    });

    let valley_sign = if y_edge > y_valley { -1.0 } else { 1.0 };
    planes.retain(|p| {
        !(p.normal.y * valley_sign > 0.9 && p.normal.x.abs() < 0.1 && p.normal.z.abs() < 0.1)
    });

    let tris = vec![
        Tri::flat(v00, v10, e1, color),
        Tri::flat(v00, e1, e0, color),
    ];

    (
        Brush {
            planes,
            bounds: Aabb::from_mins_maxs(mins, maxs),
        },
        tris,
    )
}

fn push_aabb_tris(tris: &mut Vec<Tri>, mins: Vec3, maxs: Vec3, color: [f32; 3]) {
    let v = |x, y, z| Vec3::new(x, y, z);
    let c = [
        v(mins.x, mins.y, mins.z),
        v(maxs.x, mins.y, mins.z),
        v(maxs.x, maxs.y, mins.z),
        v(mins.x, maxs.y, mins.z),
        v(mins.x, mins.y, maxs.z),
        v(maxs.x, mins.y, maxs.z),
        v(maxs.x, maxs.y, maxs.z),
        v(mins.x, maxs.y, maxs.z),
    ];
    let faces: [[usize; 4]; 6] = [
        [0, 1, 2, 3],
        [4, 7, 6, 5],
        [0, 4, 5, 1],
        [3, 2, 6, 7],
        [0, 3, 7, 4],
        [1, 5, 6, 2],
    ];
    for f in faces {
        let (a, b, c2, d) = (c[f[0]], c[f[1]], c[f[2]], c[f[3]]);
        tris.push(Tri::flat(a, b, c2, color));
        tris.push(Tri::flat(a, c2, d, color));
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::movement::{Hull, WALKABLE_NORMAL_Z};
    use crate::trace::trace_box;

    #[test]
    fn ramp_face_normal_not_walkable() {
        let gb = surf_ramp_arena();
        let hull = Hull::css_stand();
        let y = 160.0;
        let z_face = y * 3.0_f32.sqrt();
        let start = Vec3::new(512.0, y, z_face + 80.0);
        let end = Vec3::new(512.0, y, z_face - 40.0);
        let tr = trace_box(&gb.world, start, end, hull.mins, hull.maxs);
        let hit = tr.hit.expect("should hit ramp");
        assert!(
            hit.normal.z < WALKABLE_NORMAL_Z,
            "ramp normal.z = {} should be < 0.7",
            hit.normal.z
        );
        assert!(
            (hit.normal.z - 0.5).abs() < 0.05,
            "expect ~0.5 for 60° ramp, got {}",
            hit.normal.z
        );
    }

    #[test]
    fn on_surf_ramp_detects_graybox_face() {
        use crate::tick::is_on_surf_ramp;
        let gb = surf_ramp_arena();
        let hull = Hull::css_stand();
        let y = 100.0;
        let z_face = y * 3.0_f32.sqrt();
        // Same placement family as `player_on_ramp_stays_airborne`.
        let on = Vec3::new(512.0, y, z_face + 4.0);
        assert!(
            is_on_surf_ramp(&gb.world, on, &hull),
            "expected ramp contact at {:?}",
            on
        );
        let air = Vec3::new(512.0, y, z_face + 120.0);
        assert!(!is_on_surf_ramp(&gb.world, air, &hull));
    }
}

