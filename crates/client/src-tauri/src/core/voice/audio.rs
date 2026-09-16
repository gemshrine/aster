//! Audio device abstraction, see `specs/0011-voice-core.md`.

use super::media::Frame;

pub type CaptureSink = Box<dyn FnMut(&Frame) + Send>;
pub type PlaybackSource = Box<dyn FnMut() -> Frame + Send>;

/// Keeps a capture or playback stream alive; dropping it stops the stream.
pub trait AudioStream: Send + Sync {}

/// Delivers 48 kHz mono 20 ms frames to and from the sound system.
pub trait AudioBackend: Send + Sync {
    fn capture(&self, sink: CaptureSink) -> Result<Box<dyn AudioStream>, String>;
    fn playback(&self, source: PlaybackSource) -> Result<Box<dyn AudioStream>, String>;
}

/// No sound at all: builds without audio devices (`audio-device` off).
pub struct NullBackend;

struct NullStream;
impl AudioStream for NullStream {}

impl AudioBackend for NullBackend {
    fn capture(&self, _sink: CaptureSink) -> Result<Box<dyn AudioStream>, String> {
        Ok(Box::new(NullStream))
    }

    fn playback(&self, _source: PlaybackSource) -> Result<Box<dyn AudioStream>, String> {
        Ok(Box::new(NullStream))
    }
}

/// Linear-interpolation resampler for a continuous mono stream.
pub struct Resampler {
    /// Input samples per output sample.
    ratio: f64,
    /// Position of the next output sample between `last` (0) and the next input (1).
    frac: f64,
    last: f32,
}

impl Resampler {
    pub fn new(from_rate: u32, to_rate: u32) -> Self {
        Self {
            ratio: from_rate as f64 / to_rate as f64,
            frac: 0.0,
            last: 0.0,
        }
    }

    pub fn is_identity(&self) -> bool {
        self.ratio == 1.0
    }

    pub fn process(&mut self, input: &[f32], out: &mut Vec<f32>) {
        for &sample in input {
            while self.frac < 1.0 {
                out.push(self.last + (sample - self.last) * self.frac as f32);
                self.frac += self.ratio;
            }
            self.frac -= 1.0;
            self.last = sample;
        }
    }
}

#[cfg(feature = "audio-device")]
pub use device::CpalBackend;

#[cfg(feature = "audio-device")]
mod device {
    use std::collections::VecDeque;
    use std::sync::mpsc;
    use std::thread;

    use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
    use cpal::{
        FromSample, Sample, SampleFormat, SizedSample, StreamConfig, SupportedStreamConfig,
    };

    use super::super::media::{Framer, FRAME, SAMPLE_RATE};
    use super::{AudioBackend, AudioStream, CaptureSink, PlaybackSource, Resampler};

    pub struct CpalBackend;

    /// cpal streams are not `Send` everywhere, so each one lives on its own
    /// thread until the handle is dropped.
    struct ThreadStream {
        stop: Option<mpsc::Sender<()>>,
        thread: Option<thread::JoinHandle<()>>,
    }

    impl AudioStream for ThreadStream {}

    impl Drop for ThreadStream {
        fn drop(&mut self) {
            drop(self.stop.take());
            if let Some(thread) = self.thread.take() {
                let _ = thread.join();
            }
        }
    }

    fn spawn_stream(
        name: &'static str,
        build: impl FnOnce() -> Result<cpal::Stream, String> + Send + 'static,
    ) -> Result<Box<dyn AudioStream>, String> {
        let (ready_tx, ready_rx) = mpsc::channel();
        let (stop_tx, stop_rx) = mpsc::channel::<()>();
        let thread = thread::Builder::new()
            .name(format!("aster-{name}"))
            .spawn(move || {
                let stream = match build().and_then(|stream| {
                    stream.play().map_err(|err| err.to_string())?;
                    Ok(stream)
                }) {
                    Ok(stream) => stream,
                    Err(err) => {
                        let _ = ready_tx.send(Err(err));
                        return;
                    }
                };
                let _ = ready_tx.send(Ok(()));
                // Returns once the handle drops its sender.
                let _ = stop_rx.recv();
                drop(stream);
            })
            .map_err(|err| err.to_string())?;
        match ready_rx.recv() {
            Ok(Ok(())) => Ok(Box::new(ThreadStream {
                stop: Some(stop_tx),
                thread: Some(thread),
            })),
            Ok(Err(err)) => {
                let _ = thread.join();
                Err(err)
            }
            Err(_) => Err(format!("{name} thread exited")),
        }
    }

    /// Prefers 48 kHz so no resampling is needed.
    fn pick_config(
        default: SupportedStreamConfig,
        supported: impl Iterator<Item = cpal::SupportedStreamConfigRange>,
    ) -> SupportedStreamConfig {
        supported
            .filter(|range| range.sample_format() == default.sample_format())
            .find_map(|range| range.try_with_sample_rate(SAMPLE_RATE))
            .unwrap_or(default)
    }

    impl AudioBackend for CpalBackend {
        fn capture(&self, sink: CaptureSink) -> Result<Box<dyn AudioStream>, String> {
            spawn_stream("capture", move || {
                let device = cpal::default_host()
                    .default_input_device()
                    .ok_or("no input device")?;
                let default = device.default_input_config().map_err(|e| e.to_string())?;
                let config = match device.supported_input_configs() {
                    Ok(supported) => pick_config(default, supported),
                    Err(_) => default,
                };
                match config.sample_format() {
                    SampleFormat::F32 => build_input::<f32>(&device, config.config(), sink),
                    SampleFormat::I16 => build_input::<i16>(&device, config.config(), sink),
                    SampleFormat::I32 => build_input::<i32>(&device, config.config(), sink),
                    SampleFormat::U16 => build_input::<u16>(&device, config.config(), sink),
                    other => Err(format!("unsupported input sample format {other}")),
                }
            })
        }

        fn playback(&self, source: PlaybackSource) -> Result<Box<dyn AudioStream>, String> {
            spawn_stream("playback", move || {
                let device = cpal::default_host()
                    .default_output_device()
                    .ok_or("no output device")?;
                let default = device.default_output_config().map_err(|e| e.to_string())?;
                let config = match device.supported_output_configs() {
                    Ok(supported) => pick_config(default, supported),
                    Err(_) => default,
                };
                match config.sample_format() {
                    SampleFormat::F32 => build_output::<f32>(&device, config.config(), source),
                    SampleFormat::I16 => build_output::<i16>(&device, config.config(), source),
                    SampleFormat::I32 => build_output::<i32>(&device, config.config(), source),
                    SampleFormat::U16 => build_output::<u16>(&device, config.config(), source),
                    other => Err(format!("unsupported output sample format {other}")),
                }
            })
        }
    }

    fn build_input<T>(
        device: &cpal::Device,
        config: StreamConfig,
        mut sink: CaptureSink,
    ) -> Result<cpal::Stream, String>
    where
        T: SizedSample,
        f32: FromSample<T>,
    {
        let channels = config.channels.max(1) as usize;
        let mut resampler = Resampler::new(config.sample_rate, SAMPLE_RATE);
        let mut framer = Framer::default();
        let mut mono = Vec::new();
        let mut resampled = Vec::new();
        device
            .build_input_stream(
                config,
                move |data: &[T], _| {
                    mono.clear();
                    mono.extend(data.chunks(channels).map(|frame| {
                        frame.iter().map(|&s| f32::from_sample(s)).sum::<f32>() / frame.len() as f32
                    }));
                    let samples = if resampler.is_identity() {
                        &mono
                    } else {
                        resampled.clear();
                        resampler.process(&mono, &mut resampled);
                        &resampled
                    };
                    framer.push(samples, |frame| sink(frame));
                },
                |err| eprintln!("audio capture error: {err}"),
                None,
            )
            .map_err(|err| err.to_string())
    }

    fn build_output<T>(
        device: &cpal::Device,
        config: StreamConfig,
        mut source: PlaybackSource,
    ) -> Result<cpal::Stream, String>
    where
        T: SizedSample + FromSample<f32>,
    {
        let channels = config.channels.max(1) as usize;
        let mut resampler = Resampler::new(SAMPLE_RATE, config.sample_rate);
        let mut pending: VecDeque<f32> = VecDeque::with_capacity(FRAME * 2);
        let mut resampled = Vec::new();
        device
            .build_output_stream(
                config,
                move |data: &mut [T], _| {
                    for out in data.chunks_mut(channels) {
                        if pending.is_empty() {
                            let frame = source();
                            if resampler.is_identity() {
                                pending.extend(frame);
                            } else {
                                resampled.clear();
                                resampler.process(&frame, &mut resampled);
                                pending.extend(resampled.iter().copied());
                            }
                        }
                        let sample = T::from_sample(pending.pop_front().unwrap_or(0.0));
                        out.fill(sample);
                    }
                },
                |err| eprintln!("audio playback error: {err}"),
                None,
            )
            .map_err(|err| err.to_string())
    }
}

#[cfg(test)]
mod tests {
    use super::super::media::signal::{tone_ratio, Sine};
    use super::*;

    #[test]
    fn resampler_keeps_duration_and_tone() {
        let mut sine = Sine::new(440.0, 0.5);
        let input: Vec<f32> = (0..50).flat_map(|_| sine.next_frame()).collect();

        let mut down = Resampler::new(48_000, 44_100);
        let mut at_44k = Vec::new();
        for chunk in input.chunks(300) {
            down.process(chunk, &mut at_44k);
        }
        let expected = input.len() * 44_100 / 48_000;
        assert!(
            at_44k.len().abs_diff(expected) <= 2,
            "{} vs {expected}",
            at_44k.len()
        );

        let mut up = Resampler::new(44_100, 48_000);
        let mut back = Vec::new();
        up.process(&at_44k, &mut back);
        assert!(back.len().abs_diff(input.len()) <= 3);
        let tail = &back[back.len() - 9600..];
        assert!(tone_ratio(tail, 440.0) > 0.9, "{}", tone_ratio(tail, 440.0));
    }

    #[test]
    fn identity_resampler_is_detected() {
        assert!(Resampler::new(48_000, 48_000).is_identity());
        assert!(!Resampler::new(44_100, 48_000).is_identity());
    }
}
