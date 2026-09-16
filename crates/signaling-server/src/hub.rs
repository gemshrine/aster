use std::collections::{HashMap, VecDeque};
use std::sync::Mutex;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, Instant, SystemTime};

use protocol::{ErrorCode, ServerMessage, SignalPayload, UserId};
use tokio::sync::mpsc::UnboundedSender;
use uuid::Uuid;

use crate::config::Users;
use crate::turn::TurnConfig;

/// Instructions for a connection's writer task.
#[derive(Debug)]
pub enum Outbound {
    Message(ServerMessage),
    Close,
}

/// Limits for chat messages waiting for an offline recipient, see `specs/0004-text-chat.md`.
#[derive(Debug, Clone, Copy)]
pub struct QueueLimits {
    pub ttl: Duration,
    pub capacity: usize,
}

impl Default for QueueLimits {
    fn default() -> Self {
        Self {
            ttl: Duration::from_secs(7 * 24 * 60 * 60),
            capacity: 500,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ChatOutcome {
    Delivered,
    Queued,
    QueueFull,
}

pub struct ChatMessage {
    pub id: Uuid,
    pub body: String,
    pub sent_at: i64,
}

struct Session {
    conn_id: u64,
    tx: UnboundedSender<Outbound>,
}

struct QueuedChat {
    message: ChatMessage,
    queued_at: Instant,
}

#[derive(Default)]
struct State {
    sessions: HashMap<UserId, Session>,
    /// Keyed by recipient.
    offline: HashMap<UserId, VecDeque<QueuedChat>>,
}

/// Tracks the (at most two) live sessions and routes messages between them.
pub struct Hub {
    users: Users,
    limits: QueueLimits,
    turn: Option<TurnConfig>,
    state: Mutex<State>,
    next_conn_id: AtomicU64,
}

impl Hub {
    pub fn new(users: Users) -> Self {
        Self::with_limits(users, QueueLimits::default())
    }

    pub fn with_limits(users: Users, limits: QueueLimits) -> Self {
        Self {
            users,
            limits,
            turn: None,
            state: Mutex::new(State::default()),
            next_conn_id: AtomicU64::new(1),
        }
    }

    pub fn with_turn(mut self, turn: TurnConfig) -> Self {
        self.turn = Some(turn);
        self
    }

    pub fn authenticate(&self, token: &str) -> Option<UserId> {
        self.users.authenticate(token)
    }

    /// Registers a session for `user`, sends it `Welcome` followed by any chat
    /// messages queued while it was offline, and returns its connection id.
    /// An existing session for the same user is evicted.
    pub fn connect(&self, user: &UserId, tx: UnboundedSender<Outbound>) -> u64 {
        let conn_id = self.next_conn_id.fetch_add(1, Ordering::Relaxed);
        let peer = self.users.peer_of(user);
        let mut state = self.state.lock().unwrap();

        let _ = tx.send(Outbound::Message(ServerMessage::Welcome {
            user_id: user.clone(),
            peer_online: state.sessions.contains_key(peer),
            ice_servers: self
                .turn
                .as_ref()
                .map(|turn| turn.ice_servers(user, SystemTime::now()))
                .unwrap_or_default(),
        }));

        let now = Instant::now();
        let queued = state.offline.remove(user).unwrap_or_default();
        for QueuedChat { message, .. } in queued
            .into_iter()
            .filter(|q| now.duration_since(q.queued_at) < self.limits.ttl)
        {
            let id = message.id;
            let _ = tx.send(Outbound::Message(ServerMessage::ChatMessage {
                id,
                from: peer.clone(),
                body: message.body,
                sent_at: message.sent_at,
            }));
            state.send_to(peer, ServerMessage::ChatDelivered { id });
        }

        let previous = state.sessions.insert(user.clone(), Session { conn_id, tx });
        match previous {
            Some(old) => {
                let _ = old.tx.send(Outbound::Message(ServerMessage::Error {
                    code: ErrorCode::SessionReplaced,
                    message: "another connection authenticated as this user".into(),
                }));
                let _ = old.tx.send(Outbound::Close);
            }
            None => {
                state.send_to(peer, ServerMessage::PeerStatus { online: true });
            }
        }
        conn_id
    }

    /// Removes the session if it is still the current one for `user`.
    pub fn disconnect(&self, user: &UserId, conn_id: u64) {
        let mut state = self.state.lock().unwrap();
        if state
            .sessions
            .get(user)
            .is_none_or(|s| s.conn_id != conn_id)
        {
            return;
        }
        state.sessions.remove(user);
        state.send_to(
            self.users.peer_of(user),
            ServerMessage::PeerStatus { online: false },
        );
    }

    /// Forwards a WebRTC signal to the other user. Returns `false` if they are offline.
    pub fn relay_signal(&self, from: &UserId, payload: SignalPayload) -> bool {
        let state = self.state.lock().unwrap();
        state.send_to(self.users.peer_of(from), ServerMessage::Signal { payload })
    }

    /// Delivers a chat message to the other user, or queues it if they are offline.
    pub fn send_chat(&self, from: &UserId, message: ChatMessage) -> ChatOutcome {
        let peer = self.users.peer_of(from);
        let mut state = self.state.lock().unwrap();

        if let Some(session) = state.sessions.get(peer) {
            let _ = session
                .tx
                .send(Outbound::Message(ServerMessage::ChatMessage {
                    id: message.id,
                    from: from.clone(),
                    body: message.body,
                    sent_at: message.sent_at,
                }));
            return ChatOutcome::Delivered;
        }

        let now = Instant::now();
        let ttl = self.limits.ttl;
        let queue = state.offline.entry(peer.clone()).or_default();
        queue.retain(|q| now.duration_since(q.queued_at) < ttl);
        if queue.len() >= self.limits.capacity {
            return ChatOutcome::QueueFull;
        }
        queue.push_back(QueuedChat {
            message,
            queued_at: now,
        });
        ChatOutcome::Queued
    }
}

impl State {
    /// Sends to `user` if they have a live session; returns whether it was sent.
    fn send_to(&self, user: &UserId, msg: ServerMessage) -> bool {
        match self.sessions.get(user) {
            Some(session) => {
                let _ = session.tx.send(Outbound::Message(msg));
                true
            }
            None => false,
        }
    }
}
