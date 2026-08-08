//! CORE.A "Maglev": the continuous surf voice plus its essential transients.
//!
//! Three layers, all driven by state the sim already computes:
//!
//! * **sub** — felt more than heard, level and pitch follow speed.
//! * **air** — broadband rush, always present, scaled by speed.
//! * **core** — noise through two tracking bandpass filters, gated by ramp
//!   contact. Speed moves the centre frequency; **strafe sync moves Q**, so
//!   good strafing resolves noise into tone at roughly constant loudness.
//!
//! The design rule is that reward is additive and failure is subtractive:
//! strafing badly removes the core rather than adding anything punishing, and
//! there is no downward-pitched "wrong" sound anywhere in this file.
//!
//! Nothing here allocates or blocks; it runs on the CoreAudio callback thread.

use crate::dsp::{lerp, smoothstep, Biquad, Gate, Noise, OnePole, Reverb, Sine};

/// Speed that maps to "full" on every speed-driven curve, in u/s.
pub const SPEED_REF: f32 = 2200.0;

/// Filter coefficients are recomputed this often (in samples). The smoothed
/// *values* still move every sample; only the trig is strided, at ~1.5 kHz,
/// far above anything that could zipper.
const COEFF_STRIDE: u32 = 32;

const EVT_SLOTS: usize = 12;

/// Core resonance range. Q is the whole point of this voice: it is what strafe
/// sync moves, and what turns broadband noise into tone.
const CORE_Q_MIN: f32 = 0.7;
const CORE_Q_RANGE: f32 = 21.0;
const CORE_BASE: f32 = 0.10;

/// Discrete moments worth a sound. Deliberately short: this is the "essential
/// events" set, not the full lab.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum AudioEvent {
    /// Made contact with a surf ramp.
    Catch { speed: f32 },
    /// Left a surf ramp into open air.
    Release { speed: f32 },
    /// Touched walkable ground. `step` is the consecutive-hop index.
    Land { step: u8, speed: f32 },
    /// Kill-z or fail teleport. Never a death sound — see [`WipeStyle`].
    Wipe,
    /// Start zone armed, or a manual reset.
    Rearm,
}

/// The two non-punishing failure treatments. Both frame a wipe as "again"
/// rather than "you lost"; which one wears better over a grind session is the
/// open question, so both ship.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum WipeStyle {
    /// Reverse swell that ducks the world and cuts to silence, then re-arms.
    Rewind,
    /// Everything low-passes down and evaporates.
    Dissolve,
}

impl WipeStyle {
    pub fn as_str(self) -> &'static str {
        match self {
            WipeStyle::Rewind => "rewind",
            WipeStyle::Dissolve => "dissolve",
        }
    }

    pub fn from_str_or_default(s: &str) -> Self {
        match s {
            "dissolve" => WipeStyle::Dissolve,
            _ => WipeStyle::Rewind,
        }
    }

    pub fn toggled(self) -> Self {
        match self {
            WipeStyle::Rewind => WipeStyle::Dissolve,
            WipeStyle::Dissolve => WipeStyle::Rewind,
        }
    }
}

/// Per-layer mix, exposed in the pause menu so this can be tuned by ear.
#[derive(Clone, Copy, Debug)]
pub struct Levels {
    pub master: f32,
    pub core: f32,
    pub air: f32,
    pub sub: f32,
}

impl Default for Levels {
    fn default() -> Self {
        Self {
            master: 0.7,
            core: 1.0,
            air: 1.0,
            sub: 1.0,
        }
    }
}

/// Continuous state pushed from the game thread each frame.
#[derive(Clone, Copy, Debug, Default)]
pub struct Params {
    /// Horizontal speed, u/s.
    pub speed: f32,
    /// Air-strafe sync, 0..100.
    pub sync: f32,
    /// 1.0 while touching a surf ramp, else 0.0.
    pub contact: f32,
}

/// One transient: a filtered noise band and/or a sine body, each swept.
#[derive(Clone, Copy)]
struct Evt {
    active: bool,
    t: f32,
    dur: f32,
    attack: f32,
    /// Envelope rises slowly then cuts, instead of striking then decaying.
    reverse: bool,
    /// Bypasses the wipe duck, so a wipe sound isn't ducked by its own duck.
    post_duck: bool,
    n_amp: f32,
    n_f0: f32,
    n_f1: f32,
    n_q: f32,
    n_highpass: bool,
    s_amp: f32,
    s_f0: f32,
    s_f1: f32,
    send: f32,
    filt: Biquad,
    noise: Noise,
    sine: Sine,
    ctr: u32,
}

impl Default for Evt {
    fn default() -> Self {
        Self {
            active: false,
            t: 0.0,
            dur: 0.1,
            attack: 0.004,
            reverse: false,
            post_duck: false,
            n_amp: 0.0,
            n_f0: 500.0,
            n_f1: 500.0,
            n_q: 2.0,
            n_highpass: false,
            s_amp: 0.0,
            s_f0: 100.0,
            s_f1: 100.0,
            send: 0.0,
            filt: Biquad::default(),
            noise: Noise::new(0x1234_5678),
            sine: Sine::default(),
            ctr: 0,
        }
    }
}

impl Evt {
    fn envelope(&self) -> f32 {
        let u = (self.t / self.dur).clamp(0.0, 1.0);
        if self.reverse {
            // Slow rise, then the caller cuts at the end. Reads as a rewind.
            u * u * u.sqrt()
        } else {
            let a = if self.attack > 0.0 {
                (self.t / self.attack).clamp(0.0, 1.0)
            } else {
                1.0
            };
            let decay = (1.0 - u) * (1.0 - u) * (-3.0 * u).exp();
            a * decay
        }
    }

    /// Returns `(dry, wet_send)`.
    #[inline]
    fn render(&mut self, sr: f32) -> (f32, f32) {
        if !self.active {
            return (0.0, 0.0);
        }
        let u = (self.t / self.dur).clamp(0.0, 1.0);
        let env = self.envelope();

        if self.ctr.is_multiple_of(COEFF_STRIDE) {
            // Exponential glide: pitch reads as musical motion, not a linear ramp.
            let f = self.n_f0 * (self.n_f1 / self.n_f0).powf(u);
            if self.n_highpass {
                self.filt.set_highpass(f, self.n_q, sr);
            } else {
                self.filt.set_bandpass(f, self.n_q, sr);
            }
        }
        self.ctr = self.ctr.wrapping_add(1);

        let mut out = 0.0;
        if self.n_amp > 0.0 {
            out += self.filt.process(self.noise.sample()) * self.n_amp;
        }
        if self.s_amp > 0.0 {
            let f = self.s_f0 * (self.s_f1 / self.s_f0).powf(u);
            out += self.sine.sample(f, sr) * self.s_amp;
        }
        out *= env;

        self.t += 1.0 / sr;
        if self.t >= self.dur {
            self.active = false;
        }
        (out, out * self.send)
    }
}

#[derive(Clone, Copy, PartialEq)]
enum Wipe {
    Idle,
    Rewind(f32),
    Dissolve(f32),
}

// Rewind timeline, seconds.
const REWIND_SWELL: f32 = 0.44;
const REWIND_CUT: f32 = 0.47;
const REWIND_REARM: f32 = 0.72;
const REWIND_END: f32 = 0.95;
// Dissolve timeline, seconds.
const DISSOLVE_FALL: f32 = 0.42;
const DISSOLVE_REARM: f32 = 0.56;
const DISSOLVE_END: f32 = 0.75;

/// Idle cutoff for the dissolve filter: high enough to be transparent, so the
/// filter can stay in the path and never introduce a switching discontinuity.
const DISSOLVE_OPEN_HZ: f32 = 18_000.0;

pub struct MaglevVoice {
    sr: f32,
    params: Params,
    levels: Levels,
    wipe_style: WipeStyle,
    enabled: bool,

    speed_sm: OnePole,
    sync_sm: OnePole,
    contact_gate: Gate,
    enable_gain: OnePole,
    duck: OnePole,

    core_noise: Noise,
    core_b1: Biquad,
    core_b2: Biquad,

    air_noise_l: Noise,
    air_noise_r: Noise,
    air_bp_l: Biquad,
    air_bp_r: Biquad,

    sub: Sine,

    dissolve_lp_l: Biquad,
    dissolve_lp_r: Biquad,

    reverb: Reverb,
    events: [Evt; EVT_SLOTS],
    wipe: Wipe,
    /// Set once per wipe so the timed sub-events fire exactly once.
    wipe_fired_rearm: bool,
    coeff_ctr: u32,
}

impl MaglevVoice {
    pub fn new(sr: f32) -> Self {
        Self {
            sr,
            params: Params::default(),
            levels: Levels::default(),
            wipe_style: WipeStyle::Rewind,
            enabled: true,

            // Speed may change fast on a bad clip; sync is noisy, so it gets more.
            speed_sm: OnePole::new(0.05, sr),
            sync_sm: OnePole::new(0.09, sr),
            contact_gate: Gate::new(0.03, 0.14, sr),
            enable_gain: OnePole::new(0.05, sr),
            duck: OnePole::new(0.02, sr),

            core_noise: Noise::new(0x2545_F491),
            core_b1: Biquad::default(),
            core_b2: Biquad::default(),

            air_noise_l: Noise::new(0x9E37_79B9),
            air_noise_r: Noise::new(0x85EB_CA6B),
            air_bp_l: Biquad::default(),
            air_bp_r: Biquad::default(),

            sub: Sine::default(),

            dissolve_lp_l: Biquad::default(),
            dissolve_lp_r: Biquad::default(),

            reverb: Reverb::new(sr),
            events: [Evt::default(); EVT_SLOTS],
            wipe: Wipe::Idle,
            wipe_fired_rearm: false,
            coeff_ctr: 0,
        }
    }

    pub fn set_params(&mut self, p: Params) {
        self.params = p;
    }

    pub fn set_levels(&mut self, l: Levels) {
        self.levels = l;
    }

    pub fn set_wipe_style(&mut self, s: WipeStyle) {
        self.wipe_style = s;
    }

    pub fn set_enabled(&mut self, on: bool) {
        self.enabled = on;
    }

    fn slot(&mut self) -> Option<&mut Evt> {
        // Prefer a free slot; if the pool is full, steal the most-elapsed voice
        // rather than dropping the newest — the newest carries the information.
        if let Some(i) = self.events.iter().position(|e| !e.active) {
            return Some(&mut self.events[i]);
        }
        let mut best = 0usize;
        let mut best_u = -1.0f32;
        for (i, e) in self.events.iter().enumerate() {
            let u = e.t / e.dur;
            if u > best_u {
                best_u = u;
                best = i;
            }
        }
        Some(&mut self.events[best])
    }

    fn spawn(&mut self, mut e: Evt) {
        // Preserve each slot's noise state so repeated events don't correlate.
        let seed = self.events.iter().map(|s| s.ctr).sum::<u32>() ^ 0xA5A5_1234;
        e.noise = Noise::new(seed | 1);
        e.active = true;
        e.t = 0.0;
        e.ctr = 0;
        e.filt.reset();
        e.sine.reset();
        if let Some(s) = self.slot() {
            *s = e;
        }
    }

    pub fn trigger(&mut self, ev: AudioEvent) {
        match ev {
            AudioEvent::Catch { speed } => {
                let k = 0.5 + 0.5 * (speed / 1500.0).clamp(0.0, 1.0);
                self.spawn(Evt {
                    dur: 0.13,
                    n_amp: 0.11 * k,
                    n_f0: 260.0,
                    n_f1: 1500.0,
                    n_q: 3.0,
                    s_amp: 0.16 * k,
                    s_f0: 78.0,
                    s_f1: 46.0,
                    send: 0.25,
                    ..Evt::default()
                });
            }
            AudioEvent::Release { speed } => {
                let k = 0.5 + 0.5 * (speed / 1500.0).clamp(0.0, 1.0);
                self.spawn(Evt {
                    dur: 0.42,
                    attack: 0.01,
                    n_amp: 0.075 * k,
                    n_f0: 1800.0,
                    n_f1: 420.0,
                    n_q: 2.2,
                    send: 0.9,
                    ..Evt::default()
                });
            }
            AudioEvent::Land { step, speed } => {
                let k = 0.45 + 0.55 * (speed / 1200.0).clamp(0.0, 1.0);
                // Consecutive hops step the click upward. It resets itself, and
                // it follows the player's rhythm rather than any tempo grid.
                let click = 1800.0 * 1.09_f32.powi(step.min(8) as i32);
                self.spawn(Evt {
                    dur: 0.035,
                    attack: 0.002,
                    n_amp: 0.075 * k,
                    n_f0: click,
                    n_f1: click,
                    n_q: 1.4,
                    n_highpass: true,
                    s_amp: 0.19 * k,
                    s_f0: 62.0,
                    s_f1: 40.0,
                    ..Evt::default()
                });
            }
            AudioEvent::Rearm => {
                self.spawn(Evt {
                    dur: 0.05,
                    attack: 0.003,
                    s_amp: 0.03,
                    s_f0: 1180.0,
                    s_f1: 1180.0,
                    send: 0.35,
                    post_duck: true,
                    ..Evt::default()
                });
            }
            AudioEvent::Wipe => {
                self.wipe_fired_rearm = false;
                match self.wipe_style {
                    WipeStyle::Rewind => {
                        self.wipe = Wipe::Rewind(0.0);
                        self.spawn(Evt {
                            dur: REWIND_CUT,
                            reverse: true,
                            post_duck: true,
                            n_amp: 0.11,
                            n_f0: 1600.0,
                            n_f1: 340.0,
                            n_q: 3.5,
                            send: 0.5,
                            ..Evt::default()
                        });
                    }
                    WipeStyle::Dissolve => {
                        self.wipe = Wipe::Dissolve(0.0);
                    }
                }
            }
        }
    }

    /// Advance the wipe state machine by one sample, returning
    /// `(duck_target, dissolve_cutoff_hz)`.
    fn wipe_step(&mut self) -> (f32, f32) {
        let dt = 1.0 / self.sr;
        match self.wipe {
            Wipe::Idle => (1.0, DISSOLVE_OPEN_HZ),
            Wipe::Rewind(t) => {
                let nt = t + dt;
                self.wipe = if nt >= REWIND_END {
                    Wipe::Idle
                } else {
                    Wipe::Rewind(nt)
                };
                if nt >= REWIND_REARM && !self.wipe_fired_rearm {
                    self.wipe_fired_rearm = true;
                    self.trigger(AudioEvent::Rearm);
                }
                let duck = if nt < REWIND_SWELL {
                    // Sink away under the rising swell...
                    lerp(1.0, 0.25, nt / REWIND_SWELL)
                } else if nt < REWIND_REARM {
                    // ...then cut, and hold silence until the re-arm tick.
                    0.0
                } else {
                    1.0
                };
                (duck, DISSOLVE_OPEN_HZ)
            }
            Wipe::Dissolve(t) => {
                let nt = t + dt;
                self.wipe = if nt >= DISSOLVE_END {
                    Wipe::Idle
                } else {
                    Wipe::Dissolve(nt)
                };
                if nt >= DISSOLVE_REARM && !self.wipe_fired_rearm {
                    self.wipe_fired_rearm = true;
                    self.trigger(AudioEvent::Rearm);
                }
                if nt < DISSOLVE_FALL {
                    let u = nt / DISSOLVE_FALL;
                    let cutoff = DISSOLVE_OPEN_HZ * (180.0f32 / DISSOLVE_OPEN_HZ).powf(u);
                    (1.0 - u, cutoff)
                } else if nt < DISSOLVE_REARM {
                    (0.0, 180.0)
                } else {
                    (1.0, DISSOLVE_OPEN_HZ)
                }
            }
        }
    }

    /// Render interleaved stereo into `out`. `channels` is the device's channel
    /// count; channels above the second are fed the left signal.
    pub fn render(&mut self, out: &mut [f32], channels: usize) {
        if channels == 0 {
            return;
        }
        let sr = self.sr;
        let target_enable = if self.enabled { 1.0 } else { 0.0 };

        for frame in out.chunks_mut(channels) {
            // --- smoothed parameters (the anti-zipper layer) ---
            let speed = self.speed_sm.process(self.params.speed);
            let sync = self.sync_sm.process(self.params.sync);
            let contact = self.contact_gate.process(self.params.contact);
            let enable = self.enable_gain.process(target_enable);

            let s = (speed / SPEED_REF).clamp(0.0, 1.35);
            let sy = (sync / 100.0).clamp(0.0, 1.0);

            let (duck_target, dissolve_hz) = self.wipe_step();
            let duck = self.duck.process(duck_target);

            // --- coefficients ---
            if self.coeff_ctr.is_multiple_of(COEFF_STRIDE) {
                // Centre glides with speed, so the voice never sits on a pitch
                // that could imply a key against the player's own music.
                let centre = 150.0 + 900.0 * s;
                let q = CORE_Q_MIN + CORE_Q_RANGE * sy * sy;
                self.core_b1.set_bandpass(centre, q, sr);
                self.core_b2.set_bandpass(centre * 2.01, 0.6 + 13.0 * sy * sy, sr);

                let air_f = 900.0 + 5400.0 * s;
                self.air_bp_l.set_bandpass(air_f, 0.6, sr);
                self.air_bp_r.set_bandpass(air_f * 1.03, 0.6, sr);

                self.dissolve_lp_l.set_lowpass(dissolve_hz, 0.707, sr);
                self.dissolve_lp_r.set_lowpass(dissolve_hz, 0.707, sr);
            }
            self.coeff_ctr = self.coeff_ctr.wrapping_add(1);

            // --- core: gated by ramp contact, character from sync ---
            //
            // White noise through a constant-peak-gain bandpass carries energy
            // proportional to the band's width, so RMS falls as ~1/sqrt(Q).
            // Left uncompensated the core would *vanish* exactly when the
            // player strafes well, which is backwards. Normalising by sqrt(Q)
            // holds loudness roughly flat and lets Q carry timbre alone — the
            // change reads as chaos resolving into tone, not as a volume pump.
            let q = CORE_Q_MIN + CORE_Q_RANGE * sy * sy;
            let q_norm = (q / CORE_Q_MIN).sqrt();
            let core_amp =
                CORE_BASE * q_norm * (0.35 + 0.65 * s) * contact * self.levels.core;
            let n = self.core_noise.sample();
            let core = (self.core_b1.process(n) + 0.45 * self.core_b2.process(n)) * core_amp;

            // --- air: always on, speed-scaled, decorrelated for width ---
            let air_amp = 0.052 * s.powf(1.25) * self.levels.air;
            let air_l = self.air_bp_l.process(self.air_noise_l.sample()) * air_amp;
            let air_r = self.air_bp_r.process(self.air_noise_r.sample()) * air_amp;

            // --- sub: felt, not heard ---
            let sub_f = 27.0 + 21.0 * s;
            let sub_amp = 0.115 * smoothstep(0.06, 0.55, s) * self.levels.sub;
            let sub = self.sub.sample(sub_f, sr) * sub_amp;

            let mut dry_l = core + air_l + sub;
            let mut dry_r = core + air_r + sub;
            let mut wet = core * 0.16;
            let mut post_l = 0.0;
            let mut post_r = 0.0;

            // --- transients ---
            for e in &mut self.events {
                if !e.active {
                    continue;
                }
                let post = e.post_duck;
                let (d, w) = e.render(sr);
                if post {
                    post_l += d;
                    post_r += d;
                } else {
                    dry_l += d;
                    dry_r += d;
                }
                wet += w;
            }

            // --- bus: duck, dissolve, reverb ---
            let (rv_l, rv_r) = self.reverb.process(wet);
            let mut l = (dry_l + rv_l) * duck;
            let mut r = (dry_r + rv_r) * duck;
            l = self.dissolve_lp_l.process(l);
            r = self.dissolve_lp_r.process(r);
            l = (l + post_l) * self.levels.master * enable;
            r = (r + post_r) * self.levels.master * enable;

            // Backstop only: correct levels never reach this.
            let l = l.clamp(-0.98, 0.98);
            let r = r.clamp(-0.98, 0.98);

            for (i, sample) in frame.iter_mut().enumerate() {
                *sample = if i == 1 { r } else { l };
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::dsp::Biquad;

    const SR: f32 = 48_000.0;

    /// Core only — the sub is a loud pure sine and the air is broadband, so
    /// both swamp any measurement aimed at the core.
    const CORE_ONLY: Levels = Levels {
        master: 1.0,
        core: 1.0,
        air: 0.0,
        sub: 0.0,
    };

    fn peak(buf: &[f32]) -> f32 {
        buf.iter().fold(0.0f32, |m, &v| m.max(v.abs()))
    }

    fn render_secs(v: &mut MaglevVoice, secs: f32) -> Vec<f32> {
        let mut buf = vec![0.0; (SR * secs) as usize * 2];
        v.render(&mut buf, 2);
        buf
    }

    fn left(buf: &[f32]) -> Vec<f32> {
        buf.iter().step_by(2).copied().collect()
    }

    fn rms(x: &[f32]) -> f32 {
        (x.iter().map(|v| v * v).sum::<f32>() / x.len().max(1) as f32).sqrt()
    }

    /// Normalised autocorrelation at `lag`. Near 1 (or -1) means the signal
    /// repeats at that period — i.e. it is ringing rather than hissing. This is
    /// the right measure of resonance; crest factor is not, since a narrowband
    /// ring approaches a sine (crest 1.41) while white noise sits nearer 3-4.
    fn autocorr(x: &[f32], lag: usize) -> f32 {
        if x.len() <= lag {
            return 0.0;
        }
        let mut num = 0.0f64;
        let mut den = 0.0f64;
        for i in 0..x.len() - lag {
            num += (x[i] * x[i + lag]) as f64;
            den += (x[i] * x[i]) as f64;
        }
        if den <= 0.0 {
            0.0
        } else {
            (num / den) as f32
        }
    }

    #[test]
    fn silent_without_ramp_contact_or_speed() {
        let mut v = MaglevVoice::new(SR);
        v.set_params(Params {
            speed: 0.0,
            sync: 0.0,
            contact: 0.0,
        });
        let buf = render_secs(&mut v, 0.5);
        assert!(peak(&buf) < 1e-4, "expected silence, got {}", peak(&buf));
    }

    #[test]
    fn core_is_gated_by_ramp_contact() {
        let run = |contact| {
            let mut v = MaglevVoice::new(SR);
            v.set_levels(CORE_ONLY);
            v.set_params(Params {
                speed: 1400.0,
                sync: 95.0,
                contact,
            });
            peak(&render_secs(&mut v, 1.0))
        };
        let off = run(0.0);
        let on = run(1.0);
        assert!(off < 1e-4, "core leaked with no ramp contact: {off}");
        assert!(on > 0.02, "core inaudible while surfing: {on}");
    }

    /// The core has to survive alongside the sub and air at default levels —
    /// measured in its own band, since broadband peak is dominated by the sub.
    #[test]
    fn core_stands_out_in_its_own_band_at_default_levels() {
        let centre = 150.0 + 900.0 * (1400.0 / SPEED_REF);
        let measure = |contact| {
            let mut v = MaglevVoice::new(SR);
            v.set_params(Params {
                speed: 1400.0,
                sync: 95.0,
                contact,
            });
            let buf = render_secs(&mut v, 1.5);
            let tail = left(&buf[(SR as usize) * 2..]);
            let mut bq = Biquad::default();
            bq.set_bandpass(centre, 2.0, SR);
            let banded: Vec<f32> = tail.iter().map(|&x| bq.process(x)).collect();
            rms(&banded)
        };
        let off = measure(0.0);
        let on = measure(1.0);
        assert!(
            on > off * 4.0,
            "core swamped by sub/air in its own band: {off} -> {on}"
        );
    }

    #[test]
    fn air_layer_is_audible_off_ramp_and_scales_with_speed() {
        let run = |speed| {
            let mut v = MaglevVoice::new(SR);
            v.set_params(Params {
                speed,
                sync: 0.0,
                contact: 0.0,
            });
            peak(&render_secs(&mut v, 1.0))
        };
        let slow = run(300.0);
        let fast = run(2000.0);
        assert!(fast > 1e-4, "fast air should be audible: {fast}");
        assert!(fast > slow * 3.0, "slow {slow} fast {fast}");
    }

    #[test]
    fn sync_sharpens_resonance_without_a_loudness_pump() {
        const SPEED: f32 = 1400.0;
        // Same centre-frequency law as the voice: 150 + 900 * (speed / SPEED_REF).
        let centre = 150.0 + 900.0 * (SPEED / SPEED_REF);
        let lag = (SR / centre).round() as usize;

        let run = |sync| {
            let mut v = MaglevVoice::new(SR);
            v.set_levels(CORE_ONLY);
            v.set_params(Params {
                speed: SPEED,
                sync,
                contact: 1.0,
            });
            let buf = render_secs(&mut v, 1.5);
            // Measure the settled tail, not the contact gate's attack.
            let tail = left(&buf[(SR as usize) * 2..]);
            (autocorr(&tail, lag), rms(&tail))
        };
        let (lo_ring, lo_rms) = run(10.0);
        let (hi_ring, hi_rms) = run(98.0);

        // Good sync rings at the centre period; bad sync is broadband.
        assert!(
            hi_ring > 0.5,
            "high sync should ring at the centre period, got {hi_ring}"
        );
        assert!(
            lo_ring < 0.25,
            "low sync should stay broadband, got {lo_ring}"
        );

        // ...but loudness stays in the same ballpark. Timbre carries sync, not volume.
        let ratio = hi_rms / lo_rms.max(1e-9);
        assert!(
            (0.4..2.5).contains(&ratio),
            "loudness moved too much with sync: x{ratio} ({lo_rms} -> {hi_rms})"
        );
    }

    #[test]
    fn output_never_clips() {
        let mut v = MaglevVoice::new(SR);
        v.set_levels(Levels {
            master: 1.0,
            core: 1.0,
            air: 1.0,
            sub: 1.0,
        });
        v.set_params(Params {
            speed: 3000.0,
            sync: 100.0,
            contact: 1.0,
        });
        // Pile every transient on at once.
        for i in 0..EVT_SLOTS {
            v.trigger(AudioEvent::Catch { speed: 3000.0 });
            v.trigger(AudioEvent::Land {
                step: i as u8,
                speed: 3000.0,
            });
        }
        let buf = render_secs(&mut v, 2.0);
        assert!(peak(&buf) <= 0.98, "clipped at {}", peak(&buf));
        assert!(buf.iter().all(|v| v.is_finite()));
    }

    #[test]
    fn disabling_fades_to_silence() {
        let mut v = MaglevVoice::new(SR);
        v.set_params(Params {
            speed: 1400.0,
            sync: 90.0,
            contact: 1.0,
        });
        render_secs(&mut v, 0.5);
        v.set_enabled(false);
        // The enable gain is a 50 ms one-pole, so allow it several time
        // constants before demanding silence.
        let buf = render_secs(&mut v, 1.0);
        let tail = &buf[buf.len() * 3 / 4..];
        assert!(peak(tail) < 1e-4, "still audible after disable: {}", peak(tail));
    }

    #[test]
    fn both_wipes_return_to_full_level_and_are_never_silent_forever() {
        for style in [WipeStyle::Rewind, WipeStyle::Dissolve] {
            let mut v = MaglevVoice::new(SR);
            v.set_wipe_style(style);
            v.set_params(Params {
                speed: 1400.0,
                sync: 90.0,
                contact: 1.0,
            });
            render_secs(&mut v, 0.3);
            v.trigger(AudioEvent::Wipe);
            let during = render_secs(&mut v, 1.0);
            assert!(
                during.iter().all(|v| v.is_finite()),
                "{style:?} produced non-finite output"
            );
            // Well after the wipe, the voice must be alive again.
            let after = render_secs(&mut v, 1.0);
            assert!(
                peak(&after) > 1e-4,
                "{style:?} never recovered (peak {})",
                peak(&after)
            );
        }
    }

    #[test]
    fn wipe_actually_ducks_the_world() {
        let mut v = MaglevVoice::new(SR);
        v.set_wipe_style(WipeStyle::Dissolve);
        v.set_params(Params {
            speed: 1400.0,
            sync: 90.0,
            contact: 1.0,
        });
        let before = peak(&render_secs(&mut v, 0.5));
        v.trigger(AudioEvent::Wipe);
        // Sample the window after the fall completes but before re-arm.
        render_secs(&mut v, DISSOLVE_FALL + 0.05);
        let quiet = peak(&render_secs(&mut v, 0.05));
        assert!(quiet < before * 0.1, "before {before} during {quiet}");
    }

    #[test]
    fn event_pool_overflow_steals_oldest_rather_than_dropping() {
        let mut v = MaglevVoice::new(SR);
        for _ in 0..EVT_SLOTS * 3 {
            v.trigger(AudioEvent::Catch { speed: 1000.0 });
        }
        let active = v.events.iter().filter(|e| e.active).count();
        assert_eq!(active, EVT_SLOTS, "pool should be saturated, not dropping");
    }

    #[test]
    fn bhop_chain_steps_the_click_upward() {
        let mut v = MaglevVoice::new(SR);
        v.trigger(AudioEvent::Land {
            step: 0,
            speed: 800.0,
        });
        let first = v.events.iter().find(|e| e.active).map(|e| e.n_f0).unwrap();
        let mut v2 = MaglevVoice::new(SR);
        v2.trigger(AudioEvent::Land {
            step: 4,
            speed: 800.0,
        });
        let fifth = v2.events.iter().find(|e| e.active).map(|e| e.n_f0).unwrap();
        assert!(fifth > first * 1.3, "{first} -> {fifth}");
    }

    #[test]
    fn wipe_style_round_trips_through_settings_string() {
        for s in [WipeStyle::Rewind, WipeStyle::Dissolve] {
            assert_eq!(WipeStyle::from_str_or_default(s.as_str()), s);
        }
        assert_eq!(
            WipeStyle::from_str_or_default("nonsense"),
            WipeStyle::Rewind
        );
        assert_eq!(WipeStyle::Rewind.toggled(), WipeStyle::Dissolve);
    }
}
