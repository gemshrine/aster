use std::sync::Arc;
use std::time::Duration;

use axum::extract::State;
use axum::extract::ws::{Message, WebSocket, WebSocketUpgrade};
use axum::response::Response;
use futures_util::stream::SplitStream;
use futures_util::{SinkExt, StreamExt};
use protocol::{ClientMessage, ErrorCode, PROTOCOL_VERSION, RejectReason, ServerMessage, UserId};
use tokio::sync::mpsc::{self, UnboundedSender};
use tokio::time::{Instant, timeout, timeout_at};

use crate::hub::{ChatMessage, ChatOutcome, Hub, Outbound};

const MAX_MESSAGE_SIZE: usize = 64 * 1024;
const HANDSHAKE_TIMEOUT: Duration = Duration::from_secs(10);
const PING_INTERVAL: Duration = Duration::from_secs(20);
/// A healthy client answers pings, so silence this long means a dead link.
const IDLE_TIMEOUT: Duration = Duration::from_secs(60);

pub async fn ws_handler(ws: WebSocketUpgrade, State(hub): State<Arc<Hub>>) -> Response {
    ws.max_message_size(MAX_MESSAGE_SIZE)
        .on_upgrade(move |socket| handle_socket(socket, hub))
}

async fn handle_socket(socket: WebSocket, hub: Arc<Hub>) {
    let (mut sink, mut stream) = socket.split();
    let (tx, mut rx) = mpsc::unbounded_channel::<Outbound>();

    let writer = tokio::spawn(async move {
        let mut ping = tokio::time::interval_at(Instant::now() + PING_INTERVAL, PING_INTERVAL);
        loop {
            tokio::select! {
                out = rx.recv() => match out {
                    Some(Outbound::Message(msg)) => {
                        let json = serde_json::to_string(&msg).expect("ServerMessage serializes");
                        if sink.send(Message::Text(json.into())).await.is_err() {
                            break;
                        }
                    }
                    Some(Outbound::Close) | None => {
                        let _ = sink.send(Message::Close(None)).await;
                        break;
                    }
                },
                _ = ping.tick() => {
                    if sink.send(Message::Ping(Default::default())).await.is_err() {
                        break;
                    }
                }
            }
        }
    });

    if let Some(user) = handshake(&mut stream, &tx, &hub).await {
        let conn_id = hub.connect(&user, tx.clone());
        tracing::info!(user = %user.0, conn_id, "session started");
        serve(&mut stream, &tx, &hub, &user).await;
        hub.disconnect(&user, conn_id);
        tracing::info!(user = %user.0, conn_id, "session ended");
    }

    let _ = tx.send(Outbound::Close);
    drop(tx);
    let _ = writer.await;
}

/// Waits for a valid `Hello`. Returns `None` if the connection should be closed.
async fn handshake(
    stream: &mut SplitStream<WebSocket>,
    tx: &UnboundedSender<Outbound>,
    hub: &Hub,
) -> Option<UserId> {
    let deadline = Instant::now() + HANDSHAKE_TIMEOUT;
    loop {
        let Ok(Some(Ok(frame))) = timeout_at(deadline, stream.next()).await else {
            return None;
        };
        let msg = match parse(frame, tx)? {
            Some(msg) => msg,
            None => continue,
        };
        let ClientMessage::Hello {
            protocol_version,
            auth_token,
        } = msg
        else {
            send_error(tx, ErrorCode::NotAuthenticated, "send hello first");
            continue;
        };

        let reject = |reason| {
            let _ = tx.send(Outbound::Message(ServerMessage::Rejected { reason }));
            None
        };
        if protocol_version != PROTOCOL_VERSION {
            return reject(RejectReason::UnsupportedProtocolVersion);
        }
        return match hub.authenticate(&auth_token) {
            Some(user) => Some(user),
            None => {
                tracing::warn!("rejected connection with invalid token");
                reject(RejectReason::InvalidToken)
            }
        };
    }
}

async fn serve(
    stream: &mut SplitStream<WebSocket>,
    tx: &UnboundedSender<Outbound>,
    hub: &Hub,
    user: &UserId,
) {
    loop {
        let Ok(Some(Ok(frame))) = timeout(IDLE_TIMEOUT, stream.next()).await else {
            return;
        };
        let msg = match parse(frame, tx) {
            None => return,
            Some(None) => continue,
            Some(Some(msg)) => msg,
        };
        match msg {
            ClientMessage::Hello { .. } => {
                send_error(tx, ErrorCode::MalformedMessage, "already authenticated")
            }
            ClientMessage::Signal { payload } => {
                if !hub.relay_signal(user, payload) {
                    send_error(tx, ErrorCode::PeerOffline, "the other user is offline");
                }
            }
            ClientMessage::ChatMessage { id, body, sent_at } => {
                let ack = match hub.send_chat(user, ChatMessage { id, body, sent_at }) {
                    ChatOutcome::Delivered => ServerMessage::ChatDelivered { id },
                    ChatOutcome::Queued => ServerMessage::ChatQueued { id },
                    ChatOutcome::QueueFull => ServerMessage::Error {
                        code: ErrorCode::QueueFull,
                        message: format!("offline queue is full, message {id} dropped"),
                    },
                };
                let _ = tx.send(Outbound::Message(ack));
            }
        }
    }
}

/// `None` — the peer closed the connection; `Some(None)` — control frame or
/// malformed input (already answered), nothing to handle.
fn parse(frame: Message, tx: &UnboundedSender<Outbound>) -> Option<Option<ClientMessage>> {
    match frame {
        Message::Text(text) => match serde_json::from_str(&text) {
            Ok(msg) => Some(Some(msg)),
            Err(err) => {
                send_error(tx, ErrorCode::MalformedMessage, &err.to_string());
                Some(None)
            }
        },
        Message::Binary(_) => {
            send_error(
                tx,
                ErrorCode::MalformedMessage,
                "binary frames are not supported",
            );
            Some(None)
        }
        Message::Ping(_) | Message::Pong(_) => Some(None),
        Message::Close(_) => None,
    }
}

fn send_error(tx: &UnboundedSender<Outbound>, code: ErrorCode, message: &str) {
    let _ = tx.send(Outbound::Message(ServerMessage::Error {
        code,
        message: message.to_string(),
    }));
}
