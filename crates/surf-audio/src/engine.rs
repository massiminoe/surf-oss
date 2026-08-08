//! CoreAudio output and the game <-> audio thread boundary.
//!
//! The hard rule: audio never touches the simulation. The game thread pushes a
//! plain parameter block and a short event queue across; nothing flows back.
//! A late or dropped audio buffer therefore cannot perturb a run, and replays
//! stay bit-identical whether or not the device is present.

use std::collections::VecDeque;
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::{Arc, Mutex};

use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};

use crate::voice::{AudioEvent, Levels, MaglevVoice, Params, WipeStyle};

/// Beyond this the game is producing events faster than they could ever be
/// heard; drop the oldest so a stall can't grow the queue without bound.
const MAX_QUEUED_EVENTS: usize = 64;

#[inline]
fn pack(v: f32) -> u32 {
    v.to_bits()
}

#[inline]
fn unpack(v: u32) -> f32 {
    let f = f32::from_bits(v);
    if f.is_finite() {
        f
    } else {
        0.0
    }
}

struct Shared {
    speed: AtomicU32,
    sync: AtomicU32,
    contact: AtomicU32,
    master: AtomicU32,
    core: AtomicU32,
    air: AtomicU32,
    sub: AtomicU32,
    enabled: AtomicU32,
    wipe_style: AtomicU32,
    events: Mutex<VecDeque<AudioEvent>>,
}

impl Shared {
    fn new(levels: Levels, enabled: bool, style: WipeStyle) -> Self {
        Self {
            speed: AtomicU32::new(pack(0.0)),
            sync: AtomicU32::new(pack(0.0)),
            contact: AtomicU32::new(pack(0.0)),
            master: AtomicU32::new(pack(levels.master)),
            core: AtomicU32::new(pack(levels.core)),
            air: AtomicU32::new(pack(levels.air)),
            sub: AtomicU32::new(pack(levels.sub)),
            enabled: AtomicU32::new(u32::from(enabled)),
            wipe_style: AtomicU32::new(u32::from(style == WipeStyle::Dissolve)),
            events: Mutex::new(VecDeque::with_capacity(MAX_QUEUED_EVENTS)),
        }
    }

    fn params(&self) -> Params {
        Params {
            speed: unpack(self.speed.load(Ordering::Relaxed)),
            sync: unpack(self.sync.load(Ordering::Relaxed)),
            contact: unpack(self.contact.load(Ordering::Relaxed)),
        }
    }

    fn levels(&self) -> Levels {
        Levels {
            master: unpack(self.master.load(Ordering::Relaxed)),
            core: unpack(self.core.load(Ordering::Relaxed)),
            air: unpack(self.air.load(Ordering::Relaxed)),
            sub: unpack(self.sub.load(Ordering::Relaxed)),
        }
    }
}

/// Owns the output stream. Not `Send` — `cpal::Stream` isn't, and this lives on
/// the winit main thread alongside the rest of the app.
pub struct AudioEngine {
    shared: Arc<Shared>,
    sample_rate: u32,
    channels: u16,
    _stream: cpal::Stream,
}

impl AudioEngine {
    pub fn new(levels: Levels, enabled: bool, style: WipeStyle) -> Result<Self, String> {
        let host = cpal::default_host();
        let device = host
            .default_output_device()
            .ok_or_else(|| "no default output device".to_string())?;
        let supported = device
            .default_output_config()
            .map_err(|e| format!("default output config: {e}"))?;

        let sample_format = supported.sample_format();
        if sample_format != cpal::SampleFormat::F32 {
            // CoreAudio is f32 in practice; anything else is out of scope
            // rather than silently mis-rendered.
            return Err(format!("unsupported sample format {sample_format:?}"));
        }

        let config: cpal::StreamConfig = supported.config();
        let sample_rate = config.sample_rate;
        let channels = config.channels;

        let shared = Arc::new(Shared::new(levels, enabled, style));
        let cb_shared = Arc::clone(&shared);
        let mut voice = MaglevVoice::new(sample_rate as f32);
        let n_channels = channels as usize;

        let stream = device
            .build_output_stream(
                config,
                move |data: &mut [f32], _: &cpal::OutputCallbackInfo| {
                    // try_lock, never lock: the audio thread must not block on
                    // the game thread. On contention the events stay queued and
                    // arrive one buffer later (~5 ms) instead of being lost.
                    if let Ok(mut q) = cb_shared.events.try_lock() {
                        while let Some(ev) = q.pop_front() {
                            voice.trigger(ev);
                        }
                    }
                    voice.set_params(cb_shared.params());
                    voice.set_levels(cb_shared.levels());
                    voice.set_enabled(cb_shared.enabled.load(Ordering::Relaxed) != 0);
                    voice.set_wipe_style(
                        if cb_shared.wipe_style.load(Ordering::Relaxed) != 0 {
                            WipeStyle::Dissolve
                        } else {
                            WipeStyle::Rewind
                        },
                    );
                    voice.render(data, n_channels);
                },
                move |err| eprintln!("audio stream error: {err}"),
                None,
            )
            .map_err(|e| format!("build output stream: {e}"))?;

        stream.play().map_err(|e| format!("stream play: {e}"))?;

        Ok(Self {
            shared,
            sample_rate,
            channels,
            _stream: stream,
        })
    }

    pub fn sample_rate(&self) -> u32 {
        self.sample_rate
    }

    pub fn channels(&self) -> u16 {
        self.channels
    }

    pub fn set_params(&self, p: Params) {
        self.shared.speed.store(pack(p.speed), Ordering::Relaxed);
        self.shared.sync.store(pack(p.sync), Ordering::Relaxed);
        self.shared.contact.store(pack(p.contact), Ordering::Relaxed);
    }

    pub fn set_levels(&self, l: Levels) {
        self.shared.master.store(pack(l.master), Ordering::Relaxed);
        self.shared.core.store(pack(l.core), Ordering::Relaxed);
        self.shared.air.store(pack(l.air), Ordering::Relaxed);
        self.shared.sub.store(pack(l.sub), Ordering::Relaxed);
    }

    pub fn set_enabled(&self, on: bool) {
        self.shared.enabled.store(u32::from(on), Ordering::Relaxed);
    }

    pub fn set_wipe_style(&self, s: WipeStyle) {
        self.shared
            .wipe_style
            .store(u32::from(s == WipeStyle::Dissolve), Ordering::Relaxed);
    }

    pub fn push(&self, ev: AudioEvent) {
        if let Ok(mut q) = self.shared.events.lock() {
            if q.len() >= MAX_QUEUED_EVENTS {
                q.pop_front();
            }
            q.push_back(ev);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn f32_round_trips_through_the_atomic_param_block() {
        for v in [0.0f32, 1.0, -3.5, 2200.0, f32::MIN_POSITIVE] {
            assert_eq!(unpack(pack(v)), v);
        }
    }

    #[test]
    fn non_finite_params_are_neutralised() {
        assert_eq!(unpack(pack(f32::NAN)), 0.0);
        assert_eq!(unpack(pack(f32::INFINITY)), 0.0);
    }

    #[test]
    fn shared_defaults_and_accessors_agree() {
        let levels = Levels {
            master: 0.5,
            core: 0.9,
            air: 0.3,
            sub: 1.2,
        };
        let s = Shared::new(levels, true, WipeStyle::Dissolve);
        let got = s.levels();
        assert_eq!(got.master, 0.5);
        assert_eq!(got.core, 0.9);
        assert_eq!(got.air, 0.3);
        assert_eq!(got.sub, 1.2);
        assert_eq!(s.enabled.load(Ordering::Relaxed), 1);
        assert_eq!(s.wipe_style.load(Ordering::Relaxed), 1);
    }

    #[test]
    fn event_queue_drops_oldest_when_saturated() {
        let s = Shared::new(Levels::default(), true, WipeStyle::Rewind);
        let mut q = s.events.lock().unwrap();
        for i in 0..(MAX_QUEUED_EVENTS + 10) {
            if q.len() >= MAX_QUEUED_EVENTS {
                q.pop_front();
            }
            q.push_back(AudioEvent::Land {
                step: (i % 8) as u8,
                speed: 500.0,
            });
        }
        assert_eq!(q.len(), MAX_QUEUED_EVENTS);
    }
}
