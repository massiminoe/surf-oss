//! End-to-end audio check against a real recorded run.
//!
//! Neither the physics tests nor the DSP unit tests can tell you whether the
//! audio layer actually tracks a *run* — whether the core is present while
//! surfing and absent while flying, whether ramp transitions produce events at
//! plausible rates. This drives the imported KSF summit replay through exactly
//! the pipeline the game uses and checks the output numerically.
//!
//! Skips silently when the map or replay assets are absent, matching the
//! convention in the rest of the suite.

use std::path::PathBuf;
use std::time::Instant;

use surf_app::replay::{GhostPlayback, Replay};
use surf_audio::{AudioEvent, EventDetector, Levels, MaglevVoice, Observation, Params};
use surf_core::movement::Hull;
use surf_core::{air_strafe_sync, is_on_surf_ramp};

const SR: f32 = 48_000.0;

struct Rendered {
    on_ramp_rms: f32,
    off_ramp_rms: f32,
    peak: f32,
    max_step: f32,
    catches: usize,
    releases: usize,
    lands: usize,
}

#[test]
fn summit_replay_drives_the_audio_layer_sensibly() {
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..");
    let map = root.join("assets/maps/surf_summit.bsp");
    let ghost = root.join(
        "assets/replays/external/ksf/surf_summit/imported/replay_css_2946_0_712551_1745965307.osxr",
    );
    if !map.is_file() || !ghost.is_file() {
        eprintln!("summit assets absent; skipping");
        return;
    }

    let loaded = surf_map::LoadedMap::load_path(&map).expect("summit");
    let replay = Replay::load(&ghost).expect("ksf ghost");
    let dt = replay.header.tick_interval;
    let mut g = GhostPlayback::from_replay(replay);

    // Time the probe: it runs every tick in the live loop, so it has to be cheap.
    let hull = Hull::css_stand();
    let t0 = Instant::now();
    g.classify_ramp_contact(&loaded.world);
    let probe_us = t0.elapsed().as_secs_f64() * 1e6 / g.frames.len().max(1) as f64;
    println!("is_on_surf_ramp: {probe_us:.2} us/call over {} frames", g.frames.len());
    assert!(
        probe_us < 100.0,
        "ramp probe too slow to run per tick: {probe_us:.2} us/call"
    );
    // Sanity: the classifier and the live probe must agree.
    let spot = is_on_surf_ramp(&loaded.world, g.frames[g.frames.len() / 2].origin, &hull);
    assert_eq!(spot, g.on_ramp[g.frames.len() / 2]);

    // Core in isolation: the sub is a high-amplitude 40 Hz sine that dominates
    // broadband RMS while being perceptually quiet, so measuring the gate in the
    // full mix would tell us nothing.
    let core_only = render_run(
        &g,
        dt,
        Levels {
            master: 1.0,
            core: 1.0,
            air: 0.0,
            sub: 0.0,
        },
    );
    let r = render_run(&g, dt, Levels::default());

    println!(
        "events: {} catch, {} release, {} land",
        r.catches, r.releases, r.lands
    );
    println!(
        "core-only rms: on-ramp {:.5} off-ramp {:.5} (ratio {:.1}x)",
        core_only.on_ramp_rms,
        core_only.off_ramp_rms,
        core_only.on_ramp_rms / core_only.off_ramp_rms.max(1e-9)
    );
    println!(
        "full mix: peak {:.4} max-step {:.4}",
        r.peak, r.max_step
    );

    // A 40-second summit run crosses plenty of ramps.
    assert!(r.catches >= 5, "expected several ramp catches, got {}", r.catches);
    assert!(
        r.releases >= 5,
        "expected several ramp releases, got {}",
        r.releases
    );
    // Catch/release must roughly pair up — a big imbalance means edge detection
    // is chattering on one side.
    let imbalance = (r.catches as i32 - r.releases as i32).unsigned_abs() as usize;
    assert!(
        imbalance <= 2,
        "catch/release imbalance {imbalance} ({} vs {})",
        r.catches,
        r.releases
    );

    // The whole premise, measured on a real run: the core is present while
    // surfing and essentially absent while flying.
    assert!(
        core_only.on_ramp_rms > core_only.off_ramp_rms * 3.0,
        "core not gated by ramp contact: on {:.5} vs off {:.5}",
        core_only.on_ramp_rms,
        core_only.off_ramp_rms
    );

    assert!(r.peak <= 0.98, "clipped at {}", r.peak);
    // Gross discontinuities (a click from an unsmoothed parameter) would dwarf
    // the band-limited content's sample-to-sample motion.
    assert!(
        r.max_step < 0.3,
        "discontinuity in output: max step {}",
        r.max_step
    );
}

fn render_run(g: &GhostPlayback, dt: f32, levels: Levels) -> Rendered {
    let mut voice = MaglevVoice::new(SR);
    voice.set_levels(levels);
    let mut detector = EventDetector::default();

    let per_tick = (SR * dt) as usize;
    let mut buf = vec![0.0f32; per_tick * 2];

    let mut on_sum = 0.0f64;
    let mut on_n = 0usize;
    let mut off_sum = 0.0f64;
    let mut off_n = 0usize;
    let mut peak = 0.0f32;
    let mut max_step = 0.0f32;
    let (mut catches, mut releases, mut lands) = (0, 0, 0);

    let mut prev_vel = g.frames[0].velocity;
    let mut sync_display = 0.0f32;
    let mut last_sample = 0.0f32;

    for (i, f) in g.frames.iter().enumerate() {
        let on_ramp = g.on_ramp[i];
        let wishing = f.forward_move.abs() + f.side_move.abs() > 0.0;
        let sync = air_strafe_sync(prev_vel, f.velocity, wishing, !f.grounded);
        // Same asymmetric display smoothing the HUD and the audio params use.
        if sync > sync_display {
            sync_display = sync_display * 0.5 + sync * 0.5;
        } else {
            sync_display = sync_display * 0.85 + sync * 0.15;
        }
        prev_vel = f.velocity;

        let speed = f.velocity.length_2d();
        for ev in detector
            .observe(Observation {
                dt,
                speed,
                on_ramp,
                grounded: f.grounded,
                wiped: false,
            })
            .iter()
        {
            match ev {
                AudioEvent::Catch { .. } => catches += 1,
                AudioEvent::Release { .. } => releases += 1,
                AudioEvent::Land { .. } => lands += 1,
                _ => {}
            }
            voice.trigger(ev);
        }

        voice.set_params(Params {
            speed,
            sync: sync_display,
            contact: if on_ramp { 1.0 } else { 0.0 },
        });
        buf.iter_mut().for_each(|s| *s = 0.0);
        voice.render(&mut buf, 2);

        let mut sq = 0.0f64;
        for pair in buf.chunks(2) {
            let l = pair[0];
            sq += (l * l) as f64;
            peak = peak.max(l.abs());
            max_step = max_step.max((l - last_sample).abs());
            last_sample = l;
        }
        let ms = sq / per_tick as f64;
        if on_ramp {
            on_sum += ms;
            on_n += 1;
        } else {
            off_sum += ms;
            off_n += 1;
        }
        assert!(buf.iter().all(|s| s.is_finite()), "non-finite at frame {i}");
    }

    Rendered {
        on_ramp_rms: (on_sum / on_n.max(1) as f64).sqrt() as f32,
        off_ramp_rms: (off_sum / off_n.max(1) as f64).sqrt() as f32,
        peak,
        max_step,
        catches,
        releases,
        lands,
    }
}
