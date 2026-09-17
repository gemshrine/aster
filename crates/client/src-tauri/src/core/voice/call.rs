//! Call state machine and WebRTC session, see `specs/0011-voice-core.md`.

use std::future::Future;
use std::sync::atomic::{AtomicBool, AtomicI32, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use bytes::Bytes;
use protocol::{ErrorCode, SignalPayload};
use rtc::media::Sample;
use rtc::media_stream::MediaStreamTrack;
use rtc::peer_connection::configuration::media_engine::MIME_TYPE_OPUS;
use rtc::rtp_transceiver::rtp_sender::{
    RTCRtpCodec, RTCRtpCodingParameters, RTCRtpEncodingParameters, RtpCodecKind,
};
use serde::Serialize;
use tokio::sync::{mpsc, oneshot, watch};
use tokio::task::JoinHandle;
use tokio::time::Instant;
use webrtc::media_stream::track_local::static_sample::TrackLocalStaticSample;
use webrtc::media_stream::track_local::TrackLocal;
use webrtc::media_stream::track_remote::{TrackRemote, TrackRemoteEvent};
use webrtc::media_stream::Track;
use webrtc::peer_connection::{
    MediaEngine, PeerConnection, PeerConnectionBuilder, PeerConnectionEventHandler,
    RTCConfigurationBuilder, RTCIceCandidateInit, RTCIceServer, RTCPeerConnectionIceEvent,
    RTCPeerConnectionState, RTCSessionDescription,
};
use webrtc::rtp_transceiver::RtpSender;

use super::audio::{AudioBackend, AudioStream};
use super::media::{Encoder, Frame, JitterBuffer, LevelMeter, Vad, FRAME, SILENCE};
use super::processing::{Dsp, DspInput, ProcessingConfig};
use super::settings::VoiceSettings;
use crate::core::signaling::{ConnectionState, CoreEvent, SignalingHandle};

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(tag = "state", rename_all = "snake_case")]
pub enum CallState {
    Idle,
    Calling,
    Ringing,
    Connecting,
    Connected { since: i64 },
    Ended { reason: EndReason },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum EndReason {
    Hangup,
    Declined,
    Cancelled,
    RemoteHangup,
    Timeout,
    Failed,
    ConnectionLost,
    PeerOffline,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize)]
pub struct AudioStatus {
    pub muted: bool,
    pub local_speaking: bool,
    pub remote_speaking: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum AudioDirection {
    Capture,
    Playback,
}

#[derive(Debug, Clone, PartialEq)]
pub enum VoiceEvent {
    State(CallState),
    Audio(AudioStatus),
    AudioError {
        direction: AudioDirection,
        message: String,
    },
    /// Microphone level after processing, while the input monitor is on.
    InputLevel {
        dbfs: f32,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum CallError {
    InvalidState,
    NotConnected,
    PeerOffline,
    Failed,
}

#[derive(Debug, Clone)]
pub struct CallConfig {
    pub ring_timeout: Duration,
    pub connect_timeout: Duration,
    pub disconnect_grace: Duration,
    pub audio_poll: Duration,
    /// Local addresses WebRTC binds for ICE.
    pub udp_addrs: Vec<String>,
}

impl Default for CallConfig {
    fn default() -> Self {
        Self {
            ring_timeout: Duration::from_secs(30),
            connect_timeout: Duration::from_secs(20),
            disconnect_grace: Duration::from_secs(10),
            audio_poll: Duration::from_millis(100),
            udp_addrs: vec!["0.0.0.0:0".to_string()],
        }
    }
}

enum Command {
    Start,
    Accept,
    Decline,
    HangUp,
    SetMuted(bool),
    UpdateSettings(VoiceSettings),
    SetInputMonitor(bool),
}

enum Input {
    Command(Command, oneshot::Sender<Result<(), CallError>>),
    Core(CoreEvent),
}

enum PcEvent {
    Candidate(RTCIceCandidateInit),
    State(RTCPeerConnectionState),
    Track(Arc<dyn TrackRemote>),
}

#[derive(Clone)]
pub struct VoiceHandle {
    inputs: mpsc::UnboundedSender<Input>,
    state: watch::Receiver<CallState>,
}

impl VoiceHandle {
    pub fn state(&self) -> CallState {
        self.state.borrow().clone()
    }

    pub async fn start_call(&self) -> Result<(), CallError> {
        self.command(Command::Start).await
    }

    pub async fn accept_call(&self) -> Result<(), CallError> {
        self.command(Command::Accept).await
    }

    pub async fn decline_call(&self) -> Result<(), CallError> {
        self.command(Command::Decline).await
    }

    pub async fn hang_up(&self) -> Result<(), CallError> {
        self.command(Command::HangUp).await
    }

    pub async fn set_muted(&self, muted: bool) -> Result<(), CallError> {
        self.command(Command::SetMuted(muted)).await
    }

    /// Applies immediately, including to a call in progress.
    pub async fn set_settings(&self, settings: VoiceSettings) -> Result<(), CallError> {
        self.command(Command::UpdateSettings(settings)).await
    }

    /// Reports `InputLevel` 10 times a second; outside a call this opens the
    /// microphone with the same processing but sends nothing.
    pub async fn set_input_monitor(&self, enabled: bool) -> Result<(), CallError> {
        self.command(Command::SetInputMonitor(enabled)).await
    }

    /// Feeds signaling events to the call; call in event order.
    pub fn core_event(&self, event: &CoreEvent) {
        let relevant = matches!(
            event,
            CoreEvent::Signal(_)
                | CoreEvent::State(_)
                | CoreEvent::PeerStatus { .. }
                | CoreEvent::ServerError {
                    code: ErrorCode::PeerOffline,
                    ..
                }
        );
        if relevant {
            let _ = self.inputs.send(Input::Core(event.clone()));
        }
    }

    async fn command(&self, command: Command) -> Result<(), CallError> {
        let (reply, rx) = oneshot::channel();
        self.inputs
            .send(Input::Command(command, reply))
            .map_err(|_| CallError::Failed)?;
        rx.await.unwrap_or(Err(CallError::Failed))
    }
}

/// Creates the handle and the actor future; the caller spawns the future.
pub fn voice(
    signaling: SignalingHandle,
    backend: Arc<dyn AudioBackend>,
    events: mpsc::UnboundedSender<VoiceEvent>,
    config: CallConfig,
    settings: VoiceSettings,
) -> (VoiceHandle, impl Future<Output = ()> + Send + 'static) {
    let (inputs_tx, inputs) = mpsc::unbounded_channel();
    let (pc_tx, pc_rx) = mpsc::unbounded_channel();
    let (state_tx, state) = watch::channel(CallState::Idle);
    let handle = VoiceHandle {
        inputs: inputs_tx,
        state,
    };
    let actor = Actor {
        inputs,
        pc_tx,
        pc_rx,
        signaling,
        backend,
        events,
        state: state_tx,
        config,
        settings,
        phase: Phase::Idle,
        generation: 0,
        muted: false,
        audio: AudioStatus::default(),
        monitor: Arc::new(AtomicBool::new(false)),
        idle_monitor: None,
    };
    (handle, actor.run())
}

#[derive(Default)]
struct Flags {
    muted: AtomicBool,
    local_speaking: AtomicBool,
    remote_speaking: AtomicBool,
    vad_threshold_dbfs: AtomicI32,
}

struct Session {
    generation: u64,
    pc: Arc<dyn PeerConnection>,
    track: Arc<TrackLocalStaticSample>,
    sender: Arc<dyn RtpSender>,
    remote_set: bool,
    pending_candidates: Vec<RTCIceCandidateInit>,
    offer_session_id: Option<u64>,
    jitter: Arc<Mutex<JitterBuffer>>,
    flags: Arc<Flags>,
    receiver: Option<JoinHandle<()>>,
    media: Option<Media>,
}

/// A microphone stream and the processing it feeds.
struct Capture {
    /// Declared first: the stream stops before its DSP thread loses input.
    stream: Option<Box<dyn AudioStream>>,
    /// Kept so a new stream can feed the running processing.
    dsp: Dsp,
}

struct Media {
    capture: Capture,
    playback: Option<Box<dyn AudioStream>>,
    encoder: JoinHandle<()>,
}

impl Drop for Media {
    fn drop(&mut self) {
        self.encoder.abort();
    }
}

impl Session {
    fn close(mut self) {
        if let Some(receiver) = self.receiver.take() {
            receiver.abort();
        }
        drop(self.media.take());
        let pc = self.pc.clone();
        tokio::spawn(async move {
            let _ = pc.close().await;
        });
    }

    async fn add_remote_candidate(&mut self, candidate: RTCIceCandidateInit) {
        if self.remote_set {
            let _ = self.pc.add_ice_candidate(candidate).await;
        } else {
            self.pending_candidates.push(candidate);
        }
    }

    async fn set_remote(&mut self, description: RTCSessionDescription) -> Result<(), CallError> {
        self.pc
            .set_remote_description(description)
            .await
            .map_err(|_| CallError::Failed)?;
        self.remote_set = true;
        for candidate in std::mem::take(&mut self.pending_candidates) {
            let _ = self.pc.add_ice_candidate(candidate).await;
        }
        Ok(())
    }
}

// One instance per actor, so the variant size gap costs nothing.
#[allow(clippy::large_enum_variant)]
enum Phase {
    Idle,
    Calling {
        session: Session,
        deadline: Instant,
    },
    Ringing {
        offer: RTCSessionDescription,
        candidates: Vec<RTCIceCandidateInit>,
        deadline: Instant,
    },
    Connecting {
        session: Session,
        deadline: Instant,
    },
    Connected {
        session: Session,
        disconnected_since: Option<Instant>,
    },
}

impl Phase {
    fn session_mut(&mut self) -> Option<&mut Session> {
        match self {
            Phase::Calling { session, .. }
            | Phase::Connecting { session, .. }
            | Phase::Connected { session, .. } => Some(session),
            Phase::Idle | Phase::Ringing { .. } => None,
        }
    }

    /// Before media flows: signaling or peer loss ends the call.
    fn is_establishing(&self) -> bool {
        matches!(
            self,
            Phase::Calling { .. } | Phase::Ringing { .. } | Phase::Connecting { .. }
        )
    }
}

struct Actor {
    inputs: mpsc::UnboundedReceiver<Input>,
    pc_tx: mpsc::UnboundedSender<(u64, PcEvent)>,
    pc_rx: mpsc::UnboundedReceiver<(u64, PcEvent)>,
    signaling: SignalingHandle,
    backend: Arc<dyn AudioBackend>,
    events: mpsc::UnboundedSender<VoiceEvent>,
    state: watch::Sender<CallState>,
    config: CallConfig,
    settings: VoiceSettings,
    phase: Phase,
    generation: u64,
    muted: bool,
    audio: AudioStatus,
    /// Shared with every DSP thread: whether to report `InputLevel`.
    monitor: Arc<AtomicBool>,
    /// The monitor's own microphone while no call has media.
    idle_monitor: Option<Capture>,
}

impl Actor {
    async fn run(mut self) {
        let mut audio_poll = tokio::time::interval(self.config.audio_poll);
        loop {
            let deadline = self.deadline();
            let in_call = matches!(self.phase, Phase::Connected { .. });
            tokio::select! {
                input = self.inputs.recv() => match input {
                    None => {
                        self.teardown();
                        return;
                    }
                    Some(Input::Command(command, reply)) => {
                        let result = self.command(command).await;
                        let _ = reply.send(result);
                    }
                    Some(Input::Core(event)) => self.core_event(event).await,
                },
                Some((generation, event)) = self.pc_rx.recv() => {
                    if self.phase.session_mut().is_some_and(|s| s.generation == generation) {
                        self.pc_event(event).await;
                    }
                }
                _ = sleep_until(deadline) => self.deadline_passed().await,
                _ = audio_poll.tick(), if in_call => self.refresh_audio(),
            }
        }
    }

    fn deadline(&self) -> Option<Instant> {
        match &self.phase {
            Phase::Idle => None,
            Phase::Calling { deadline, .. }
            | Phase::Ringing { deadline, .. }
            | Phase::Connecting { deadline, .. } => Some(*deadline),
            Phase::Connected {
                disconnected_since, ..
            } => disconnected_since.map(|since| since + self.config.disconnect_grace),
        }
    }

    async fn command(&mut self, command: Command) -> Result<(), CallError> {
        match command {
            Command::Start => self.start_call().await,
            Command::Accept => {
                let Phase::Ringing { .. } = self.phase else {
                    return Err(CallError::InvalidState);
                };
                let Phase::Ringing {
                    offer, candidates, ..
                } = std::mem::replace(&mut self.phase, Phase::Idle)
                else {
                    unreachable!()
                };
                self.answer(offer, candidates).await
            }
            Command::Decline => {
                let Phase::Ringing { .. } = self.phase else {
                    return Err(CallError::InvalidState);
                };
                self.send_call_end().await;
                self.end(EndReason::Declined);
                Ok(())
            }
            Command::HangUp => {
                if matches!(self.phase, Phase::Idle) {
                    return Err(CallError::InvalidState);
                }
                self.send_call_end().await;
                self.end(EndReason::Hangup);
                Ok(())
            }
            Command::UpdateSettings(settings) => {
                self.apply_settings(settings);
                Ok(())
            }
            Command::SetInputMonitor(enabled) => {
                self.monitor.store(enabled, Ordering::Relaxed);
                self.sync_monitor();
                Ok(())
            }
            Command::SetMuted(muted) => {
                self.muted = muted;
                if let Some(session) = self.phase.session_mut() {
                    session.flags.muted.store(muted, Ordering::Relaxed);
                }
                self.refresh_audio();
                Ok(())
            }
        }
    }

    async fn start_call(&mut self) -> Result<(), CallError> {
        if !matches!(self.phase, Phase::Idle) {
            return Err(CallError::InvalidState);
        }
        match self.signaling.state() {
            ConnectionState::Connected {
                peer_online: true, ..
            } => {}
            ConnectionState::Connected { .. } => return Err(CallError::PeerOffline),
            _ => return Err(CallError::NotConnected),
        }

        let mut session = self.new_session().await?;
        let offer = async {
            let offer = session.pc.create_offer(None).await.ok()?;
            session.pc.set_local_description(offer.clone()).await.ok()?;
            Some(offer)
        }
        .await;
        let Some(offer) = offer else {
            session.close();
            return Err(CallError::Failed);
        };
        session.offer_session_id = sdp_session_id(&offer.sdp);

        if let Err(err) = self
            .signaling
            .send_signal(SignalPayload::Offer { sdp: offer.sdp })
            .await
        {
            session.close();
            return Err(match err {
                crate::core::signaling::SendError::NotConnected => CallError::NotConnected,
                crate::core::signaling::SendError::Io => CallError::Failed,
            });
        }

        self.phase = Phase::Calling {
            session,
            deadline: Instant::now() + self.config.ring_timeout,
        };
        self.set_state(CallState::Calling);
        Ok(())
    }

    async fn answer(
        &mut self,
        offer: RTCSessionDescription,
        candidates: Vec<RTCIceCandidateInit>,
    ) -> Result<(), CallError> {
        let mut session = match self.new_session().await {
            Ok(session) => session,
            Err(err) => {
                self.send_call_end().await;
                self.end(EndReason::Failed);
                return Err(err);
            }
        };
        session.pending_candidates = candidates;

        let answer = async {
            session.set_remote(offer).await.ok()?;
            let answer = session.pc.create_answer(None).await.ok()?;
            session
                .pc
                .set_local_description(answer.clone())
                .await
                .ok()?;
            Some(answer)
        }
        .await;
        let sent = match answer {
            Some(answer) => self
                .signaling
                .send_signal(SignalPayload::Answer { sdp: answer.sdp })
                .await
                .is_ok(),
            None => false,
        };
        if !sent {
            session.close();
            self.send_call_end().await;
            self.end(EndReason::Failed);
            return Err(CallError::Failed);
        }

        self.phase = Phase::Connecting {
            session,
            deadline: Instant::now() + self.config.connect_timeout,
        };
        self.set_state(CallState::Connecting);
        Ok(())
    }

    async fn core_event(&mut self, event: CoreEvent) {
        match event {
            CoreEvent::Signal(payload) => self.signal(payload).await,
            CoreEvent::State(ConnectionState::Connected { .. }) => {}
            CoreEvent::State(_) if self.phase.is_establishing() => {
                self.end(EndReason::ConnectionLost)
            }
            CoreEvent::PeerStatus { online: false } | CoreEvent::ServerError { .. }
                if self.phase.is_establishing() =>
            {
                self.end(EndReason::PeerOffline)
            }
            _ => {}
        }
    }

    async fn signal(&mut self, payload: SignalPayload) {
        match payload {
            SignalPayload::Offer { sdp } => {
                let Ok(offer) = RTCSessionDescription::offer(sdp.clone()) else {
                    return;
                };
                match &self.phase {
                    Phase::Idle => {
                        self.phase = Phase::Ringing {
                            offer,
                            candidates: Vec::new(),
                            deadline: Instant::now() + self.config.ring_timeout,
                        };
                        self.set_state(CallState::Ringing);
                    }
                    Phase::Calling { session, .. } => {
                        // Glare: the offer with the lower SDP session id wins.
                        let ours = session.offer_session_id;
                        let theirs = sdp_session_id(&sdp);
                        if ours > theirs {
                            let Phase::Calling { session, .. } =
                                std::mem::replace(&mut self.phase, Phase::Idle)
                            else {
                                unreachable!()
                            };
                            session.close();
                            let _ = self.answer(offer, Vec::new()).await;
                        }
                    }
                    _ => {}
                }
            }
            SignalPayload::Answer { sdp } => {
                let Phase::Calling { .. } = self.phase else {
                    return;
                };
                let Phase::Calling { mut session, .. } =
                    std::mem::replace(&mut self.phase, Phase::Idle)
                else {
                    unreachable!()
                };
                let applied = match RTCSessionDescription::answer(sdp) {
                    Ok(answer) => session.set_remote(answer).await.is_ok(),
                    Err(_) => false,
                };
                if !applied {
                    self.phase = Phase::Calling {
                        session,
                        deadline: Instant::now(),
                    };
                    self.send_call_end().await;
                    self.end(EndReason::Failed);
                    return;
                }
                self.phase = Phase::Connecting {
                    session,
                    deadline: Instant::now() + self.config.connect_timeout,
                };
                self.set_state(CallState::Connecting);
            }
            SignalPayload::IceCandidate {
                candidate,
                sdp_mid,
                sdp_mline_index,
            } => {
                let init = RTCIceCandidateInit {
                    candidate,
                    sdp_mid,
                    sdp_mline_index,
                    ..Default::default()
                };
                match &mut self.phase {
                    Phase::Ringing { candidates, .. } => candidates.push(init),
                    phase => {
                        if let Some(session) = phase.session_mut() {
                            session.add_remote_candidate(init).await;
                        }
                    }
                }
            }
            SignalPayload::CallEnd => {
                let reason = match self.phase {
                    Phase::Idle => return,
                    Phase::Calling { .. } => EndReason::Declined,
                    Phase::Ringing { .. } => EndReason::Cancelled,
                    Phase::Connecting { .. } | Phase::Connected { .. } => EndReason::RemoteHangup,
                };
                self.end(reason);
            }
        }
    }

    async fn pc_event(&mut self, event: PcEvent) {
        match event {
            PcEvent::Candidate(init) => {
                let _ = self
                    .signaling
                    .send_signal(SignalPayload::IceCandidate {
                        candidate: init.candidate,
                        sdp_mid: init.sdp_mid,
                        sdp_mline_index: init.sdp_mline_index,
                    })
                    .await;
            }
            PcEvent::Track(track) => {
                if let Some(session) = self.phase.session_mut() {
                    let jitter = session.jitter.clone();
                    session.receiver = Some(tokio::spawn(async move {
                        while let Some(event) = track.poll().await {
                            if let TrackRemoteEvent::OnRtpPacket(packet) = event {
                                jitter
                                    .lock()
                                    .unwrap()
                                    .push(packet.header.sequence_number, &packet.payload);
                            }
                        }
                    }));
                }
            }
            PcEvent::State(RTCPeerConnectionState::Connected) => match &mut self.phase {
                Phase::Connecting { .. } => {
                    let Phase::Connecting { mut session, .. } =
                        std::mem::replace(&mut self.phase, Phase::Idle)
                    else {
                        unreachable!()
                    };
                    self.start_media(&mut session).await;
                    self.phase = Phase::Connected {
                        session,
                        disconnected_since: None,
                    };
                    self.set_state(CallState::Connected { since: now_ms() });
                    self.refresh_audio();
                }
                Phase::Connected {
                    disconnected_since, ..
                } => *disconnected_since = None,
                _ => {}
            },
            PcEvent::State(RTCPeerConnectionState::Disconnected) => {
                if let Phase::Connected {
                    disconnected_since, ..
                } = &mut self.phase
                {
                    disconnected_since.get_or_insert_with(Instant::now);
                }
            }
            PcEvent::State(RTCPeerConnectionState::Failed) => {
                if matches!(
                    self.phase,
                    Phase::Connecting { .. } | Phase::Connected { .. }
                ) {
                    self.send_call_end().await;
                    self.end(EndReason::ConnectionLost);
                }
            }
            PcEvent::State(_) => {}
        }
    }

    async fn deadline_passed(&mut self) {
        let reason = match self.phase {
            Phase::Idle => return,
            Phase::Calling { .. } | Phase::Ringing { .. } => EndReason::Timeout,
            Phase::Connecting { .. } => EndReason::Failed,
            Phase::Connected { .. } => EndReason::ConnectionLost,
        };
        self.send_call_end().await;
        self.end(reason);
    }

    async fn new_session(&mut self) -> Result<Session, CallError> {
        self.generation += 1;
        let generation = self.generation;

        let mut media_engine = MediaEngine::default();
        media_engine
            .register_default_codecs()
            .map_err(|_| CallError::Failed)?;
        let ice_servers = self
            .signaling
            .ice_servers()
            .into_iter()
            .map(|server| RTCIceServer {
                urls: server.urls,
                username: server.username.unwrap_or_default(),
                credential: server.credential.unwrap_or_default(),
            })
            .collect();
        let pc = PeerConnectionBuilder::new()
            .with_configuration(
                RTCConfigurationBuilder::default()
                    .with_ice_servers(ice_servers)
                    .build(),
            )
            .with_media_engine(media_engine)
            .with_handler(Arc::new(PcHandler {
                generation,
                events: self.pc_tx.clone(),
            }))
            .with_udp_addrs(self.config.udp_addrs.clone())
            .build()
            .await
            .map_err(|_| CallError::Failed)?;
        let pc: Arc<dyn PeerConnection> = Arc::new(pc);

        let track = Arc::new(
            TrackLocalStaticSample::new(MediaStreamTrack::new(
                "aster".to_string(),
                "voice".to_string(),
                "voice".to_string(),
                RtpCodecKind::Audio,
                vec![RTCRtpEncodingParameters {
                    rtp_coding_parameters: RTCRtpCodingParameters {
                        ssrc: Some(fastrand::u32(1..)),
                        ..Default::default()
                    },
                    codec: RTCRtpCodec {
                        mime_type: MIME_TYPE_OPUS.to_string(),
                        clock_rate: 48_000,
                        channels: 2,
                        sdp_fmtp_line: "minptime=10;useinbandfec=1".to_string(),
                        rtcp_feedback: vec![],
                    },
                    ..Default::default()
                }],
            ))
            .map_err(|_| CallError::Failed)?,
        );
        let sender = match pc.add_track(track.clone() as Arc<dyn TrackLocal>).await {
            Ok(sender) => sender,
            Err(_) => {
                let _ = pc.close().await;
                return Err(CallError::Failed);
            }
        };

        let flags = Arc::new(Flags::default());
        flags.muted.store(self.muted, Ordering::Relaxed);
        flags
            .vad_threshold_dbfs
            .store(self.settings.vad_threshold_dbfs, Ordering::Relaxed);
        Ok(Session {
            generation,
            pc,
            track,
            sender,
            remote_set: false,
            pending_candidates: Vec::new(),
            offer_session_id: None,
            jitter: Arc::new(Mutex::new(JitterBuffer::new())),
            flags,
            receiver: None,
            media: None,
        })
    }

    async fn start_media(&mut self, session: &mut Session) {
        let payload_type = session
            .sender
            .get_parameters()
            .await
            .ok()
            .and_then(|params| params.rtp_parameters.codecs.first().map(|c| c.payload_type));
        let ssrc = session.track.ssrcs().await.first().copied();

        // The call takes over the microphone; its DSP reports levels too.
        self.idle_monitor = None;
        let (frames_tx, mut frames_rx) = mpsc::channel::<Frame>(25);
        let dsp = self.spawn_dsp(Some(frames_tx));
        let capture = open_capture(
            &*self.backend,
            self.settings.input_device.as_deref(),
            dsp.sender(),
            &self.events,
        );

        let track = session.track.clone();
        let flags = session.flags.clone();
        let encoder = tokio::spawn(async move {
            let (Some(payload_type), Some(ssrc)) = (payload_type, ssrc) else {
                return;
            };
            let mut encoder = Encoder::new();
            let mut vad = Vad::default();
            while let Some(frame) = frames_rx.recv().await {
                vad.set_threshold(flags.vad_threshold_dbfs.load(Ordering::Relaxed) as f32);
                let speaking = vad.update(&frame);
                let muted = flags.muted.load(Ordering::Relaxed);
                flags
                    .local_speaking
                    .store(speaking && !muted, Ordering::Relaxed);
                let input = if muted || !speaking { &SILENCE } else { &frame };
                let sample = Sample {
                    data: Bytes::from(encoder.encode(input)),
                    duration: Duration::from_millis((FRAME * 1000 / 48_000) as u64),
                    ..Default::default()
                };
                let _ = track
                    .sample_writer(ssrc, payload_type)
                    .write_sample(&sample)
                    .await;
            }
        });

        let playback = open_playback(
            &*self.backend,
            self.settings.output_device.as_deref(),
            session.jitter.clone(),
            session.flags.clone(),
            dsp.sender(),
            &self.events,
        );

        session.media = Some(Media {
            capture: Capture {
                stream: capture,
                dsp,
            },
            playback,
            encoder,
        });
    }

    /// Processing for one microphone. Processed frames go to `frames` when a
    /// call is sending, and to `InputLevel` while the monitor is on.
    fn spawn_dsp(&self, frames: Option<mpsc::Sender<Frame>>) -> Dsp {
        let monitor = self.monitor.clone();
        let events = self.events.clone();
        let mut meter = LevelMeter::default();
        Dsp::spawn(ProcessingConfig::from(&self.settings), move |frame| {
            if let Some(frames) = &frames {
                let _ = frames.try_send(*frame);
            }
            if let Some(dbfs) = meter.push(frame) {
                if monitor.load(Ordering::Relaxed) {
                    let _ = events.send(VoiceEvent::InputLevel { dbfs });
                }
            }
        })
    }

    /// Opens or closes the monitor's own microphone: it runs only while the
    /// monitor is on and no call has media.
    fn sync_monitor(&mut self) {
        let in_call = matches!(
            &self.phase,
            Phase::Connected { session, .. } if session.media.is_some()
        );
        if !self.monitor.load(Ordering::Relaxed) || in_call {
            self.idle_monitor = None;
        } else if self.idle_monitor.is_none() {
            let dsp = self.spawn_dsp(None);
            let stream = open_capture(
                &*self.backend,
                self.settings.input_device.as_deref(),
                dsp.sender(),
                &self.events,
            );
            self.idle_monitor = Some(Capture { stream, dsp });
        }
    }

    fn apply_settings(&mut self, settings: VoiceSettings) {
        let previous = std::mem::replace(&mut self.settings, settings);
        if let Some(session) = self.phase.session_mut() {
            session
                .flags
                .vad_threshold_dbfs
                .store(self.settings.vad_threshold_dbfs, Ordering::Relaxed);
        }

        let processing = ProcessingConfig::from(&self.settings);
        let capture = match &mut self.phase {
            Phase::Connected { session, .. } => {
                session.media.as_mut().map(|media| &mut media.capture)
            }
            _ => None,
        }
        .or(self.idle_monitor.as_mut());
        if let Some(capture) = capture {
            if ProcessingConfig::from(&previous) != processing {
                capture.dsp.configure(processing);
            }
            if previous.input_device != self.settings.input_device {
                // Release the old device before opening the new one.
                capture.stream = None;
                capture.stream = open_capture(
                    &*self.backend,
                    self.settings.input_device.as_deref(),
                    capture.dsp.sender(),
                    &self.events,
                );
            }
        }

        if previous.output_device != self.settings.output_device {
            if let Phase::Connected { session, .. } = &mut self.phase {
                if let Some(media) = session.media.as_mut() {
                    media.playback = None;
                    media.playback = open_playback(
                        &*self.backend,
                        self.settings.output_device.as_deref(),
                        session.jitter.clone(),
                        session.flags.clone(),
                        media.capture.dsp.sender(),
                        &self.events,
                    );
                }
            }
        }
    }

    fn refresh_audio(&mut self) {
        let status = match &mut self.phase {
            Phase::Connected { session, .. } => AudioStatus {
                muted: self.muted,
                local_speaking: session.flags.local_speaking.load(Ordering::Relaxed),
                remote_speaking: session.flags.remote_speaking.load(Ordering::Relaxed),
            },
            _ => AudioStatus {
                muted: self.muted,
                ..AudioStatus::default()
            },
        };
        if status != self.audio {
            self.audio = status;
            self.emit(VoiceEvent::Audio(status));
        }
    }

    async fn send_call_end(&self) {
        let _ = self.signaling.send_signal(SignalPayload::CallEnd).await;
    }

    fn end(&mut self, reason: EndReason) {
        self.teardown();
        self.muted = false;
        self.refresh_audio();
        self.sync_monitor();
        self.set_state(CallState::Ended { reason });
        self.set_state(CallState::Idle);
    }

    fn teardown(&mut self) {
        match std::mem::replace(&mut self.phase, Phase::Idle) {
            Phase::Calling { session, .. }
            | Phase::Connecting { session, .. }
            | Phase::Connected { session, .. } => session.close(),
            Phase::Idle | Phase::Ringing { .. } => {}
        }
    }

    fn set_state(&self, state: CallState) {
        self.state.send_replace(state.clone());
        self.emit(VoiceEvent::State(state));
    }

    fn emit(&self, event: VoiceEvent) {
        let _ = self.events.send(event);
    }
}

fn open_capture(
    backend: &dyn AudioBackend,
    device: Option<&str>,
    dsp: std::sync::mpsc::SyncSender<DspInput>,
    events: &mpsc::UnboundedSender<VoiceEvent>,
) -> Option<Box<dyn AudioStream>> {
    let sink = Box::new(move |frame: &Frame| {
        // Dropping a frame beats blocking the audio thread.
        let _ = dsp.try_send(DspInput::Capture(*frame));
    });
    opened(
        backend.capture(device, sink),
        AudioDirection::Capture,
        events,
    )
}

fn open_playback(
    backend: &dyn AudioBackend,
    device: Option<&str>,
    jitter: Arc<Mutex<JitterBuffer>>,
    flags: Arc<Flags>,
    render: std::sync::mpsc::SyncSender<DspInput>,
    events: &mpsc::UnboundedSender<VoiceEvent>,
) -> Option<Box<dyn AudioStream>> {
    let mut vad = Vad::default();
    let source = Box::new(move || {
        let frame = jitter.lock().unwrap().pop();
        // What reaches the speaker is the echo reference.
        let _ = render.try_send(DspInput::Render(frame));
        flags
            .remote_speaking
            .store(vad.update(&frame), Ordering::Relaxed);
        frame
    });
    opened(
        backend.playback(device, source),
        AudioDirection::Playback,
        events,
    )
}

/// Reports fallbacks and failures; a missing device never ends the call.
fn opened(
    result: Result<super::audio::Opened, String>,
    direction: AudioDirection,
    events: &mpsc::UnboundedSender<VoiceEvent>,
) -> Option<Box<dyn AudioStream>> {
    let (stream, message) = match result {
        Ok(opened) => (Some(opened.stream), opened.fallback),
        Err(message) => (None, Some(message)),
    };
    if let Some(message) = message {
        let _ = events.send(VoiceEvent::AudioError { direction, message });
    }
    stream
}

struct PcHandler {
    generation: u64,
    events: mpsc::UnboundedSender<(u64, PcEvent)>,
}

#[async_trait::async_trait]
impl PeerConnectionEventHandler for PcHandler {
    async fn on_ice_candidate(&self, event: RTCPeerConnectionIceEvent) {
        if let Ok(init) = event.candidate.to_json() {
            let _ = self
                .events
                .send((self.generation, PcEvent::Candidate(init)));
        }
    }

    async fn on_connection_state_change(&self, state: RTCPeerConnectionState) {
        let _ = self.events.send((self.generation, PcEvent::State(state)));
    }

    async fn on_track(&self, track: Arc<dyn TrackRemote>) {
        let _ = self.events.send((self.generation, PcEvent::Track(track)));
    }
}

async fn sleep_until(deadline: Option<Instant>) {
    match deadline {
        Some(deadline) => tokio::time::sleep_until(deadline).await,
        None => std::future::pending().await,
    }
}

/// Session id from the SDP `o=` line, used to break call glare.
fn sdp_session_id(sdp: &str) -> Option<u64> {
    sdp.lines()
        .find_map(|line| line.strip_prefix("o="))
        .and_then(|origin| origin.split_whitespace().nth(1))
        .and_then(|id| id.parse().ok())
}

fn now_ms() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .expect("clock is after 1970")
        .as_millis() as i64
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_sdp_session_id() {
        let sdp = "v=0\r\no=- 4611731400430051336 2 IN IP4 127.0.0.1\r\ns=-\r\n";
        assert_eq!(sdp_session_id(sdp), Some(4_611_731_400_430_051_336));
        assert_eq!(sdp_session_id("v=0\r\n"), None);
    }

    #[test]
    fn call_state_json_matches_spec() {
        for (state, json) in [
            (CallState::Idle, r#"{"state":"idle"}"#),
            (CallState::Calling, r#"{"state":"calling"}"#),
            (
                CallState::Connected { since: 5 },
                r#"{"state":"connected","since":5}"#,
            ),
            (
                CallState::Ended {
                    reason: EndReason::RemoteHangup,
                },
                r#"{"state":"ended","reason":"remote_hangup"}"#,
            ),
        ] {
            assert_eq!(serde_json::to_string(&state).unwrap(), json);
        }
    }
}
