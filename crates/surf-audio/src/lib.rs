//! Procedural game audio for osx-surf. No samples, no audio assets.
//!
//! Two design rules drive everything here:
//!
//! 1. **Reward is additive; failure is subtractive.** Strafing well adds a
//!    sound. Strafing badly removes it rather than adding a punishing one, and
//!    there is no buzzer or downward "wrong" cue anywhere in this crate.
//! 2. **The player's own music is the music.** Every resonance glides with
//!    speed instead of sitting on a pitch, nothing pulses on a tempo grid, and
//!    the mix sits mostly below and above the band music occupies.
//!
//! Everything is synthesized, so there is nothing to license or redistribute.
//!
//! The synthesis is hand-rolled ([`dsp`]) to match the project's no-engine
//! stance and to keep per-sample control of the parameter smoothing, which is
//! what stops the 66.67 Hz sim from zippering the filters.

pub mod detect;
pub mod dsp;
pub mod engine;
pub mod voice;

pub use detect::{EventDetector, Emitted, Observation};
pub use engine::AudioEngine;
pub use voice::{AudioEvent, Levels, MaglevVoice, Params, WipeStyle, SPEED_REF};
