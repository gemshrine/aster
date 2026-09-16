//! Attention signals (spec 0012).
//!
//! Everything is synthesised with a couple of oscillators, so the bundle
//! carries no audio files. Browsers refuse to start an `AudioContext` before
//! the user has interacted with the page, so the context is created lazily and
//! resumed on every play attempt.

use std::cell::RefCell;

use wasm_bindgen::prelude::*;
use web_sys::{AudioContext, GainNode, OscillatorType};

thread_local! {
    static CONTEXT: RefCell<Option<AudioContext>> = const { RefCell::new(None) };
}

/// One beep: `freq` Hz for `len` seconds, starting `at` seconds from now.
struct Beep {
    freq: f32,
    at: f64,
    len: f64,
    gain: f32,
}

/// The signals the app can play, each a short pattern of beeps.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Signal {
    /// Incoming call — the loudest and longest, repeated while it rings.
    Ring,
    /// Outgoing call, waiting for the peer to pick up.
    Dial,
    /// Media is up.
    Connected,
    /// The call is over.
    Ended,
    /// A message arrived while the window was in the background.
    Message,
}

impl Signal {
    fn beeps(self) -> &'static [Beep] {
        match self {
            Self::Ring => &[
                Beep {
                    freq: 587.0,
                    at: 0.0,
                    len: 0.34,
                    gain: 0.16,
                },
                Beep {
                    freq: 440.0,
                    at: 0.40,
                    len: 0.34,
                    gain: 0.16,
                },
            ],
            Self::Dial => &[Beep {
                freq: 420.0,
                at: 0.0,
                len: 0.5,
                gain: 0.07,
            }],
            Self::Connected => &[
                Beep {
                    freq: 587.0,
                    at: 0.0,
                    len: 0.09,
                    gain: 0.10,
                },
                Beep {
                    freq: 880.0,
                    at: 0.10,
                    len: 0.14,
                    gain: 0.10,
                },
            ],
            Self::Ended => &[
                Beep {
                    freq: 587.0,
                    at: 0.0,
                    len: 0.09,
                    gain: 0.09,
                },
                Beep {
                    freq: 392.0,
                    at: 0.10,
                    len: 0.18,
                    gain: 0.09,
                },
            ],
            Self::Message => &[Beep {
                freq: 784.0,
                at: 0.0,
                len: 0.10,
                gain: 0.09,
            }],
        }
    }

    /// How long the whole pattern lasts, in milliseconds — the repeat period
    /// of a looping signal is derived from it.
    pub fn len_ms(self) -> f64 {
        self.beeps()
            .iter()
            .map(|beep| (beep.at + beep.len) * 1000.0)
            .fold(0.0, f64::max)
    }
}

/// Plays one pattern. Failures are silent on purpose: no sound device, a
/// context the browser refuses to start — none of it is worth an error in the
/// user's face, and the call itself is unaffected.
pub fn play(signal: Signal) {
    let _ = try_play(signal);
}

fn try_play(signal: Signal) -> Result<(), JsValue> {
    CONTEXT.with(|slot| {
        let mut slot = slot.borrow_mut();
        let ctx = match slot.as_ref() {
            Some(ctx) => ctx,
            None => {
                *slot = Some(AudioContext::new()?);
                slot.as_ref().expect("just stored")
            }
        };
        // A context created before the first click starts suspended.
        let _ = ctx.resume();

        let now = ctx.current_time();
        for beep in signal.beeps() {
            let osc = ctx.create_oscillator()?;
            let gain: GainNode = ctx.create_gain()?;
            osc.set_type(OscillatorType::Sine);
            osc.frequency().set_value(beep.freq);
            osc.connect_with_audio_node(&gain)?;
            gain.connect_with_audio_node(&ctx.destination())?;

            // A short ramp at each end: a square-edged beep clicks.
            let start = now + beep.at;
            let end = start + beep.len;
            let envelope = gain.gain();
            envelope.set_value(0.0);
            envelope.linear_ramp_to_value_at_time(beep.gain, start + 0.012)?;
            envelope.set_value_at_time(beep.gain, end - 0.02)?;
            envelope.linear_ramp_to_value_at_time(0.0, end)?;

            osc.start_with_when(start)?;
            osc.stop_with_when(end + 0.01)?;
        }
        Ok(())
    })
}

/// A signal repeating until it is dropped — the ringtone and the dial tone.
pub struct Loop {
    handle: Option<leptos::leptos_dom::helpers::IntervalHandle>,
}

impl Loop {
    /// Starts `signal` now and repeats it with a gap after each pattern.
    pub fn start(signal: Signal, gap_ms: f64) -> Self {
        play(signal);
        let period = std::time::Duration::from_millis((signal.len_ms() + gap_ms) as u64);
        let handle =
            leptos::leptos_dom::helpers::set_interval_with_handle(move || play(signal), period)
                .ok();
        Self { handle }
    }
}

impl Drop for Loop {
    fn drop(&mut self) {
        if let Some(handle) = self.handle.take() {
            handle.clear();
        }
    }
}
