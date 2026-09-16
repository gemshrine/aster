use std::collections::HashMap;
use std::sync::Mutex;
use std::sync::atomic::{AtomicU64, Ordering};

use protocol::{ErrorCode, ServerMessage, UserId};
use tokio::sync::mpsc::UnboundedSender;

use crate::config::Users;

/// Instructions for a connection's writer task.
#[derive(Debug)]
pub enum Outbound {
    Message(ServerMessage),
    Close,
}

struct Session {
    conn_id: u64,
    tx: UnboundedSender<Outbound>,
}

/// Tracks the (at most two) live sessions and routes messages between them.
pub struct Hub {
    users: Users,
    sessions: Mutex<HashMap<UserId, Session>>,
    next_conn_id: AtomicU64,
}

impl Hub {
    pub fn new(users: Users) -> Self {
        Self {
            users,
            sessions: Mutex::new(HashMap::new()),
            next_conn_id: AtomicU64::new(1),
        }
    }

    pub fn authenticate(&self, token: &str) -> Option<UserId> {
        self.users.authenticate(token)
    }

    /// Registers a session for `user`, sends it `Welcome`, and returns its
    /// connection id. An existing session for the same user is evicted.
    pub fn connect(&self, user: &UserId, tx: UnboundedSender<Outbound>) -> u64 {
        let conn_id = self.next_conn_id.fetch_add(1, Ordering::Relaxed);
        let peer = self.users.peer_of(user);
        let mut sessions = self.sessions.lock().unwrap();

        let peer_online = sessions.contains_key(peer);
        let _ = tx.send(Outbound::Message(ServerMessage::Welcome {
            user_id: user.clone(),
            peer_online,
        }));

        let previous = sessions.insert(user.clone(), Session { conn_id, tx });
        match previous {
            Some(old) => {
                let _ = old.tx.send(Outbound::Message(ServerMessage::Error {
                    code: ErrorCode::SessionReplaced,
                    message: "another connection authenticated as this user".into(),
                }));
                let _ = old.tx.send(Outbound::Close);
            }
            None => {
                if let Some(peer_session) = sessions.get(peer) {
                    let _ = peer_session
                        .tx
                        .send(Outbound::Message(ServerMessage::PeerStatus {
                            online: true,
                        }));
                }
            }
        }
        conn_id
    }

    /// Removes the session if it is still the current one for `user`.
    pub fn disconnect(&self, user: &UserId, conn_id: u64) {
        let mut sessions = self.sessions.lock().unwrap();
        if sessions.get(user).is_none_or(|s| s.conn_id != conn_id) {
            return;
        }
        sessions.remove(user);
        if let Some(peer_session) = sessions.get(self.users.peer_of(user)) {
            let _ = peer_session
                .tx
                .send(Outbound::Message(ServerMessage::PeerStatus {
                    online: false,
                }));
        }
    }
}
