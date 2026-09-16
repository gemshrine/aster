//! Chat history, statuses and resend, see `specs/0010-chat-history.md`.

use std::path::Path;
use std::sync::Mutex;

use rusqlite::{params, Connection, OptionalExtension, Row};
use serde::Serialize;
use tokio::sync::mpsc;
use uuid::Uuid;

use super::signaling::{AckStatus, ConnectionState, CoreEvent, SignalingHandle};

const MAX_BODY_CHARS: usize = 16_000;
pub const MAX_PAGE: u32 = 200;

const MIGRATIONS: &[&str] = &["CREATE TABLE messages (
        id          TEXT PRIMARY KEY,
        direction   TEXT NOT NULL CHECK (direction IN ('out', 'in')),
        body        TEXT NOT NULL,
        sent_at     INTEGER NOT NULL,
        received_at INTEGER NOT NULL,
        status      TEXT CHECK (status IN ('sending', 'sent', 'queued', 'delivered', 'failed'))
    );
    CREATE INDEX messages_order ON messages (sent_at, received_at);"];

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Direction {
    Out,
    In,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Status {
    Sending,
    Sent,
    Queued,
    Delivered,
    Failed,
}

impl Status {
    fn as_str(self) -> &'static str {
        match self {
            Status::Sending => "sending",
            Status::Sent => "sent",
            Status::Queued => "queued",
            Status::Delivered => "delivered",
            Status::Failed => "failed",
        }
    }

    fn parse(raw: &str) -> Option<Self> {
        Some(match raw {
            "sending" => Status::Sending,
            "sent" => Status::Sent,
            "queued" => Status::Queued,
            "delivered" => Status::Delivered,
            "failed" => Status::Failed,
            _ => return None,
        })
    }

    /// Progress rank for monotonic updates; `failed` is handled separately.
    fn rank(self) -> u8 {
        match self {
            Status::Sending | Status::Failed => 0,
            Status::Sent => 1,
            Status::Queued => 2,
            Status::Delivered => 3,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Message {
    pub id: Uuid,
    pub direction: Direction,
    pub body: String,
    pub sent_at: i64,
    pub status: Option<Status>,
}

#[derive(Debug)]
pub enum ChatError {
    InvalidMessage(String),
    NotFound,
    NotFailed,
    Db(rusqlite::Error),
}

impl From<rusqlite::Error> for ChatError {
    fn from(err: rusqlite::Error) -> Self {
        ChatError::Db(err)
    }
}

impl ChatError {
    pub fn code(&self) -> &'static str {
        match self {
            ChatError::InvalidMessage(_) => "invalid_message",
            ChatError::NotFound => "not_found",
            ChatError::NotFailed => "not_failed",
            ChatError::Db(_) => "io",
        }
    }
}

impl std::fmt::Display for ChatError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ChatError::InvalidMessage(reason) => write!(f, "{reason}"),
            ChatError::NotFound => write!(f, "message not found"),
            ChatError::NotFailed => write!(f, "only failed messages can be retried"),
            ChatError::Db(err) => write!(f, "history database error: {err}"),
        }
    }
}

pub struct ChatStore {
    conn: Connection,
}

impl ChatStore {
    pub fn open(path: &Path) -> Result<Self, ChatError> {
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir).map_err(|err| {
                ChatError::Db(rusqlite::Error::ToSqlConversionFailure(err.into()))
            })?;
        }
        let conn = Connection::open(path)?;
        conn.pragma_update(None, "journal_mode", "WAL")?;
        Self::with_connection(conn)
    }

    pub fn open_in_memory() -> Result<Self, ChatError> {
        Self::with_connection(Connection::open_in_memory()?)
    }

    fn with_connection(mut conn: Connection) -> Result<Self, ChatError> {
        let version: i64 = conn.pragma_query_value(None, "user_version", |row| row.get(0))?;
        let tx = conn.transaction()?;
        for (index, migration) in MIGRATIONS.iter().enumerate().skip(version as usize) {
            tx.execute_batch(migration)?;
            tx.pragma_update(None, "user_version", index as i64 + 1)?;
        }
        tx.commit()?;
        Ok(Self { conn })
    }

    pub fn insert_outgoing(&self, body: &str, now: i64) -> Result<Message, ChatError> {
        let body = body.trim();
        if body.is_empty() {
            return Err(ChatError::InvalidMessage("message is empty".into()));
        }
        if body.chars().count() > MAX_BODY_CHARS {
            return Err(ChatError::InvalidMessage(format!(
                "message is longer than {MAX_BODY_CHARS} characters"
            )));
        }
        let message = Message {
            id: Uuid::new_v4(),
            direction: Direction::Out,
            body: body.to_string(),
            sent_at: now,
            status: Some(Status::Sending),
        };
        self.conn.execute(
            "INSERT INTO messages (id, direction, body, sent_at, received_at, status)
             VALUES (?1, 'out', ?2, ?3, ?3, 'sending')",
            params![message.id.to_string(), message.body, now],
        )?;
        Ok(message)
    }

    /// `None` if a message with this id is already stored.
    pub fn insert_incoming(
        &self,
        id: Uuid,
        body: &str,
        sent_at: i64,
        now: i64,
    ) -> Result<Option<Message>, ChatError> {
        let inserted = self.conn.execute(
            "INSERT INTO messages (id, direction, body, sent_at, received_at, status)
             VALUES (?1, 'in', ?2, ?3, ?4, NULL)
             ON CONFLICT (id) DO NOTHING",
            params![id.to_string(), body, sent_at, now],
        )?;
        Ok((inserted == 1).then(|| Message {
            id,
            direction: Direction::In,
            body: body.to_string(),
            sent_at,
            status: None,
        }))
    }

    /// Moves an outgoing message forward; `None` if nothing changed.
    pub fn advance(&self, id: Uuid, status: Status) -> Result<Option<Message>, ChatError> {
        debug_assert_ne!(status, Status::Failed);
        let Some(current) = self.get(id)? else {
            return Ok(None);
        };
        let Some(old) = current.status else {
            return Ok(None);
        };
        if old != Status::Failed && status.rank() <= old.rank() {
            return Ok(None);
        }
        if old == Status::Failed && status != Status::Delivered {
            // Only a real delivery overrides a server-side drop.
            return Ok(None);
        }
        self.set_status(current, status).map(Some)
    }

    /// `None` if the message is unknown, incoming, delivered or already failed.
    pub fn mark_failed(&self, id: Uuid) -> Result<Option<Message>, ChatError> {
        match self.get(id)? {
            Some(message)
                if matches!(
                    message.status,
                    Some(Status::Sending | Status::Sent | Status::Queued)
                ) =>
            {
                self.set_status(message, Status::Failed).map(Some)
            }
            _ => Ok(None),
        }
    }

    pub fn retry(&self, id: Uuid) -> Result<Message, ChatError> {
        let message = self.get(id)?.ok_or(ChatError::NotFound)?;
        if message.status != Some(Status::Failed) {
            return Err(ChatError::NotFailed);
        }
        self.set_status(message, Status::Sending)
    }

    /// Own messages without a server ack, oldest first.
    pub fn unacked(&self) -> Result<Vec<Message>, ChatError> {
        let mut stmt = self.conn.prepare(
            "SELECT id, direction, body, sent_at, status FROM messages
             WHERE direction = 'out' AND status IN ('sending', 'sent')
             ORDER BY sent_at, received_at, rowid",
        )?;
        let rows = stmt.query_map([], row_to_message)?;
        rows.collect::<Result<_, _>>().map_err(Into::into)
    }

    /// Up to `limit` messages strictly before `before` (or the latest), ascending.
    pub fn page(&self, before: Option<Uuid>, limit: u32) -> Result<Vec<Message>, ChatError> {
        let limit = limit.clamp(1, MAX_PAGE);
        let mut messages = match before {
            None => {
                let mut stmt = self.conn.prepare(
                    "SELECT id, direction, body, sent_at, status FROM messages
                     ORDER BY sent_at DESC, received_at DESC, rowid DESC LIMIT ?1",
                )?;
                let rows = stmt
                    .query_map([limit], row_to_message)?
                    .collect::<Result<Vec<_>, _>>()?;
                rows
            }
            Some(before) => {
                let key: Option<(i64, i64, i64)> = self
                    .conn
                    .query_row(
                        "SELECT sent_at, received_at, rowid FROM messages WHERE id = ?1",
                        [before.to_string()],
                        |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
                    )
                    .optional()?;
                let (sent_at, received_at, rowid) = key.ok_or(ChatError::NotFound)?;
                let mut stmt = self.conn.prepare(
                    "SELECT id, direction, body, sent_at, status FROM messages
                     WHERE (sent_at, received_at, rowid) < (?1, ?2, ?3)
                     ORDER BY sent_at DESC, received_at DESC, rowid DESC LIMIT ?4",
                )?;
                let rows = stmt
                    .query_map(params![sent_at, received_at, rowid, limit], row_to_message)?
                    .collect::<Result<Vec<_>, _>>()?;
                rows
            }
        };
        messages.reverse();
        Ok(messages)
    }

    fn get(&self, id: Uuid) -> Result<Option<Message>, ChatError> {
        self.conn
            .query_row(
                "SELECT id, direction, body, sent_at, status FROM messages WHERE id = ?1",
                [id.to_string()],
                row_to_message,
            )
            .optional()
            .map_err(Into::into)
    }

    fn set_status(&self, mut message: Message, status: Status) -> Result<Message, ChatError> {
        self.conn.execute(
            "UPDATE messages SET status = ?1 WHERE id = ?2",
            params![status.as_str(), message.id.to_string()],
        )?;
        message.status = Some(status);
        Ok(message)
    }
}

fn row_to_message(row: &Row<'_>) -> rusqlite::Result<Message> {
    let id: String = row.get(0)?;
    let direction: String = row.get(1)?;
    let status: Option<String> = row.get(4)?;
    let invalid = |what: &str| {
        rusqlite::Error::FromSqlConversionFailure(
            0,
            rusqlite::types::Type::Text,
            format!("invalid {what} in history").into(),
        )
    };
    Ok(Message {
        id: Uuid::parse_str(&id).map_err(|_| invalid("id"))?,
        direction: match direction.as_str() {
            "out" => Direction::Out,
            "in" => Direction::In,
            _ => return Err(invalid("direction")),
        },
        body: row.get(2)?,
        sent_at: row.get(3)?,
        status: match status {
            None => None,
            Some(raw) => Some(Status::parse(&raw).ok_or_else(|| invalid("status"))?),
        },
    })
}

/// Glue between the store, the signaling connection and the UI.
pub struct Chat {
    store: Mutex<ChatStore>,
    signaling: SignalingHandle,
    upserts: mpsc::UnboundedSender<Message>,
}

impl Chat {
    pub fn new(
        store: ChatStore,
        signaling: SignalingHandle,
        upserts: mpsc::UnboundedSender<Message>,
    ) -> Self {
        Self {
            store: Mutex::new(store),
            signaling,
            upserts,
        }
    }

    pub fn load_history(
        &self,
        before: Option<Uuid>,
        limit: u32,
    ) -> Result<Vec<Message>, ChatError> {
        self.store.lock().unwrap().page(before, limit)
    }

    pub async fn send_message(&self, body: &str) -> Result<Message, ChatError> {
        let message = self.store.lock().unwrap().insert_outgoing(body, now_ms())?;
        self.emit(Some(message.clone()));
        Ok(self.transmit(message).await)
    }

    pub async fn retry_message(&self, id: Uuid) -> Result<Message, ChatError> {
        let message = self.store.lock().unwrap().retry(id)?;
        self.emit(Some(message.clone()));
        Ok(self.transmit(message).await)
    }

    /// Feeds every core event through the history; call in event order.
    pub async fn handle_event(&self, event: &CoreEvent) {
        let result = match event {
            CoreEvent::State(ConnectionState::Connected { .. }) => {
                self.resend().await;
                Ok(None)
            }
            CoreEvent::ChatMessage {
                id, body, sent_at, ..
            } => self
                .store
                .lock()
                .unwrap()
                .insert_incoming(*id, body, *sent_at, now_ms()),
            CoreEvent::ChatAck { id, status } => self.store.lock().unwrap().advance(
                *id,
                match status {
                    AckStatus::Queued => Status::Queued,
                    AckStatus::Delivered => Status::Delivered,
                },
            ),
            CoreEvent::ChatRejected { id: Some(id), .. } => {
                self.store.lock().unwrap().mark_failed(*id)
            }
            _ => Ok(None),
        };
        match result {
            Ok(message) => self.emit(message),
            Err(err) => eprintln!("chat history: {err}"),
        }
    }

    async fn resend(&self) {
        let unacked = match self.store.lock().unwrap().unacked() {
            Ok(unacked) => unacked,
            Err(err) => {
                eprintln!("chat history: {err}");
                return;
            }
        };
        for message in unacked {
            let before = message.status;
            let after = self.transmit(message).await;
            if after.status == before {
                // Not connected anymore; the next `connected` resends the rest.
                break;
            }
        }
    }

    /// Sends and returns the message with its latest stored status.
    async fn transmit(&self, message: Message) -> Message {
        let sent = self
            .signaling
            .send_chat(message.id, message.body.clone(), message.sent_at)
            .await;
        if sent.is_err() {
            return message;
        }
        let advanced = self.store.lock().unwrap().advance(message.id, Status::Sent);
        match advanced {
            Ok(Some(updated)) => {
                self.emit(Some(updated.clone()));
                updated
            }
            Ok(None) => self
                .store
                .lock()
                .unwrap()
                .get(message.id)
                .ok()
                .flatten()
                .unwrap_or(message),
            Err(err) => {
                eprintln!("chat history: {err}");
                message
            }
        }
    }

    fn emit(&self, message: Option<Message>) {
        if let Some(message) = message {
            let _ = self.upserts.send(message);
        }
    }
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

    fn store() -> ChatStore {
        ChatStore::open_in_memory().unwrap()
    }

    #[test]
    fn validates_outgoing_body() {
        let store = store();
        assert!(matches!(
            store.insert_outgoing("   ", 1),
            Err(ChatError::InvalidMessage(_))
        ));
        assert!(matches!(
            store.insert_outgoing(&"я".repeat(MAX_BODY_CHARS + 1), 1),
            Err(ChatError::InvalidMessage(_))
        ));
        let message = store.insert_outgoing("  привет \n", 5).unwrap();
        assert_eq!(message.body, "привет");
        assert_eq!(message.status, Some(Status::Sending));
        assert_eq!(message.direction, Direction::Out);
    }

    #[test]
    fn messages_sharing_a_timestamp_keep_insertion_order() {
        // Two messages can land in the same millisecond — a fast reply, or a
        // queue flushed on reconnect. Then only the order they were stored in
        // is left to sort by; a random UUID would shuffle them at every read.
        let store = store();
        for body in ["первое", "второе", "третье"] {
            store.insert_incoming(Uuid::new_v4(), body, 7, 7).unwrap();
        }
        let bodies: Vec<_> = store
            .page(None, 50)
            .unwrap()
            .into_iter()
            .map(|m| m.body)
            .collect();
        assert_eq!(bodies, ["первое", "второе", "третье"]);
    }

    #[test]
    fn paging_through_a_shared_timestamp_does_not_repeat_or_skip() {
        let store = store();
        for body in ["a", "b", "c", "d"] {
            store.insert_incoming(Uuid::new_v4(), body, 7, 7).unwrap();
        }
        let tail = store.page(None, 2).unwrap();
        assert_eq!(
            tail.iter().map(|m| m.body.as_str()).collect::<Vec<_>>(),
            ["c", "d"]
        );
        let head = store.page(Some(tail[0].id), 2).unwrap();
        assert_eq!(
            head.iter().map(|m| m.body.as_str()).collect::<Vec<_>>(),
            ["a", "b"]
        );
    }

    #[test]
    fn incoming_is_deduplicated_by_id() {
        let store = store();
        let id = Uuid::new_v4();
        assert!(store.insert_incoming(id, "hi", 1, 10).unwrap().is_some());
        assert!(store.insert_incoming(id, "hi", 1, 20).unwrap().is_none());
        assert_eq!(store.page(None, 50).unwrap().len(), 1);
    }

    #[test]
    fn status_only_moves_forward() {
        let store = store();
        let id = store.insert_outgoing("x", 1).unwrap().id;

        assert_eq!(
            store.advance(id, Status::Queued).unwrap().unwrap().status,
            Some(Status::Queued)
        );
        assert!(store.advance(id, Status::Sent).unwrap().is_none());
        assert!(store.advance(id, Status::Queued).unwrap().is_none());
        assert_eq!(
            store
                .advance(id, Status::Delivered)
                .unwrap()
                .unwrap()
                .status,
            Some(Status::Delivered)
        );
        assert!(store.advance(id, Status::Queued).unwrap().is_none());
        assert!(store.mark_failed(id).unwrap().is_none(), "delivered stays");

        let incoming = Uuid::new_v4();
        store.insert_incoming(incoming, "in", 1, 1).unwrap();
        assert!(store
            .advance(incoming, Status::Delivered)
            .unwrap()
            .is_none());
        assert!(store
            .advance(Uuid::new_v4(), Status::Sent)
            .unwrap()
            .is_none());
    }

    #[test]
    fn failed_messages_can_be_retried() {
        let store = store();
        let id = store.insert_outgoing("x", 1).unwrap().id;
        assert!(matches!(store.retry(id), Err(ChatError::NotFailed)));
        assert!(matches!(
            store.retry(Uuid::new_v4()),
            Err(ChatError::NotFound)
        ));

        store.advance(id, Status::Sent).unwrap();
        assert_eq!(
            store.mark_failed(id).unwrap().unwrap().status,
            Some(Status::Failed)
        );
        assert!(store.mark_failed(id).unwrap().is_none());
        assert!(
            store.advance(id, Status::Sent).unwrap().is_none(),
            "failed ignores late sent"
        );
        assert!(store.unacked().unwrap().is_empty());

        assert_eq!(store.retry(id).unwrap().status, Some(Status::Sending));
        assert_eq!(store.unacked().unwrap().len(), 1);
    }

    #[test]
    fn unacked_lists_sending_and_sent_in_order() {
        let store = store();
        let later = store.insert_outgoing("later", 20).unwrap().id;
        let earlier = store.insert_outgoing("earlier", 10).unwrap().id;
        let queued = store.insert_outgoing("queued", 5).unwrap().id;
        store.advance(later, Status::Sent).unwrap();
        store.advance(queued, Status::Queued).unwrap();

        let ids: Vec<_> = store.unacked().unwrap().iter().map(|m| m.id).collect();
        assert_eq!(ids, [earlier, later]);
    }

    #[test]
    fn pages_backwards_in_ascending_order() {
        let store = store();
        let ids: Vec<Uuid> = (0..5)
            .map(|i| store.insert_outgoing(&format!("m{i}"), i).unwrap().id)
            .collect();

        let last_two: Vec<_> = store.page(None, 2).unwrap().iter().map(|m| m.id).collect();
        assert_eq!(last_two, ids[3..]);

        let before: Vec<_> = store
            .page(Some(ids[3]), 2)
            .unwrap()
            .iter()
            .map(|m| m.id)
            .collect();
        assert_eq!(before, ids[1..3]);

        assert!(store.page(Some(ids[0]), 10).unwrap().is_empty());
        assert!(matches!(
            store.page(Some(Uuid::new_v4()), 10),
            Err(ChatError::NotFound)
        ));
    }

    #[test]
    fn reopening_the_file_keeps_history() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("data").join("history.sqlite3");
        let id = {
            let store = ChatStore::open(&path).unwrap();
            store.insert_outgoing("persisted", 1).unwrap().id
        };
        let store = ChatStore::open(&path).unwrap();
        let page = store.page(None, 10).unwrap();
        assert_eq!(page.len(), 1);
        assert_eq!(page[0].id, id);
    }

    #[test]
    fn message_json_matches_spec() {
        let message = Message {
            id: Uuid::nil(),
            direction: Direction::Out,
            body: "привет".into(),
            sent_at: 1_726_500_000_000,
            status: Some(Status::Delivered),
        };
        assert_eq!(
            serde_json::to_value(&message).unwrap(),
            serde_json::json!({
                "id": "00000000-0000-0000-0000-000000000000",
                "direction": "out",
                "body": "привет",
                "sent_at": 1_726_500_000_000_i64,
                "status": "delivered"
            })
        );
    }
}
