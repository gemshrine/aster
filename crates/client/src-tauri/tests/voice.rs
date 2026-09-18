mod common;

use std::net::SocketAddr;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::Duration;

use client_tauri_lib::core::settings::Settings;
use client_tauri_lib::core::signaling::{signaling, ConnectionState, SignalingHandle};
use client_tauri_lib::core::voice::audio::{
    AudioBackend, AudioDevice, AudioDevices, AudioStream, CaptureSink, Opened, PlaybackSource,
};
use client_tauri_lib::core::voice::call::AudioDirection;
use client_tauri_lib::core::voice::call::{
    voice, CallConfig, CallError, CallState, EndReason, PushToTalkSource, VoiceEvent, VoiceHandle,
};
use client_tauri_lib::core::voice::media::signal::{tone_ratio, Sine};
use client_tauri_lib::core::voice::media::{level_dbfs, FRAME};
use client_tauri_lib::core::voice::settings::{InputMode, NoiseSuppression, VoiceSettings};
use client_tauri_lib::core::voice::transport::{CandidateKind, TransportPath};
use common::{fast_timing, spawn_server, ALICE, BOB, WAIT};
use signaling_server::hub::QueueLimits;
use tokio::sync::mpsc;

/// Plays a sine into capture and records everything handed to playback.
struct ToneBackend {
    freq: f32,
    /// How loud the speaker leaks back into the microphone.
    echo_gain: f32,
    played: Arc<Mutex<Vec<f32>>>,
    /// `(direction, resolved device)` for every opened stream, in order.
    opened: Arc<Mutex<Vec<(&'static str, String)>>>,
}

const INPUTS: [&str; 2] = ["mic-a", "mic-b"];
const OUTPUTS: [&str; 2] = ["spk-a", "spk-b"];

impl ToneBackend {
    fn resolve(
        &self,
        direction: &'static str,
        known: &[&str],
        requested: Option<&str>,
    ) -> Option<String> {
        let (device, fallback) = match requested {
            Some(id) if known.contains(&id) => (id.to_string(), None),
            Some(id) => (known[0].to_string(), Some(format!("{id} not found"))),
            None => (known[0].to_string(), None),
        };
        self.opened.lock().unwrap().push((direction, device));
        fallback
    }
}

fn list(ids: &[&str]) -> Vec<AudioDevice> {
    ids.iter()
        .enumerate()
        .map(|(i, id)| AudioDevice {
            id: id.to_string(),
            name: id.to_uppercase(),
            is_default: i == 0,
        })
        .collect()
}

struct ThreadStream {
    stop: Arc<AtomicBool>,
    thread: Option<thread::JoinHandle<()>>,
}

impl AudioStream for ThreadStream {}

impl Drop for ThreadStream {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}

fn every_frame(mut tick: impl FnMut() + Send + 'static) -> Box<dyn AudioStream> {
    let stop = Arc::new(AtomicBool::new(false));
    let stopped = stop.clone();
    let thread = thread::spawn(move || {
        let period = Duration::from_millis(20);
        let mut next = std::time::Instant::now();
        while !stopped.load(Ordering::Relaxed) {
            tick();
            next += period;
            thread::sleep(next.saturating_duration_since(std::time::Instant::now()));
        }
    });
    Box::new(ThreadStream {
        stop,
        thread: Some(thread),
    })
}

impl AudioBackend for ToneBackend {
    fn devices(&self) -> AudioDevices {
        AudioDevices {
            inputs: list(&INPUTS),
            outputs: list(&OUTPUTS),
        }
    }

    fn capture(&self, device: Option<&str>, mut sink: CaptureSink) -> Result<Opened, String> {
        let fallback = self.resolve("capture", &INPUTS, device);
        let mut sine = Sine::new(self.freq, 0.3);
        let (played, echo_gain) = (self.played.clone(), self.echo_gain);
        Ok(Opened {
            stream: every_frame(move || {
                let mut frame = sine.next_frame();
                let played = played.lock().unwrap();
                if echo_gain > 0.0 && played.len() >= FRAME {
                    let echo = &played[played.len() - FRAME..];
                    for (sample, echo) in frame.iter_mut().zip(echo) {
                        *sample += echo_gain * echo;
                    }
                }
                drop(played);
                sink(&frame)
            }),
            fallback,
        })
    }

    fn playback(&self, device: Option<&str>, mut source: PlaybackSource) -> Result<Opened, String> {
        let fallback = self.resolve("playback", &OUTPUTS, device);
        let played = self.played.clone();
        Ok(Opened {
            stream: every_frame(move || {
                let frame = source();
                played.lock().unwrap().extend_from_slice(&frame);
            }),
            fallback,
        })
    }
}

struct Peer {
    signaling: SignalingHandle,
    voice: VoiceHandle,
    events: mpsc::UnboundedReceiver<VoiceEvent>,
    played: Arc<Mutex<Vec<f32>>>,
    opened: Arc<Mutex<Vec<(&'static str, String)>>>,
}

impl Peer {
    async fn online(addr: SocketAddr, token: &str, freq: f32, config: CallConfig) -> Self {
        Self::with_audio(addr, token, freq, config, 0.0, VoiceSettings::default()).await
    }

    async fn with_audio(
        addr: SocketAddr,
        token: &str,
        freq: f32,
        config: CallConfig,
        echo_gain: f32,
        settings: VoiceSettings,
    ) -> Self {
        let (core_tx, mut core_events) = mpsc::unbounded_channel();
        let (signaling, actor) = signaling(core_tx, fast_timing());
        tokio::spawn(actor);

        let played = Arc::new(Mutex::new(Vec::new()));
        let opened = Arc::new(Mutex::new(Vec::new()));
        let backend = Arc::new(ToneBackend {
            freq,
            echo_gain,
            played: played.clone(),
            opened: opened.clone(),
        });
        let (voice_tx, events) = mpsc::unbounded_channel();
        let (voice, actor) = voice(signaling.clone(), backend, voice_tx, config, settings);
        tokio::spawn(actor);

        let forward = voice.clone();
        tokio::spawn(async move {
            while let Some(event) = core_events.recv().await {
                forward.core_event(&event);
            }
        });

        signaling.connect(Settings::new(&format!("ws://{addr}/ws"), token).unwrap());
        tokio::time::timeout(WAIT, async {
            while !matches!(signaling.state(), ConnectionState::Connected { .. }) {
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
        })
        .await
        .expect("did not connect");

        Self {
            signaling,
            voice,
            events,
            played,
            opened,
        }
    }

    fn opened(&self) -> Vec<(&'static str, String)> {
        self.opened.lock().unwrap().clone()
    }

    async fn wait(&mut self, pred: impl Fn(&VoiceEvent) -> bool) -> VoiceEvent {
        let fut = async {
            loop {
                let event = self.events.recv().await.expect("voice stopped");
                if pred(&event) {
                    return event;
                }
            }
        };
        tokio::time::timeout(Duration::from_secs(10), fut)
            .await
            .expect("expected voice event did not arrive")
    }

    async fn wait_state(&mut self, state: CallState) {
        self.wait(|e| *e == VoiceEvent::State(state.clone())).await;
    }

    async fn wait_connected(&mut self) {
        self.wait(|e| matches!(e, VoiceEvent::State(CallState::Connected { .. })))
            .await;
    }

    async fn wait_ended(&mut self, reason: EndReason) {
        self.wait_state(CallState::Ended { reason }).await;
    }

    /// The last `ms` of what reached this peer's speaker.
    fn played_tail(&self, ms: usize) -> Vec<f32> {
        let played = self.played.lock().unwrap();
        let len = (ms * 48).min(played.len());
        played[played.len() - len..].to_vec()
    }

    /// Waits for the call to be back at idle.
    ///
    /// `ended` is momentary (spec 0011): the core emits it and returns to
    /// `idle` right after, so reading the state the instant the end event
    /// arrives is a race.
    async fn expect_idle(&self) {
        let deadline = std::time::Instant::now() + Duration::from_secs(2);
        loop {
            let state = self.voice.state();
            if state == CallState::Idle {
                return;
            }
            assert!(
                std::time::Instant::now() < deadline,
                "call did not return to idle, still {state:?}"
            );
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    }

    /// Waits until the tail of the speaker signal is a clean `freq` tone.
    ///
    /// The first tail after `connected` can still hold silence, PLC frames or
    /// a phase jump from the jitter buffer filling up, which drags the
    /// Goertzel ratio below the threshold. Sampling again a moment later
    /// rather than once at a fixed delay keeps the assertion strict — the
    /// tone must arrive clean — without depending on how fast the machine
    /// running the test happens to be.
    async fn wait_for_tone(&self, freq: f32, ms: usize) -> Vec<f32> {
        let deadline = std::time::Instant::now() + Duration::from_secs(5);
        loop {
            let tail = self.played_tail(ms);
            let (level, ratio) = (level_dbfs(&tail), tone_ratio(&tail, freq));
            if level > -20.0 && ratio > 0.5 {
                return tail;
            }
            assert!(
                std::time::Instant::now() < deadline,
                "no clean {freq} Hz tone within 5 s: {level} dBFS, ratio {ratio}"
            );
            tokio::time::sleep(Duration::from_millis(100)).await;
        }
    }
}

fn test_config() -> CallConfig {
    CallConfig {
        udp_addrs: vec!["127.0.0.1:0".to_string()],
        ..CallConfig::default()
    }
}

async fn both_online(config: CallConfig) -> (Peer, Peer) {
    let addr = spawn_server(QueueLimits::default()).await;
    let alice = Peer::online(addr, ALICE, 440.0, config.clone()).await;
    let bob = Peer::online(addr, BOB, 1000.0, config).await;
    tokio::time::timeout(WAIT, async {
        while !matches!(
            alice.signaling.state(),
            ConnectionState::Connected {
                peer_online: true,
                ..
            }
        ) {
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .expect("peer did not come online");
    (alice, bob)
}

async fn connected_call() -> (Peer, Peer) {
    let (alice, bob) = both_online(test_config()).await;
    connect(alice, bob).await
}

async fn connect(mut alice: Peer, mut bob: Peer) -> (Peer, Peer) {
    alice.voice.start_call().await.unwrap();
    alice.wait_state(CallState::Calling).await;
    bob.wait_state(CallState::Ringing).await;
    bob.voice.accept_call().await.unwrap();
    alice.wait_connected().await;
    bob.wait_connected().await;
    (alice, bob)
}

#[tokio::test(flavor = "multi_thread")]
async fn call_connects_and_audio_flows_both_ways() {
    let (mut alice, mut bob) = connected_call().await;

    alice
        .wait(|e| matches!(e, VoiceEvent::Audio(a) if a.remote_speaking && a.local_speaking))
        .await;
    bob.wait(|e| matches!(e, VoiceEvent::Audio(a) if a.remote_speaking))
        .await;
    tokio::time::sleep(Duration::from_millis(600)).await;

    // Each peer must hear the other's tone and only that tone: alice sends
    // 440 Hz, bob 1000 Hz.
    let at_bob = bob.wait_for_tone(440.0, 400).await;
    assert!(
        tone_ratio(&at_bob, 1000.0) < 0.1,
        "1000 Hz leaked into bob's speaker: {}",
        tone_ratio(&at_bob, 1000.0)
    );

    let at_alice = alice.wait_for_tone(1000.0, 400).await;
    assert!(
        tone_ratio(&at_alice, 440.0) < 0.1,
        "440 Hz leaked into alice's speaker: {}",
        tone_ratio(&at_alice, 440.0)
    );

    alice.voice.hang_up().await.unwrap();
    alice.wait_ended(EndReason::Hangup).await;
    bob.wait_ended(EndReason::RemoteHangup).await;
    bob.expect_idle().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn mute_silences_the_microphone() {
    let (mut alice, mut bob) = connected_call().await;
    bob.wait(|e| matches!(e, VoiceEvent::Audio(a) if a.remote_speaking))
        .await;

    alice.voice.set_muted(true).await.unwrap();
    alice
        .wait(|e| matches!(e, VoiceEvent::Audio(a) if a.muted && !a.local_speaking))
        .await;
    bob.wait(|e| matches!(e, VoiceEvent::Audio(a) if !a.remote_speaking))
        .await;
    let at_bob = bob.played_tail(200);
    assert!(
        level_dbfs(&at_bob) < -45.0,
        "{} dBFS after mute",
        level_dbfs(&at_bob)
    );

    alice.voice.set_muted(false).await.unwrap();
    bob.wait(|e| matches!(e, VoiceEvent::Audio(a) if a.remote_speaking))
        .await;
}

#[tokio::test(flavor = "multi_thread")]
async fn declined_call_ends_for_both() {
    let (mut alice, mut bob) = both_online(test_config()).await;
    alice.voice.start_call().await.unwrap();
    bob.wait_state(CallState::Ringing).await;
    assert_eq!(
        alice.voice.accept_call().await,
        Err(CallError::InvalidState)
    );

    bob.voice.decline_call().await.unwrap();
    bob.wait_ended(EndReason::Declined).await;
    alice.wait_ended(EndReason::Declined).await;
    alice.expect_idle().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn caller_can_cancel_while_ringing() {
    let (mut alice, mut bob) = both_online(test_config()).await;
    alice.voice.start_call().await.unwrap();
    bob.wait_state(CallState::Ringing).await;

    alice.voice.hang_up().await.unwrap();
    alice.wait_ended(EndReason::Hangup).await;
    bob.wait_ended(EndReason::Cancelled).await;
}

#[tokio::test(flavor = "multi_thread")]
async fn unanswered_call_times_out() {
    let (mut alice, mut bob) = both_online(CallConfig {
        ring_timeout: Duration::from_millis(400),
        ..test_config()
    })
    .await;
    alice.voice.start_call().await.unwrap();
    bob.wait_state(CallState::Ringing).await;
    alice.wait_ended(EndReason::Timeout).await;
    bob.wait(|e| matches!(e, VoiceEvent::State(CallState::Ended { .. })))
        .await;
    bob.expect_idle().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn simultaneous_calls_connect_once() {
    let (mut alice, mut bob) = both_online(test_config()).await;
    let (a, b) = tokio::join!(alice.voice.start_call(), bob.voice.start_call());
    a.unwrap();
    b.unwrap();
    alice.wait_connected().await;
    bob.wait_connected().await;
    tokio::time::sleep(Duration::from_millis(300)).await;
    assert!(matches!(alice.voice.state(), CallState::Connected { .. }));
    assert!(matches!(bob.voice.state(), CallState::Connected { .. }));
}

#[tokio::test(flavor = "multi_thread")]
async fn calling_requires_an_online_peer() {
    let addr = spawn_server(QueueLimits::default()).await;
    let alice = Peer::online(addr, ALICE, 440.0, test_config()).await;
    assert_eq!(alice.voice.start_call().await, Err(CallError::PeerOffline));
    assert_eq!(alice.voice.hang_up().await, Err(CallError::InvalidState));
}

#[tokio::test(flavor = "multi_thread")]
async fn caller_going_offline_ends_ringing() {
    let (mut alice, mut bob) = both_online(test_config()).await;
    alice.voice.start_call().await.unwrap();
    bob.wait_state(CallState::Ringing).await;

    alice.signaling.disconnect();
    alice.wait_ended(EndReason::ConnectionLost).await;
    bob.wait_ended(EndReason::PeerOffline).await;
}

#[tokio::test(flavor = "multi_thread")]
async fn output_device_switches_during_a_call() {
    let (mut alice, mut bob) = connected_call().await;
    bob.wait(|e| matches!(e, VoiceEvent::Audio(a) if a.remote_speaking))
        .await;
    assert_eq!(
        bob.opened(),
        [
            ("capture", "mic-a".to_string()),
            ("playback", "spk-a".to_string())
        ]
    );

    bob.voice
        .set_settings(VoiceSettings {
            output_device: Some("spk-b".into()),
            ..VoiceSettings::default()
        })
        .await
        .unwrap();
    assert_eq!(
        bob.opened().last(),
        Some(&("playback", "spk-b".to_string()))
    );
    assert_eq!(
        bob.opened().iter().filter(|(d, _)| *d == "capture").count(),
        1,
        "capture must not restart"
    );

    // Audio keeps flowing to the new device and the call survives.
    let before = bob.played.lock().unwrap().len();
    tokio::time::sleep(Duration::from_millis(500)).await;
    let tail = bob.played_tail(300);
    assert!(bob.played.lock().unwrap().len() > before);
    assert!(
        level_dbfs(&tail) > -20.0,
        "{} dBFS after switch",
        level_dbfs(&tail)
    );
    assert!(matches!(bob.voice.state(), CallState::Connected { .. }));
    assert!(matches!(alice.voice.state(), CallState::Connected { .. }));
    alice.voice.hang_up().await.unwrap();
    alice.wait_ended(EndReason::Hangup).await;
}

#[tokio::test(flavor = "multi_thread")]
async fn missing_device_falls_back_without_ending_the_call() {
    let (mut alice, mut bob) = both_online(test_config()).await;
    bob.voice
        .set_settings(VoiceSettings {
            input_device: Some("unplugged-headset".into()),
            ..VoiceSettings::default()
        })
        .await
        .unwrap();

    alice.voice.start_call().await.unwrap();
    bob.wait_state(CallState::Ringing).await;
    bob.voice.accept_call().await.unwrap();
    let error = bob
        .wait(|e| matches!(e, VoiceEvent::AudioError { .. }))
        .await;
    assert!(matches!(
        error,
        VoiceEvent::AudioError { direction: AudioDirection::Capture, ref message }
            if message.contains("unplugged-headset")
    ));
    bob.wait_connected().await;
    assert_eq!(bob.opened()[0], ("capture", "mic-a".to_string()));

    alice
        .wait(|e| matches!(e, VoiceEvent::Audio(a) if a.remote_speaking))
        .await;
}

/// Bob's speaker leaks alice's 440 Hz back into his microphone. How much of
/// it comes back to alice's speaker, relative to bob's own 1000 Hz tone.
///
/// Only bob processes audio. Noise suppression and gain adapt to a steady
/// tone over time, which would make the result depend on machine speed. And
/// with AEC at alice too, the 440 Hz coming back in her render reference
/// lets her echo suppressor mute her own 440 Hz microphone, so whoever
/// starts first decides whether any echo exists at all.
async fn echo_returned(echo_cancellation: bool) -> f32 {
    let addr = spawn_server(QueueLimits::default()).await;
    let unprocessed = VoiceSettings {
        echo_cancellation: false,
        noise_suppression: NoiseSuppression::Off,
        auto_gain: false,
        ..VoiceSettings::default()
    };
    let alice = Peer::with_audio(addr, ALICE, 440.0, test_config(), 0.0, unprocessed.clone()).await;
    let settings = VoiceSettings {
        echo_cancellation,
        ..unprocessed
    };
    let bob = Peer::with_audio(addr, BOB, 1000.0, test_config(), 0.8, settings).await;
    tokio::time::timeout(WAIT, async {
        while !matches!(
            alice.signaling.state(),
            ConnectionState::Connected {
                peer_online: true,
                ..
            }
        ) {
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .expect("peer did not come online");
    let (alice, bob) = connect(alice, bob).await;

    // The echo exists only once alice's voice plays at bob.
    bob.wait_for_tone(440.0, 400).await;
    alice.wait_for_tone(1000.0, 400).await;
    // Give AEC3 time to converge on the echo path.
    tokio::time::sleep(Duration::from_secs(3)).await;
    let tail = alice.played_tail(1000);
    tone_ratio(&tail, 440.0) / tone_ratio(&tail, 1000.0)
}

#[tokio::test(flavor = "multi_thread")]
async fn echo_cancellation_keeps_the_callers_voice_from_coming_back() {
    let without = echo_returned(false).await;
    let with = echo_returned(true).await;
    assert!(without > 0.05, "no echo to cancel in the setup: {without}");
    assert!(with < without / 20.0, "echo {without} -> {with}");
}

#[tokio::test(flavor = "multi_thread")]
async fn input_monitor_reports_levels_outside_a_call() {
    let addr = spawn_server(QueueLimits::default()).await;
    let mut alice = Peer::online(addr, ALICE, 440.0, test_config()).await;

    alice.voice.set_input_monitor(true).await.unwrap();
    let mut levels = Vec::new();
    while levels.len() < 10 {
        if let VoiceEvent::InputLevel { dbfs } = alice
            .wait(|e| matches!(e, VoiceEvent::InputLevel { .. }))
            .await
        {
            levels.push(dbfs);
        }
    }
    assert_eq!(alice.opened(), [("capture", "mic-a".to_string())]);
    let loudest = levels.iter().cloned().fold(f32::MIN, f32::max);
    assert!(loudest > -30.0, "the tone should register: {levels:?}");

    // Nothing is sent: the peer is not even in a call.
    assert_eq!(alice.voice.state(), CallState::Idle);

    alice.voice.set_input_monitor(false).await.unwrap();
    tokio::time::sleep(Duration::from_millis(100)).await;
    while alice.events.try_recv().is_ok() {}
    tokio::time::sleep(Duration::from_millis(400)).await;
    assert!(
        !std::iter::from_fn(|| alice.events.try_recv().ok())
            .any(|e| matches!(e, VoiceEvent::InputLevel { .. })),
        "levels keep coming after the monitor is off"
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn push_to_talk_sends_only_while_the_key_is_held() {
    let addr = spawn_server(QueueLimits::default()).await;
    // Gain control would slowly turn the steady test tone down while the key
    // is up; this test is about the gate, not the processing.
    let settings = VoiceSettings {
        input_mode: InputMode::PushToTalk,
        ptt_release_delay_ms: 1000,
        echo_cancellation: false,
        noise_suppression: NoiseSuppression::Off,
        auto_gain: false,
        ..VoiceSettings::default()
    };
    let alice = Peer::with_audio(addr, ALICE, 440.0, test_config(), 0.0, settings).await;
    let bob = Peer::online(addr, BOB, 1000.0, test_config()).await;
    tokio::time::timeout(WAIT, async {
        while !matches!(
            alice.signaling.state(),
            ConnectionState::Connected {
                peer_online: true,
                ..
            }
        ) {
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .expect("peer did not come online");
    let (mut alice, bob) = connect(alice, bob).await;

    // Not pressed: bob hears nothing although alice's microphone is loud.
    alice.wait_for_tone(1000.0, 400).await;
    let at_bob = bob.played_tail(400);
    assert!(level_dbfs(&at_bob) < -45.0, "{} dBFS", level_dbfs(&at_bob));

    alice.voice.set_push_to_talk(PushToTalkSource::Window, true);
    alice
        .wait(|e| matches!(e, VoiceEvent::Audio(a) if a.transmitting))
        .await;
    bob.wait_for_tone(440.0, 400).await;

    // Released: the tail keeps going for the delay, then silence.
    alice
        .voice
        .set_push_to_talk(PushToTalkSource::Window, false);
    tokio::time::sleep(Duration::from_millis(400)).await;
    let tail = bob.played_tail(200);
    assert!(
        level_dbfs(&tail) > -20.0,
        "tail cut: {} dBFS",
        level_dbfs(&tail)
    );
    alice
        .wait(|e| matches!(e, VoiceEvent::Audio(a) if !a.transmitting))
        .await;
    tokio::time::sleep(Duration::from_millis(400)).await;
    let after = bob.played_tail(200);
    assert!(level_dbfs(&after) < -45.0, "{} dBFS", level_dbfs(&after));

    // A global press counts the same as the window.
    alice.voice.set_push_to_talk(PushToTalkSource::Global, true);
    bob.wait_for_tone(440.0, 400).await;
}

#[tokio::test(flavor = "multi_thread")]
async fn transport_reports_the_selected_candidate_pair() {
    let (mut alice, mut bob) = connected_call().await;

    let event = alice
        .wait(|e| matches!(e, VoiceEvent::Transport(t) if t.selected.is_some()))
        .await;
    let VoiceEvent::Transport(status) = event else {
        unreachable!()
    };
    let pair = status.selected.clone().expect("a nominated pair");
    // Both peers are on loopback, so the pair is host to host over UDP.
    assert_eq!(status.path, Some(TransportPath::Direct));
    assert_eq!(pair.local, CandidateKind::Host);
    // The peer's candidate is not always in the report (peer reflexive).
    assert!(
        matches!(pair.remote, None | Some(CandidateKind::Host)),
        "{:?}",
        pair.remote
    );
    assert_eq!(pair.protocol, "udp");
    assert!(status.local_candidates.host > 0, "{status:?}");
    assert!(status.remote_candidates.host > 0, "{status:?}");
    assert_eq!(status.ice.as_deref(), Some("connected"));
    assert!(status.errors.is_empty(), "{:?}", status.errors);

    bob.voice.hang_up().await.unwrap();
    bob.wait_ended(EndReason::Hangup).await;
}

#[test]
fn frame_is_20ms() {
    assert_eq!(FRAME, 960);
}
