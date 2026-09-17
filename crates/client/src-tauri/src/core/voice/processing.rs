//! Capture processing: echo cancellation, noise suppression and automatic
//! gain, see `specs/0013-voice-settings.md`.

use std::sync::mpsc;
use std::thread;

use aec3::nodes::audio::AudioFormat;
use aec3::pipelines::linear::{self, LinearPipeline};
use nnnoiseless::DenoiseState;

use super::media::{Frame, FRAME, SAMPLE_RATE};
use super::settings::{NoiseSuppression, VoiceSettings};

/// aec3 and RNNoise work on 10 ms, the rest of the pipeline on 20 ms.
const CHUNK: usize = FRAME / 2;
/// RNNoise expects samples scaled like 16-bit PCM.
const PCM_SCALE: f32 = 32_768.0;
/// Frames waiting for the DSP thread; beyond this, capture frames are dropped.
const QUEUE_FRAMES: usize = 64;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ProcessingConfig {
    pub echo_cancellation: bool,
    pub noise_suppression: NoiseSuppression,
    pub auto_gain: bool,
}

impl From<&VoiceSettings> for ProcessingConfig {
    fn from(settings: &VoiceSettings) -> Self {
        Self {
            echo_cancellation: settings.echo_cancellation,
            noise_suppression: settings.noise_suppression,
            auto_gain: settings.auto_gain,
        }
    }
}

/// The capture chain for one microphone. Not `Send`: the aec3 graph is built
/// on `Rc`, so it lives on the thread started by [`Dsp::spawn`].
pub struct Processor {
    config: ProcessingConfig,
    pipeline: LinearPipeline,
    rnnoise: Option<Box<DenoiseState<'static>>>,
    scratch: [f32; CHUNK],
}

impl Processor {
    pub fn new(config: ProcessingConfig) -> Self {
        let format = AudioFormat::ten_ms(SAMPLE_RATE, 1);
        let pipeline = linear::builder(format, format)
            .enable_noise_suppression(config.noise_suppression == NoiseSuppression::Standard)
            .enable_gain_controller2(config.auto_gain)
            .build()
            .expect("10 ms 48 kHz mono is a valid aec3 pipeline");
        let rnnoise =
            (config.noise_suppression == NoiseSuppression::Strong).then(DenoiseState::new);
        Self {
            config,
            pipeline,
            rnnoise,
            scratch: [0.0; CHUNK],
        }
    }

    pub fn config(&self) -> ProcessingConfig {
        self.config
    }

    /// Feeds what went to the speaker as the echo reference.
    pub fn render(&mut self, frame: &Frame) {
        if !self.config.echo_cancellation {
            return;
        }
        for chunk in frame.as_chunks::<CHUNK>().0 {
            let _ = self.pipeline.handle_render_frame(chunk);
        }
    }

    /// Processes one microphone frame; errors pass the audio through.
    pub fn capture(&mut self, frame: &Frame) -> Frame {
        let mut out = [0.0; FRAME];
        let outputs = out.as_chunks_mut::<CHUNK>().0;
        for (input, output) in frame.as_chunks::<CHUNK>().0.iter().zip(outputs) {
            if self
                .pipeline
                .process_capture_frame(input, &mut self.scratch)
                .is_err()
            {
                self.scratch.copy_from_slice(input);
            }
            match self.rnnoise.as_mut() {
                Some(rnnoise) => {
                    for sample in &mut self.scratch {
                        *sample *= PCM_SCALE;
                    }
                    rnnoise.process_frame(output, &self.scratch);
                    for sample in output.iter_mut() {
                        *sample /= PCM_SCALE;
                    }
                }
                None => output.copy_from_slice(&self.scratch),
            }
        }
        out
    }
}

pub enum DspInput {
    Capture(Frame),
    Render(Frame),
    Configure(ProcessingConfig),
}

/// Runs a [`Processor`] on its own thread so neither the audio callbacks nor
/// the async runtime pay for it. The thread exits once every sender is gone.
pub struct Dsp {
    inputs: mpsc::SyncSender<DspInput>,
}

impl Dsp {
    pub fn spawn(
        config: ProcessingConfig,
        mut output: impl FnMut(&Frame) + Send + 'static,
    ) -> Self {
        let (inputs, rx) = mpsc::sync_channel(QUEUE_FRAMES);
        thread::Builder::new()
            .name("aster-dsp".into())
            .spawn(move || {
                let mut processor = Processor::new(config);
                while let Ok(input) = rx.recv() {
                    match input {
                        DspInput::Capture(frame) => output(&processor.capture(&frame)),
                        DspInput::Render(frame) => processor.render(&frame),
                        DspInput::Configure(config) if config != processor.config() => {
                            // AEC re-converges within seconds.
                            processor = Processor::new(config);
                        }
                        DspInput::Configure(_) => {}
                    }
                }
            })
            .expect("spawning the DSP thread");
        Self { inputs }
    }

    /// For audio callbacks: never blocks, drops the frame when the queue is full.
    pub fn sender(&self) -> mpsc::SyncSender<DspInput> {
        self.inputs.clone()
    }

    pub fn configure(&self, config: ProcessingConfig) {
        let _ = self.inputs.send(DspInput::Configure(config));
    }
}

#[cfg(test)]
mod tests {
    use super::super::media::signal::Sine;
    use super::super::media::{level_dbfs, SILENCE};
    use super::*;

    const ALL_ON: ProcessingConfig = ProcessingConfig {
        echo_cancellation: true,
        noise_suppression: NoiseSuppression::Standard,
        auto_gain: false,
    };

    /// Speech-like test voice: a gliding tone in 250 ms syllables.
    struct Voice {
        freq: f32,
        frame: usize,
        phase: f32,
    }

    impl Voice {
        fn new(freq: f32) -> Self {
            Self {
                freq,
                frame: 0,
                phase: 0.0,
            }
        }

        fn next_frame(&mut self) -> Frame {
            let t = self.frame as f32 * 0.02;
            self.frame += 1;
            let freq = self.freq * (1.0 + 0.3 * (t * 3.0).sin());
            let envelope = 0.15 + 0.15 * (t * std::f32::consts::TAU * 4.0).sin().abs();
            let step = std::f32::consts::TAU * freq / SAMPLE_RATE as f32;
            std::array::from_fn(|_| {
                self.phase = (self.phase + step) % std::f32::consts::TAU;
                envelope * self.phase.sin()
            })
        }
    }

    fn noise(seed: &mut u32, amplitude: f32) -> Frame {
        std::array::from_fn(|_| {
            *seed ^= *seed << 13;
            *seed ^= *seed >> 17;
            *seed ^= *seed << 5;
            amplitude * ((*seed as f32 / u32::MAX as f32) * 2.0 - 1.0)
        })
    }

    fn add(a: &Frame, b: &Frame) -> Frame {
        std::array::from_fn(|i| a[i] + b[i])
    }

    /// Mean level of the last `frames` of a run.
    fn tail_level(frames: &[Frame], count: usize) -> f32 {
        let tail: Vec<f32> = frames[frames.len() - count..].concat();
        level_dbfs(&tail)
    }

    /// A narrowband tone is a poor echo reference: AEC3 only converges at the
    /// frequencies it has heard. Speech is broadband, so the far end here is
    /// noise shaped into syllables.
    fn babble(seed: &mut u32, n: usize) -> Frame {
        let t = n as f32 * 0.02;
        let envelope = 0.15 + 0.15 * (t * std::f32::consts::TAU * 4.0).sin().abs();
        noise(seed, envelope)
    }

    #[test]
    fn echo_is_cancelled() {
        let mut processor = Processor::new(ALL_ON);
        let mut seed = 3;
        let mut played = Vec::new();
        let (mut mic, mut out) = (Vec::new(), Vec::new());
        for n in 0..400usize {
            let render = babble(&mut seed, n);
            played.push(render);
            processor.render(&render);
            // The microphone hears the speaker 60 ms later at -6 dB.
            let echo = match n.checked_sub(3) {
                Some(i) => played[i].map(|s| s * 0.5),
                None => SILENCE,
            };
            mic.push(echo);
            out.push(processor.capture(&echo));
        }
        let (before, after) = (tail_level(&mic, 100), tail_level(&out, 100));
        assert!(after < before - 30.0, "echo {before} -> {after} dBFS");
    }

    #[test]
    fn near_voice_survives() {
        let mut processor = Processor::new(ALL_ON);
        let mut near = Voice::new(500.0);
        let mut seed = 7;
        let (mut mic, mut out) = (Vec::new(), Vec::new());
        for _ in 0..300 {
            processor.render(&SILENCE);
            let frame = add(&near.next_frame(), &noise(&mut seed, 0.005));
            mic.push(frame);
            out.push(processor.capture(&frame));
        }
        let (before, after) = (tail_level(&mic, 100), tail_level(&out, 100));
        assert!(
            (before - after).abs() < 6.0,
            "voice {before} -> {after} dBFS"
        );
    }

    fn suppressed_noise(noise_suppression: NoiseSuppression) -> f32 {
        let mut processor = Processor::new(ProcessingConfig {
            noise_suppression,
            ..ALL_ON
        });
        let mut seed = 1;
        let (mut mic, mut out) = (Vec::new(), Vec::new());
        for _ in 0..300 {
            let frame = noise(&mut seed, 0.05);
            mic.push(frame);
            out.push(processor.capture(&frame));
        }
        tail_level(&out, 100) - tail_level(&mic, 100)
    }

    #[test]
    fn noise_is_suppressed_by_both_modes() {
        let off = suppressed_noise(NoiseSuppression::Off);
        let standard = suppressed_noise(NoiseSuppression::Standard);
        let strong = suppressed_noise(NoiseSuppression::Strong);
        assert!(off.abs() < 3.0, "off changed noise by {off} dB");
        assert!(standard < -8.0, "standard: {standard} dB");
        assert!(strong < -15.0, "strong: {strong} dB");
    }

    #[test]
    fn dsp_thread_processes_and_reconfigures() {
        let (tx, rx) = mpsc::channel();
        let dsp = Dsp::spawn(ALL_ON, move |frame: &Frame| {
            let _ = tx.send(level_dbfs(frame));
        });
        let mut tone = Sine::new(440.0, 0.3);
        let sender = dsp.sender();
        sender.send(DspInput::Capture(tone.next_frame())).unwrap();
        dsp.configure(ProcessingConfig {
            noise_suppression: NoiseSuppression::Strong,
            ..ALL_ON
        });
        sender.send(DspInput::Capture(tone.next_frame())).unwrap();
        let timeout = std::time::Duration::from_secs(2);
        rx.recv_timeout(timeout).unwrap();
        rx.recv_timeout(timeout).unwrap();
    }
}
