fn main() {
    let t = std::time::Instant::now();
    let map = surf_map::LoadedMap::load_path("assets/maps/surf_summit.bsp").expect("load");
    println!(
        "ok in {:.2?} brushes={} tris={} render_tris={} teleports={} mats={} lm={}x{} ({} faces) sky={:?} spawn=({:.1},{:.1},{:.1})",
        t.elapsed(),
        map.world.brushes.len(),
        map.world.tris.len(),
        map.mesh.tris.len(),
        map.teleports.len(),
        map.materials.textured_count,
        map.lightmaps.width,
        map.lightmaps.height,
        map.lightmaps.face_count,
        map.skyname,
        map.spawn_origin.x, map.spawn_origin.y, map.spawn_origin.z,
    );
    // Drop onto stage start floor for a few ticks.
    use surf_core::movement::{MoveVars, PlayerState, UserCmd};
    use surf_core::tick;
    let vars = MoveVars::momentum_surf();
    let mut p = PlayerState {
        origin: map.spawn_origin,
        viewangles: map.spawn_angles,
        grounded: false,
        ..PlayerState::default()
    };
    let t2 = std::time::Instant::now();
    for _ in 0..200 {
        p = tick(&map.world, &p, &UserCmd::default(), &vars);
    }
    println!(
        "200 ticks in {:.2?}  grounded={} z={:.1} velz={:.1}",
        t2.elapsed(), p.grounded, p.origin.z, p.velocity.z
    );
}
