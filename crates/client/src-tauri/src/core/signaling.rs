//! Connection to `signaling-server`: handshake, state machine and reconnects.
//! See `specs/0009-client-core.md`.

use std::collections::VecDeque;
use std::future::Future;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use futures_util::{SinkExt, StreamExt};
use protocol::{
    ClientMessage, ErrorCode, IceServer, RejectReason, ServerMessage, SignalPayload,
    PROTOCOL_VERSION,
};
use serde::Serialize;
use tokio::net::TcpStream;
use tokio::sync::{mpsc, oneshot, watch};
use tokio::time::timeout;
use tokio_tungstenite::tungstenite::Message;
use tokio_tungstenite::{connect_async, MaybeTlsStream, WebSocketStream};
use uuid::Uuid;

use super::settings::Settings;

type Ws = WebSocketStream<MaybeTlsStream<TcpStream>>;

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(tag = "state", rename_all = "snake_case")]
pub enum ConnectionState {
    NotConfigured,
    Connecting { attempt: u32 },
    Connected { user_id: String, peer_online: bool },
    Reconnecting { attempt: u32, retry_in_ms: u64 },
    Disconnected,
    Failed { reason: FailReason },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum FailReason {
    InvalidToken,
    UnsupportedProtocolVersion,
    SessionReplaced,
    /// A rejection this client version does not know.
    Other,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum AckStatus {
    Delivered,
    Queued,
}

/// Everything the core reports outward; the Tauri layer turns these into events.
#[derive(Debug, Clone, PartialEq)]
pub enum CoreEvent {
    State(ConnectionState),
    PeerStatus {
        online: bool,
    },
    ChatMessage {
        id: Uuid,
        from: String,
        body: String,
        sent_at: i64,
    },
    ChatAck {
        id: Uuid,
        status: AckStatus,
    },
    ChatRejected {
        id: Option<Uuid>,
        code: ErrorCode,
        message: String,
    },
    /// Consumed by the voice module, never forwarded to the UI.
    Signal(SignalPayload),
    ServerError {
        code: ErrorCode,
        message: String,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum SendError {
    NotConnected,
    Io,
}

#[derive(Debug, Clone, Copy)]
pub struct Timing {
    pub backoff_base: Duration,
    pub backoff_max: Duration,
    pub connect_timeout: Duration,
    pub idle_timeout: Duration,
    /// Own WS pings keep the server's idle timeout satisfied regardless of
    /// whether its pings get answered in time.
    pub ping_interval: Duration,
}

impl Default for Timing {
    fn default() -> Self {
        Self {
            backoff_base: Duration::from_secs(1),
            backoff_max: Duration::from_secs(30),
            connect_timeout: Duration::from_secs(10),
            idle_timeout: Duration::from_secs(60),
            ping_interval: Duration::from_secs(20),
        }
    }
}

enum Command {
    Connect(Settings),
    Disconnect,
    SendChat {
        id: Uuid,
        body: String,
        sent_at: i64,
        reply: oneshot::Sender<Result<(), SendError>>,
    },
    SendSignal {
        payload: SignalPayload,
        reply: oneshot::Sender<Result<(), SendError>>,
    },
}

#[derive(Clone)]
pub struct SignalingHandle {
    commands: mpsc::UnboundedSender<Command>,
    state: watch::Receiver<ConnectionState>,
    ice_servers: Arc<Mutex<Vec<IceServer>>>,
}

impl SignalingHandle {
    /// Starts (or restarts with new settings) the connection.
    pub fn connect(&self, settings: Settings) {
        let _ = self.commands.send(Command::Connect(settings));
    }

    pub fn disconnect(&self) {
        let _ = self.commands.send(Command::Disconnect);
    }

    pub fn state(&self) -> ConnectionState {
        self.state.borrow().clone()
    }

    /// ICE servers from the last `Welcome`.
    pub fn ice_servers(&self) -> Vec<IceServer> {
        self.ice_servers.lock().unwrap().clone()
    }

    pub async fn send_chat(&self, id: Uuid, body: String, sent_at: i64) -> Result<(), SendError> {
        let (reply, rx) = oneshot::channel();
        self.commands
            .send(Command::SendChat {
                id,
                body,
                sent_at,
                reply,
            })
            .map_err(|_| SendError::NotConnected)?;
        rx.await.unwrap_or(Err(SendError::NotConnected))
    }

    pub async fn send_signal(&self, payload: SignalPayload) -> Result<(), SendError> {
        let (reply, rx) = oneshot::channel();
        self.commands
            .send(Command::SendSignal { payload, reply })
            .map_err(|_| SendError::NotConnected)?;
        rx.await.unwrap_or(Err(SendError::NotConnected))
    }
}

/// Creates the handle and the actor future; the caller spawns the future on
/// its runtime. The actor stops once every handle is dropped.
pub fn signaling(
    events: mpsc::UnboundedSender<CoreEvent>,
    timing: Timing,
) -> (SignalingHandle, impl Future<Output = ()> + Send + 'static) {
    let (commands_tx, commands) = mpsc::unbounded_channel();
    let (state_tx, state) = watch::channel(ConnectionState::NotConfigured);
    let ice_servers = Arc::new(Mutex::new(Vec::new()));
    let handle = SignalingHandle {
        commands: commands_tx,
        state,
        ice_servers: ice_servers.clone(),
    };
    let actor = Actor {
        commands,
        events,
        state: state_tx,
        ice_servers,
        timing,
    };
    (handle, actor.run())
}

struct Actor {
    commands: mpsc::UnboundedReceiver<Command>,
    events: mpsc::UnboundedSender<CoreEvent>,
    state: watch::Sender<ConnectionState>,
    ice_servers: Arc<Mutex<Vec<IceServer>>>,
    timing: Timing,
}

/// Why the actor stopped waiting on a connection-related future.
enum Interrupt {
    Disconnect,
    Reconfigure(Settings),
    Shutdown,
}

enum SessionEnd {
    Lost { was_connected: bool },
    Failed(FailReason),
    Interrupted(Interrupt),
}

impl Actor {
    async fn run(mut self) {
        let mut next: Option<Settings> = None;
        loop {
            let settings = match next.take() {
                Some(settings) => settings,
                None => match self.commands.recv().await {
                    None => return,
                    Some(Command::Connect(settings)) => settings,
                    Some(Command::Disconnect) => continue,
                    Some(Command::SendChat { reply, .. }) => {
                        let _ = reply.send(Err(SendError::NotConnected));
                        continue;
                    }
                    Some(Command::SendSignal { reply, .. }) => {
                        let _ = reply.send(Err(SendError::NotConnected));
                        continue;
                    }
                },
            };

            match self.connection_loop(&settings).await {
                Interrupt::Disconnect => self.set_state(ConnectionState::Disconnected),
                Interrupt::Reconfigure(settings) => next = Some(settings),
                Interrupt::Shutdown => return,
            }
        }
    }

    /// Keeps a connection up until the user disconnects, reconfigures, or the
    /// server gives a final answer (which leaves the actor idle in `Failed`).
    async fn connection_loop(&mut self, settings: &Settings) -> Interrupt {
        let mut failures: u32 = 0;
        loop {
            self.set_state(ConnectionState::Connecting {
                attempt: failures + 1,
            });
            match self.session(settings).await {
                SessionEnd::Interrupted(interrupt) => return interrupt,
                SessionEnd::Failed(reason) => {
                    self.set_state(ConnectionState::Failed { reason });
                    return self.wait_while_failed().await;
                }
                SessionEnd::Lost { was_connected } => {
                    failures = if was_connected { 1 } else { failures + 1 };
                }
            }

            let delay = backoff(&self.timing, failures);
            self.set_state(ConnectionState::Reconnecting {
                attempt: failures,
                retry_in_ms: delay.as_millis() as u64,
            });
            if let Err(interrupt) = self.interruptible(tokio::time::sleep(delay)).await {
                return interrupt;
            }
        }
    }

    async fn wait_while_failed(&mut self) -> Interrupt {
        match self.interruptible(std::future::pending::<()>()).await {
            Err(interrupt) => interrupt,
            Ok(()) => unreachable!(),
        }
    }

    async fn session(&mut self, settings: &Settings) -> SessionEnd {
        let connect_timeout = self.timing.connect_timeout;
        let handshake = async {
            let (mut ws, _) = timeout(connect_timeout, connect_async(settings.server_url.as_str()))
                .await
                .ok()?
                .ok()?;
            let hello = ClientMessage::Hello {
                protocol_version: PROTOCOL_VERSION,
                auth_token: settings.token.clone(),
            };
            ws.send(Message::text(serde_json::to_string(&hello).ok()?))
                .await
                .ok()?;
            let reply = timeout(connect_timeout, next_server_message(&mut ws))
                .await
                .ok()??;
            Some((ws, reply))
        };

        let (mut ws, reply) = match self.interruptible(handshake).await {
            Err(interrupt) => return SessionEnd::Interrupted(interrupt),
            Ok(None) => {
                return SessionEnd::Lost {
                    was_connected: false,
                };
            }
            Ok(Some(connected)) => connected,
        };

        match reply {
            ServerMessage::Welcome {
                user_id,
                peer_online,
                ice_servers,
            } => {
                *self.ice_servers.lock().unwrap() = ice_servers;
                self.set_state(ConnectionState::Connected {
                    user_id: user_id.0,
                    peer_online,
                });
            }
            ServerMessage::Rejected { reason } => {
                return SessionEnd::Failed(match reason {
                    RejectReason::InvalidToken => FailReason::InvalidToken,
                    RejectReason::UnsupportedProtocolVersion => {
                        FailReason::UnsupportedProtocolVersion
                    }
                    RejectReason::Unknown => FailReason::Other,
                });
            }
            _ => {
                return SessionEnd::Lost {
                    was_connected: false,
                };
            }
        }

        let end = self.connected(&mut ws).await;
        if matches!(end, SessionEnd::Interrupted(_)) {
            let _ = ws.close(None).await;
        }
        end
    }

    async fn connected(&mut self, ws: &mut Ws) -> SessionEnd {
        // The server answers every chat message in order, so the oldest
        // unanswered id is the one an immediate ack or `queue_full` refers to.
        let mut pending: VecDeque<Uuid> = VecDeque::new();
        let lost = SessionEnd::Lost {
            was_connected: true,
        };
        let period = self.timing.ping_interval;
        let mut ping = tokio::time::interval_at(tokio::time::Instant::now() + period, period);

        loop {
            tokio::select! {
                _ = ping.tick() => {
                    if ws.send(Message::Ping(Default::default())).await.is_err() {
                        return lost;
                    }
                }
                frame = timeout(self.timing.idle_timeout, ws.next()) => {
                    let text = match frame {
                        Ok(Some(Ok(Message::Text(text)))) => text,
                        Ok(Some(Ok(Message::Ping(_) | Message::Pong(_) | Message::Frame(_)))) => continue,
                        Ok(Some(Ok(Message::Binary(_)))) => continue,
                        _ => return lost,
                    };
                    let Ok(msg) = serde_json::from_str::<ServerMessage>(&text) else {
                        continue;
                    };
                    if let Some(reason) = self.handle_server_message(msg, &mut pending) {
                        return SessionEnd::Failed(reason);
                    }
                }
                command = self.commands.recv() => match command {
                    None => return SessionEnd::Interrupted(Interrupt::Shutdown),
                    Some(Command::Disconnect) => return SessionEnd::Interrupted(Interrupt::Disconnect),
                    Some(Command::Connect(settings)) => {
                        return SessionEnd::Interrupted(Interrupt::Reconfigure(settings));
                    }
                    Some(Command::SendChat { id, body, sent_at, reply }) => {
                        let msg = ClientMessage::ChatMessage { id, body, sent_at };
                        let json = serde_json::to_string(&msg).expect("ClientMessage serializes");
                        if ws.send(Message::text(json)).await.is_err() {
                            let _ = reply.send(Err(SendError::Io));
                            return lost;
                        }
                        pending.push_back(id);
                        let _ = reply.send(Ok(()));
                    }
                    Some(Command::SendSignal { payload, reply }) => {
                        let msg = ClientMessage::Signal { payload };
                        let json = serde_json::to_string(&msg).expect("ClientMessage serializes");
                        if ws.send(Message::text(json)).await.is_err() {
                            let _ = reply.send(Err(SendError::Io));
                            return lost;
                        }
                        let _ = reply.send(Ok(()));
                    }
                },
            }
        }
    }

    /// Returns a fail reason if the session must end without reconnecting.
    fn handle_server_message(
        &mut self,
        msg: ServerMessage,
        pending: &mut VecDeque<Uuid>,
    ) -> Option<FailReason> {
        let event = match msg {
            ServerMessage::PeerStatus { online } => {
                // Clone first: holding the watch borrow across `set_state` deadlocks.
                let current = self.state.borrow().clone();
                if let ConnectionState::Connected { user_id, .. } = current {
                    self.set_state(ConnectionState::Connected {
                        user_id,
                        peer_online: online,
                    });
                }
                CoreEvent::PeerStatus { online }
            }
            ServerMessage::Signal { payload } => CoreEvent::Signal(payload),
            ServerMessage::ChatMessage {
                id,
                from,
                body,
                sent_at,
            } => CoreEvent::ChatMessage {
                id,
                from: from.0,
                body,
                sent_at,
            },
            ServerMessage::ChatDelivered { id } => {
                ack(pending, id);
                CoreEvent::ChatAck {
                    id,
                    status: AckStatus::Delivered,
                }
            }
            ServerMessage::ChatQueued { id } => {
                ack(pending, id);
                CoreEvent::ChatAck {
                    id,
                    status: AckStatus::Queued,
                }
            }
            ServerMessage::Error {
                code: ErrorCode::SessionReplaced,
                ..
            } => return Some(FailReason::SessionReplaced),
            ServerMessage::Error {
                code: code @ ErrorCode::QueueFull,
                message,
            } => CoreEvent::ChatRejected {
                id: pending.pop_front(),
                code,
                message,
            },
            ServerMessage::Error { code, message } => CoreEvent::ServerError { code, message },
            ServerMessage::Welcome { .. } | ServerMessage::Rejected { .. } => return None,
        };
        let _ = self.events.send(event);
        None
    }

    /// Runs `fut` while serving commands that don't need a live connection.
    async fn interruptible<F: Future>(&mut self, fut: F) -> Result<F::Output, Interrupt> {
        tokio::pin!(fut);
        loop {
            tokio::select! {
                out = &mut fut => return Ok(out),
                command = self.commands.recv() => match command {
                    None => return Err(Interrupt::Shutdown),
                    Some(Command::Disconnect) => return Err(Interrupt::Disconnect),
                    Some(Command::Connect(settings)) => return Err(Interrupt::Reconfigure(settings)),
                    Some(Command::SendChat { reply, .. }) => {
                        let _ = reply.send(Err(SendError::NotConnected));
                    }
                    Some(Command::SendSignal { reply, .. }) => {
                        let _ = reply.send(Err(SendError::NotConnected));
                    }
                },
            }
        }
    }

    fn set_state(&self, state: ConnectionState) {
        if *self.state.borrow() == state {
            return;
        }
        self.state.send_replace(state.clone());
        let _ = self.events.send(CoreEvent::State(state));
    }
}

fn ack(pending: &mut VecDeque<Uuid>, id: Uuid) {
    // Delayed `chat_delivered` for a previously queued message is not pending.
    if pending.front() == Some(&id) {
        pending.pop_front();
    }
}

async fn next_server_message(ws: &mut Ws) -> Option<ServerMessage> {
    loop {
        match ws.next().await? {
            Ok(Message::Text(text)) => return serde_json::from_str(&text).ok(),
            Ok(Message::Close(_)) | Err(_) => return None,
            Ok(_) => continue,
        }
    }
}

/// `failures` ≥ 1. Exponential from `backoff_base`, capped, ±20% jitter.
fn backoff(timing: &Timing, failures: u32) -> Duration {
    let exp = timing
        .backoff_base
        .saturating_mul(1 << failures.saturating_sub(1).min(16));
    let capped = exp.min(timing.backoff_max);
    capped.mul_f64(0.8 + fastrand::f64() * 0.4)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn backoff_grows_and_caps_with_jitter() {
        let timing = Timing::default();
        for (failures, expected) in [
            (1, 1.0),
            (2, 2.0),
            (3, 4.0),
            (5, 16.0),
            (6, 30.0),
            (40, 30.0),
        ] {
            let secs = backoff(&timing, failures).as_secs_f64();
            assert!(
                secs >= expected * 0.8 && secs <= expected * 1.2,
                "failures={failures}: {secs}s, expected ~{expected}s"
            );
        }
    }

    #[test]
    fn connection_state_json_matches_spec() {
        let cases = [
            (
                ConnectionState::NotConfigured,
                r#"{"state":"not_configured"}"#,
            ),
            (
                ConnectionState::Connecting { attempt: 2 },
                r#"{"state":"connecting","attempt":2}"#,
            ),
            (
                ConnectionState::Connected {
                    user_id: "alice".into(),
                    peer_online: true,
                },
                r#"{"state":"connected","user_id":"alice","peer_online":true}"#,
            ),
            (
                ConnectionState::Reconnecting {
                    attempt: 1,
                    retry_in_ms: 950,
                },
                r#"{"state":"reconnecting","attempt":1,"retry_in_ms":950}"#,
            ),
            (ConnectionState::Disconnected, r#"{"state":"disconnected"}"#),
            (
                ConnectionState::Failed {
                    reason: FailReason::SessionReplaced,
                },
                r#"{"state":"failed","reason":"session_replaced"}"#,
            ),
        ];
        for (state, json) in cases {
            assert_eq!(serde_json::to_string(&state).unwrap(), json);
        }
    }
}
