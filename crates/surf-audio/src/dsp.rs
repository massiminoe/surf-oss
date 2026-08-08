//! Minimal f32 DSP primitives. Hand-rolled to match the project's no-engine
//! stance, and because the parameter smoothing here is the whole ballgame:
//! the sim steps at 66.67 Hz and any parameter stepped at that rate zippers
//! audibly, so everything the game drives is smoothed per sample.
//!
//! Nothing in this module allocates. Everything is `f32`.

use std::f32::consts::TAU;

/// Below this, treat as zero — keeps high-Q filter tails out of denormal range.
const DENORMAL: f32 = 1e-20;

#[inline]
pub fn smoothstep(a: f32, b: f32, x: f32) -> f32 {
    if (b - a).abs() < f32::EPSILON {
        return if x < a { 0.0 } else { 1.0 };
    }
    let t = ((x - a) / (b - a)).clamp(0.0, 1.0);
    t * t * (3.0 - 2.0 * t)
}

#[inline]
pub fn lerp(a: f32, b: f32, t: f32) -> f32 {
    a + (b - a) * t
}

/// One-pole lowpass used as a parameter smoother.
///
/// `tc` is the time constant in seconds: the step response reaches ~63% of the
/// target after `tc`.
#[derive(Clone, Copy, Debug)]
pub struct OnePole {
    y: f32,
    a: f32,
}

impl OnePole {
    pub fn new(tc: f32, sr: f32) -> Self {
        let mut p = Self { y: 0.0, a: 1.0 };
        p.set_tc(tc, sr);
        p
    }

    pub fn set_tc(&mut self, tc: f32, sr: f32) {
        self.a = if tc <= 0.0 || sr <= 0.0 {
            1.0
        } else {
            1.0 - (-1.0 / (tc * sr)).exp()
        };
    }

    #[inline]
    pub fn process(&mut self, x: f32) -> f32 {
        self.y += (x - self.y) * self.a;
        if self.y.abs() < DENORMAL {
            self.y = 0.0;
        }
        self.y
    }

    #[inline]
    pub fn value(&self) -> f32 {
        self.y
    }

    pub fn reset(&mut self, v: f32) {
        self.y = v;
    }
}

/// Asymmetric smoother: separate attack and release time constants. Used for
/// the ramp-contact gate, where the release wants to be slower than the attack
/// so leaving a ramp reads as a release rather than a cut.
#[derive(Clone, Copy, Debug)]
pub struct Gate {
    y: f32,
    a_up: f32,
    a_dn: f32,
}

impl Gate {
    pub fn new(attack: f32, release: f32, sr: f32) -> Self {
        Self {
            y: 0.0,
            a_up: 1.0 - (-1.0 / (attack.max(1e-6) * sr)).exp(),
            a_dn: 1.0 - (-1.0 / (release.max(1e-6) * sr)).exp(),
        }
    }

    #[inline]
    pub fn process(&mut self, x: f32) -> f32 {
        let a = if x > self.y { self.a_up } else { self.a_dn };
        self.y += (x - self.y) * a;
        if self.y.abs() < DENORMAL {
            self.y = 0.0;
        }
        self.y
    }
}

/// RBJ cookbook biquad, direct form I.
#[derive(Clone, Copy, Debug)]
pub struct Biquad {
    b0: f32,
    b1: f32,
    b2: f32,
    a1: f32,
    a2: f32,
    x1: f32,
    x2: f32,
    y1: f32,
    y2: f32,
}

impl Default for Biquad {
    fn default() -> Self {
        Self {
            b0: 1.0,
            b1: 0.0,
            b2: 0.0,
            a1: 0.0,
            a2: 0.0,
            x1: 0.0,
            x2: 0.0,
            y1: 0.0,
            y2: 0.0,
        }
    }
}

impl Biquad {
    /// Constant 0 dB peak-gain bandpass. `q` above ~20 rings hard — that is the
    /// point: `q` is what carries strafe sync.
    pub fn set_bandpass(&mut self, f0: f32, q: f32, sr: f32) {
        let (w0, sin_w0, cos_w0) = Self::omega(f0, sr);
        let _ = w0;
        let alpha = sin_w0 / (2.0 * q.max(0.05));
        let a0 = 1.0 + alpha;
        self.b0 = alpha / a0;
        self.b1 = 0.0;
        self.b2 = -alpha / a0;
        self.a1 = -2.0 * cos_w0 / a0;
        self.a2 = (1.0 - alpha) / a0;
    }

    pub fn set_lowpass(&mut self, f0: f32, q: f32, sr: f32) {
        let (_, sin_w0, cos_w0) = Self::omega(f0, sr);
        let alpha = sin_w0 / (2.0 * q.max(0.05));
        let a0 = 1.0 + alpha;
        let b = (1.0 - cos_w0) / 2.0;
        self.b0 = b / a0;
        self.b1 = (1.0 - cos_w0) / a0;
        self.b2 = b / a0;
        self.a1 = -2.0 * cos_w0 / a0;
        self.a2 = (1.0 - alpha) / a0;
    }

    pub fn set_highpass(&mut self, f0: f32, q: f32, sr: f32) {
        let (_, sin_w0, cos_w0) = Self::omega(f0, sr);
        let alpha = sin_w0 / (2.0 * q.max(0.05));
        let a0 = 1.0 + alpha;
        let b = (1.0 + cos_w0) / 2.0;
        self.b0 = b / a0;
        self.b1 = -(1.0 + cos_w0) / a0;
        self.b2 = b / a0;
        self.a1 = -2.0 * cos_w0 / a0;
        self.a2 = (1.0 - alpha) / a0;
    }

    /// Clamped so a runaway parameter can never push `f0` past Nyquist.
    fn omega(f0: f32, sr: f32) -> (f32, f32, f32) {
        let f = f0.clamp(10.0, sr * 0.45);
        let w0 = TAU * f / sr;
        (w0, w0.sin(), w0.cos())
    }

    #[inline]
    pub fn process(&mut self, x: f32) -> f32 {
        let y = self.b0 * x + self.b1 * self.x1 + self.b2 * self.x2
            - self.a1 * self.y1
            - self.a2 * self.y2;
        let y = if y.is_finite() { y } else { 0.0 };
        self.x2 = self.x1;
        self.x1 = x;
        self.y2 = self.y1;
        self.y1 = if y.abs() < DENORMAL { 0.0 } else { y };
        y
    }

    pub fn reset(&mut self) {
        self.x1 = 0.0;
        self.x2 = 0.0;
        self.y1 = 0.0;
        self.y2 = 0.0;
    }
}

/// xorshift32 white noise, uniform in [-1, 1).
#[derive(Clone, Copy, Debug)]
pub struct Noise {
    s: u32,
}

impl Noise {
    pub fn new(seed: u32) -> Self {
        Self {
            s: if seed == 0 { 0x9E3779B9 } else { seed },
        }
    }

    #[inline]
    pub fn sample(&mut self) -> f32 {
        self.s ^= self.s << 13;
        self.s ^= self.s >> 17;
        self.s ^= self.s << 5;
        (self.s as f32 / u32::MAX as f32) * 2.0 - 1.0
    }
}

/// Phase-accumulator sine. Frequency may change every sample without clicking.
#[derive(Clone, Copy, Debug, Default)]
pub struct Sine {
    phase: f32,
}

impl Sine {
    #[inline]
    pub fn sample(&mut self, f: f32, sr: f32) -> f32 {
        self.phase += f / sr;
        if self.phase >= 1.0 {
            self.phase -= self.phase.floor();
        }
        (self.phase * TAU).sin()
    }

    pub fn reset(&mut self) {
        self.phase = 0.0;
    }
}

/// Damped comb filter (Schroeder reverb building block).
struct Comb {
    buf: Vec<f32>,
    idx: usize,
    store: f32,
    feedback: f32,
    damp: f32,
}

impl Comb {
    fn new(len: usize, feedback: f32, damp: f32) -> Self {
        Self {
            buf: vec![0.0; len.max(1)],
            idx: 0,
            store: 0.0,
            feedback,
            damp,
        }
    }

    #[inline]
    fn process(&mut self, x: f32) -> f32 {
        let y = self.buf[self.idx];
        self.store = y * (1.0 - self.damp) + self.store * self.damp;
        if self.store.abs() < DENORMAL {
            self.store = 0.0;
        }
        self.buf[self.idx] = x + self.store * self.feedback;
        self.idx = (self.idx + 1) % self.buf.len();
        y
    }
}

/// Schroeder allpass.
struct Allpass {
    buf: Vec<f32>,
    idx: usize,
}

impl Allpass {
    fn new(len: usize) -> Self {
        Self {
            buf: vec![0.0; len.max(1)],
            idx: 0,
        }
    }

    #[inline]
    fn process(&mut self, x: f32) -> f32 {
        let buffered = self.buf[self.idx];
        let out = -x + buffered;
        self.buf[self.idx] = x + buffered * 0.5;
        self.idx = (self.idx + 1) % self.buf.len();
        out
    }
}

/// Small Schroeder reverb for the ramp-release bloom. Mono in, stereo out;
/// the right channel uses offset delay lengths for width.
///
/// Buffers are allocated once at construction — never in the audio callback.
pub struct Reverb {
    combs_l: Vec<Comb>,
    combs_r: Vec<Comb>,
    aps_l: Vec<Allpass>,
    aps_r: Vec<Allpass>,
}

impl Reverb {
    pub fn new(sr: f32) -> Self {
        // Classic Schroeder/Freeverb tunings, given at 44.1 kHz.
        const COMB: [usize; 4] = [1116, 1188, 1277, 1356];
        const AP: [usize; 2] = [556, 441];
        const SPREAD: usize = 23;
        let scale = |n: usize| ((n as f32) * sr / 44100.0) as usize;

        Self {
            combs_l: COMB.iter().map(|&n| Comb::new(scale(n), 0.82, 0.35)).collect(),
            combs_r: COMB
                .iter()
                .map(|&n| Comb::new(scale(n + SPREAD), 0.82, 0.35))
                .collect(),
            aps_l: AP.iter().map(|&n| Allpass::new(scale(n))).collect(),
            aps_r: AP
                .iter()
                .map(|&n| Allpass::new(scale(n + SPREAD)))
                .collect(),
        }
    }

    #[inline]
    pub fn process(&mut self, x: f32) -> (f32, f32) {
        let mut l = 0.0;
        let mut r = 0.0;
        for c in &mut self.combs_l {
            l += c.process(x);
        }
        for c in &mut self.combs_r {
            r += c.process(x);
        }
        for a in &mut self.aps_l {
            l = a.process(l);
        }
        for a in &mut self.aps_r {
            r = a.process(r);
        }
        (l * 0.22, r * 0.22)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const SR: f32 = 48_000.0;

    #[test]
    fn one_pole_approaches_target_at_time_constant() {
        let mut p = OnePole::new(0.05, SR);
        for _ in 0..(0.05 * SR) as usize {
            p.process(1.0);
        }
        // One time constant -> ~63.2%.
        assert!(
            (p.value() - 0.632).abs() < 0.02,
            "after 1 tc: {}",
            p.value()
        );
    }

    #[test]
    fn gate_release_is_slower_than_attack() {
        let mut g = Gate::new(0.03, 0.14, SR);
        for _ in 0..(0.03 * SR) as usize {
            g.process(1.0);
        }
        let after_attack = g.y;
        let mut g2 = Gate::new(0.03, 0.14, SR);
        g2.y = 1.0;
        for _ in 0..(0.03 * SR) as usize {
            g2.process(0.0);
        }
        // Same elapsed time: the fall has covered less ground than the rise.
        assert!(after_attack > 1.0 - g2.y, "attack {after_attack} fall {}", 1.0 - g2.y);
    }

    #[test]
    fn bandpass_passes_centre_and_rejects_far_tones() {
        let f0 = 500.0;
        let run = |f: f32| {
            let mut bq = Biquad::default();
            bq.set_bandpass(f0, 8.0, SR);
            let mut osc = Sine::default();
            let mut peak: f32 = 0.0;
            // Settle, then measure.
            for i in 0..8000 {
                let y = bq.process(osc.sample(f, SR));
                if i > 4000 {
                    peak = peak.max(y.abs());
                }
            }
            peak
        };
        let at_centre = run(f0);
        let far_below = run(f0 / 12.0);
        let far_above = run(f0 * 12.0);
        assert!(at_centre > 0.7, "centre gain {at_centre}");
        assert!(far_below < at_centre * 0.15, "below {far_below}");
        assert!(far_above < at_centre * 0.15, "above {far_above}");
    }

    #[test]
    fn high_q_bandpass_stays_finite() {
        let mut bq = Biquad::default();
        bq.set_bandpass(1400.0, 40.0, SR);
        let mut n = Noise::new(1);
        let mut peak: f32 = 0.0;
        for _ in 0..SR as usize {
            let y = bq.process(n.sample());
            assert!(y.is_finite());
            peak = peak.max(y.abs());
        }
        assert!(peak < 20.0, "unexpected blow-up: {peak}");
    }

    #[test]
    fn noise_is_bounded_and_centred() {
        let mut n = Noise::new(12345);
        let mut sum = 0.0f64;
        const N: usize = 200_000;
        for _ in 0..N {
            let v = n.sample();
            assert!((-1.0..=1.0).contains(&v));
            sum += v as f64;
        }
        assert!((sum / N as f64).abs() < 0.01, "mean {}", sum / N as f64);
    }

    #[test]
    fn sine_holds_amplitude_across_a_frequency_sweep() {
        let mut s = Sine::default();
        let mut peak: f32 = 0.0;
        for i in 0..48_000 {
            let f = lerp(40.0, 900.0, i as f32 / 48_000.0);
            peak = peak.max(s.sample(f, SR).abs());
        }
        assert!((peak - 1.0).abs() < 0.02, "peak {peak}");
    }

    #[test]
    fn reverb_decays_to_silence() {
        let mut rv = Reverb::new(SR);
        for _ in 0..64 {
            rv.process(1.0);
        }
        let mut tail: f32 = 0.0;
        for _ in 0..(SR as usize * 6) {
            let (l, r) = rv.process(0.0);
            tail = l.abs().max(r.abs());
        }
        assert!(tail < 1e-3, "reverb still ringing at {tail}");
    }
}
