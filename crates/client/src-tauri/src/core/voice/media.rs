//! Audio pipeline pieces without I/O: framing, VAD, Opus and the jitter
//! buffer. See `specs/0011-voice-core.md`.

use std::collections::VecDeque;

use opus_pure::{Application, OpusDecoder, OpusEncoder, MAX_PACKET_BYTES};

pub const SAMPLE_RATE: u32 = 48_000;
/// 20 ms at 48 kHz, mono.
pub const FRAME: usize = 960;
pub type Frame = [f32; FRAME];

pub const SILENCE: Frame = [0.0; FRAME];

const BITRATE_BPS: i32 = 32_000;
const EXPECTED_LOSS_PERCENT: i32 = 10;

const VAD_THRESHOLD_DBFS: f32 = -45.0;
/// 300 ms of hangover at 20 ms per frame.
const VAD_HANGOVER_FRAMES: u32 = 15;

const JITTER_START_FRAMES: usize = 3;
const JITTER_MAX_FRAMES: usize = 10;
const MAX_CONCEALED_GAP: u16 = 3;

/// Cuts an arbitrary stream of mono samples into 20 ms frames.
#[derive(Default)]
pub struct Framer {
    pending: Vec<f32>,
}

impl Framer {
    pub fn push(&mut self, mut samples: &[f32], mut on_frame: impl FnMut(&Frame)) {
        if !self.pending.is_empty() {
            let take = (FRAME - self.pending.len()).min(samples.len());
            self.pending.extend_from_slice(&samples[..take]);
            samples = &samples[take..];
            if self.pending.len() < FRAME {
                return;
            }
            let frame: Frame = self.pending[..].try_into().expect("exactly one frame");
            on_frame(&frame);
            self.pending.clear();
        }
        let (frames, remainder) = samples.as_chunks::<FRAME>();
        for frame in frames {
            on_frame(frame);
        }
        self.pending.extend_from_slice(remainder);
    }
}

/// Root mean square level of a frame in dBFS, floored at −100.
pub fn level_dbfs(frame: &[f32]) -> f32 {
    let mean_square = frame.iter().map(|s| s * s).sum::<f32>() / frame.len().max(1) as f32;
    if mean_square <= 1e-10 {
        return -100.0;
    }
    (10.0 * mean_square.log10()).max(-100.0)
}

/// Frames per `input-level` report: 10 per second.
const LEVEL_FRAMES: u32 = 5;

/// Averages frame energy into periodic level reports.
#[derive(Default)]
pub struct LevelMeter {
    sum_squares: f32,
    frames: u32,
}

impl LevelMeter {
    /// Returns the level of the last 100 ms once enough frames arrived.
    pub fn push(&mut self, frame: &Frame) -> Option<f32> {
        self.sum_squares += frame.iter().map(|s| s * s).sum::<f32>();
        self.frames += 1;
        if self.frames < LEVEL_FRAMES {
            return None;
        }
        let mean_square = self.sum_squares / (LEVEL_FRAMES as usize * FRAME) as f32;
        *self = Self::default();
        Some(if mean_square <= 1e-10 {
            -100.0
        } else {
            (10.0 * mean_square.log10()).max(-100.0)
        })
    }
}

/// Energy-based voice activity with hangover.
pub struct Vad {
    threshold_dbfs: f32,
    hangover: u32,
}

impl Default for Vad {
    fn default() -> Self {
        Self::new(VAD_THRESHOLD_DBFS)
    }
}

impl Vad {
    pub fn new(threshold_dbfs: f32) -> Self {
        Self {
            threshold_dbfs,
            hangover: 0,
        }
    }

    pub fn set_threshold(&mut self, dbfs: f32) {
        self.threshold_dbfs = dbfs;
    }

    /// Feeds one frame and returns whether someone is speaking.
    pub fn update(&mut self, frame: &Frame) -> bool {
        if level_dbfs(frame) > self.threshold_dbfs {
            self.hangover = VAD_HANGOVER_FRAMES;
            return true;
        }
        if self.hangover > 0 {
            self.hangover -= 1;
            return true;
        }
        false
    }
}

#[derive(Debug, Clone, Copy, Default)]
pub struct GateInput {
    pub push_to_talk: bool,
    /// The push-to-talk key is held in any source.
    pub pressed: bool,
    pub release_delay_ms: u32,
    pub vad_threshold_dbfs: f32,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct GateState {
    pub speaking: bool,
    /// Whether this frame is sent rather than replaced by silence.
    pub open: bool,
}

/// Decides per frame whether the microphone is sent: by voice activity, or
/// by the push-to-talk key plus a hold after release.
#[derive(Default)]
pub struct Gate {
    vad: Vad,
    hold_frames: u32,
}

impl Gate {
    pub fn update(&mut self, frame: &Frame, input: GateInput) -> GateState {
        self.vad.set_threshold(input.vad_threshold_dbfs);
        let speaking = self.vad.update(frame);
        if !input.push_to_talk {
            self.hold_frames = 0;
            return GateState {
                speaking,
                open: speaking,
            };
        }
        let open = if input.pressed {
            let frame_ms = (FRAME * 1000) as u32 / SAMPLE_RATE;
            self.hold_frames = input.release_delay_ms.div_ceil(frame_ms);
            true
        } else if self.hold_frames > 0 {
            self.hold_frames -= 1;
            true
        } else {
            false
        };
        GateState { speaking, open }
    }
}

pub struct Encoder {
    opus: OpusEncoder,
    packet: Vec<u8>,
}

impl Encoder {
    pub fn new() -> Self {
        let mut opus = OpusEncoder::new(SAMPLE_RATE as i32, 1, Application::Voip)
            .expect("48 kHz mono VOIP is a valid Opus configuration");
        opus.bitrate_bps = BITRATE_BPS;
        opus.use_inband_fec = true;
        opus.packet_loss_perc = EXPECTED_LOSS_PERCENT;
        Self {
            opus,
            packet: vec![0; MAX_PACKET_BYTES],
        }
    }

    pub fn encode(&mut self, frame: &Frame) -> Vec<u8> {
        let len = self
            .opus
            .encode(frame, FRAME, &mut self.packet)
            .expect("a full frame always encodes");
        self.packet[..len].to_vec()
    }
}

impl Default for Encoder {
    fn default() -> Self {
        Self::new()
    }
}

pub struct Decoder {
    opus: OpusDecoder,
}

impl Decoder {
    pub fn new() -> Self {
        Self {
            opus: OpusDecoder::new(SAMPLE_RATE as i32, 1)
                .expect("48 kHz mono is a valid Opus configuration"),
        }
    }

    /// Corrupt packets decode as concealment rather than failing the stream.
    pub fn decode(&mut self, packet: &[u8]) -> Frame {
        let mut frame = SILENCE;
        if self.opus.decode(packet, FRAME, &mut frame).is_err() {
            return self.conceal();
        }
        frame
    }

    /// Reconstructs the frame before `next_packet` from its in-band FEC.
    pub fn recover(&mut self, next_packet: &[u8]) -> Frame {
        let mut frame = SILENCE;
        if self
            .opus
            .decode_fec(next_packet, FRAME, &mut frame)
            .is_err()
        {
            return self.conceal();
        }
        frame
    }

    pub fn conceal(&mut self) -> Frame {
        let mut frame = SILENCE;
        let _ = self.opus.decode(&[], FRAME, &mut frame);
        frame
    }
}

impl Default for Decoder {
    fn default() -> Self {
        Self::new()
    }
}

/// Reorders nothing: late packets are dropped, short gaps are concealed.
pub struct JitterBuffer {
    decoder: Decoder,
    frames: VecDeque<Frame>,
    last_seq: Option<u16>,
    playing: bool,
    concealed_underrun: bool,
}

impl JitterBuffer {
    pub fn new() -> Self {
        Self {
            decoder: Decoder::new(),
            frames: VecDeque::with_capacity(JITTER_MAX_FRAMES + 1),
            last_seq: None,
            playing: false,
            concealed_underrun: false,
        }
    }

    pub fn push(&mut self, seq: u16, payload: &[u8]) {
        if let Some(last) = self.last_seq {
            let delta = seq.wrapping_sub(last) as i16;
            if delta <= 0 {
                return;
            }
            let missing = delta as u16 - 1;
            if (1..=MAX_CONCEALED_GAP).contains(&missing) {
                for _ in 1..missing {
                    let frame = self.decoder.conceal();
                    self.enqueue(frame);
                }
                let frame = self.decoder.recover(payload);
                self.enqueue(frame);
            }
        }
        self.last_seq = Some(seq);
        let frame = self.decoder.decode(payload);
        self.enqueue(frame);
    }

    /// Next 20 ms for the speaker.
    pub fn pop(&mut self) -> Frame {
        if !self.playing {
            if self.frames.len() < JITTER_START_FRAMES {
                return SILENCE;
            }
            self.playing = true;
            self.concealed_underrun = false;
        }
        match self.frames.pop_front() {
            Some(frame) => frame,
            None => {
                self.playing = false;
                if self.concealed_underrun {
                    SILENCE
                } else {
                    self.concealed_underrun = true;
                    self.decoder.conceal()
                }
            }
        }
    }

    pub fn buffered(&self) -> usize {
        self.frames.len()
    }

    fn enqueue(&mut self, frame: Frame) {
        if self.frames.len() == JITTER_MAX_FRAMES {
            self.frames.pop_front();
        }
        self.frames.push_back(frame);
    }
}

impl Default for JitterBuffer {
    fn default() -> Self {
        Self::new()
    }
}

/// Test signal and analysis helpers, shared with integration tests.
pub mod signal {
    use super::{Frame, SAMPLE_RATE};

    /// Continuous sine generator.
    pub struct Sine {
        freq: f32,
        amplitude: f32,
        phase: f32,
    }

    impl Sine {
        pub fn new(freq: f32, amplitude: f32) -> Self {
            Self {
                freq,
                amplitude,
                phase: 0.0,
            }
        }

        pub fn next_frame(&mut self) -> Frame {
            let step = std::f32::consts::TAU * self.freq / SAMPLE_RATE as f32;
            std::array::from_fn(|_| {
                let sample = self.amplitude * self.phase.sin();
                self.phase = (self.phase + step) % std::f32::consts::TAU;
                sample
            })
        }
    }

    /// Relative power at `freq` (Goertzel), normalised by frame energy.
    pub fn tone_ratio(samples: &[f32], freq: f32) -> f32 {
        let omega = std::f32::consts::TAU * freq / SAMPLE_RATE as f32;
        let coeff = 2.0 * omega.cos();
        let (mut s1, mut s2) = (0.0f32, 0.0f32);
        for &x in samples {
            let s = x + coeff * s1 - s2;
            s2 = s1;
            s1 = s;
        }
        let power = s1 * s1 + s2 * s2 - coeff * s1 * s2;
        let energy: f32 = samples.iter().map(|s| s * s).sum();
        if energy <= 1e-9 {
            return 0.0;
        }
        power / (energy * samples.len() as f32 / 2.0)
    }
}

#[cfg(test)]
mod tests {
    use super::signal::{tone_ratio, Sine};
    use super::*;

    #[test]
    fn framer_handles_arbitrary_chunks() {
        let mut framer = Framer::default();
        let samples: Vec<f32> = (0..FRAME * 3 + 17).map(|i| i as f32).collect();
        let mut frames = Vec::new();
        for chunk in samples.chunks(333) {
            framer.push(chunk, |frame| frames.push(*frame));
        }
        assert_eq!(frames.len(), 3);
        for (n, frame) in frames.iter().enumerate() {
            assert_eq!(frame[0], (n * FRAME) as f32);
            assert_eq!(frame[FRAME - 1], ((n + 1) * FRAME - 1) as f32);
        }
    }

    #[test]
    fn level_of_silence_and_full_scale_sine() {
        assert_eq!(level_dbfs(&SILENCE), -100.0);
        let level = level_dbfs(&Sine::new(440.0, 1.0).next_frame());
        assert!((level + 3.0).abs() < 0.5, "{level}");
    }

    #[test]
    fn level_meter_reports_every_100ms() {
        let mut meter = LevelMeter::default();
        let mut sine = Sine::new(440.0, 1.0);
        for _ in 0..LEVEL_FRAMES - 1 {
            assert_eq!(meter.push(&sine.next_frame()), None);
        }
        let level = meter.push(&sine.next_frame()).unwrap();
        assert!((level + 3.0).abs() < 0.5, "{level}");
        for _ in 0..LEVEL_FRAMES - 1 {
            assert_eq!(meter.push(&SILENCE), None);
        }
        assert_eq!(meter.push(&SILENCE), Some(-100.0));
    }

    #[test]
    fn vad_holds_speech_for_hangover() {
        let mut vad = Vad::default();
        let mut speech = Sine::new(300.0, 0.1);
        assert!(!vad.update(&SILENCE));
        assert!(vad.update(&speech.next_frame()));
        for _ in 0..VAD_HANGOVER_FRAMES {
            assert!(vad.update(&SILENCE));
        }
        assert!(!vad.update(&SILENCE));

        let whisper = Sine::new(300.0, 0.001).next_frame();
        assert!(!vad.update(&whisper), "{} dBFS", level_dbfs(&whisper));

        let mut sensitive = Vad::new(-70.0);
        assert!(sensitive.update(&whisper));
        sensitive.set_threshold(-20.0);
        for _ in 0..=VAD_HANGOVER_FRAMES {
            sensitive.update(&SILENCE);
        }
        assert!(
            !sensitive.update(&speech.next_frame()),
            "-23 dBFS is below -20"
        );
    }

    #[test]
    fn gate_follows_voice_activity_or_the_key() {
        let mut gate = Gate::default();
        let speech = Sine::new(300.0, 0.1).next_frame();
        let vad = GateInput {
            vad_threshold_dbfs: -45.0,
            ..GateInput::default()
        };
        assert!(gate.update(&speech, vad).open);

        let mut gate = Gate::default();
        let ptt = GateInput {
            push_to_talk: true,
            release_delay_ms: 50,
            ..vad
        };
        let released = gate.update(&speech, ptt);
        assert_eq!(
            released,
            GateState {
                speaking: true,
                open: false
            }
        );
        let pressed = GateInput {
            pressed: true,
            ..ptt
        };
        assert!(
            gate.update(&SILENCE, pressed).open,
            "quiet speech still goes"
        );
        // 50 ms rounds up to three 20 ms frames after release.
        for _ in 0..3 {
            assert!(gate.update(&SILENCE, ptt).open);
        }
        assert!(!gate.update(&SILENCE, ptt).open);
    }

    #[test]
    fn opus_round_trip_keeps_tone_and_level() {
        let mut sine = Sine::new(440.0, 0.3);
        let mut encoder = Encoder::new();
        let mut decoder = Decoder::new();
        let mut input = Vec::new();
        let mut output = Vec::new();
        for _ in 0..50 {
            let frame = sine.next_frame();
            let packet = encoder.encode(&frame);
            assert!(
                packet.len() < 200,
                "32 kbps packet is {} bytes",
                packet.len()
            );
            input.extend_from_slice(&frame);
            output.extend_from_slice(&decoder.decode(&packet));
        }
        let tail = &output[output.len() - FRAME * 10..];
        let level_in = level_dbfs(&input[..FRAME * 10]);
        let level_out = level_dbfs(tail);
        assert!(
            (level_in - level_out).abs() < 3.0,
            "{level_in} vs {level_out}"
        );
        assert!(tone_ratio(tail, 440.0) > 0.8, "{}", tone_ratio(tail, 440.0));
        assert!(tone_ratio(tail, 1000.0) < 0.1);
    }

    fn packets(count: usize) -> Vec<Vec<u8>> {
        let mut sine = Sine::new(440.0, 0.3);
        let mut encoder = Encoder::new();
        (0..count)
            .map(|_| encoder.encode(&sine.next_frame()))
            .collect()
    }

    #[test]
    fn jitter_buffer_starts_after_three_frames() {
        let packets = packets(4);
        let mut jitter = JitterBuffer::new();
        jitter.push(10, &packets[0]);
        jitter.push(11, &packets[1]);
        assert_eq!(jitter.pop(), SILENCE);
        jitter.push(12, &packets[2]);
        assert_ne!(jitter.pop(), SILENCE);
        assert_eq!(jitter.buffered(), 2);
    }

    #[test]
    fn jitter_buffer_conceals_short_gaps_and_drops_late_packets() {
        let packets = packets(8);
        let mut jitter = JitterBuffer::new();
        jitter.push(u16::MAX, &packets[0]);
        jitter.push(1, &packets[1]); // 0 lost across the wrap
        assert_eq!(jitter.buffered(), 3);
        jitter.push(0, &packets[2]); // late
        jitter.push(1, &packets[2]); // duplicate
        assert_eq!(jitter.buffered(), 3);
        jitter.push(100, &packets[3]); // long gap: no concealment
        assert_eq!(jitter.buffered(), 4);
    }

    #[test]
    fn jitter_buffer_caps_latency_and_handles_underrun() {
        let packets = packets(15);
        let mut jitter = JitterBuffer::new();
        for (seq, packet) in packets.iter().enumerate() {
            jitter.push(seq as u16, packet);
        }
        assert_eq!(jitter.buffered(), JITTER_MAX_FRAMES);
        for _ in 0..JITTER_MAX_FRAMES {
            jitter.pop();
        }
        let concealed = jitter.pop();
        assert!(
            level_dbfs(&concealed) > -100.0,
            "first underrun is concealed"
        );
        assert_eq!(jitter.pop(), SILENCE, "then silence until refilled");
    }
}
