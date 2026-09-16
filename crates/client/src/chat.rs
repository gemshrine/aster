//! Chat data as the UI holds it (specs 0004, 0008).
//!
//! The types mirror what the core will emit (#6); grouping and status
//! rendering are decided here so the transport layer stays dumb.

use serde::{Deserialize, Serialize};

/// Gap after which a new message starts its own group, even from one author.
const GROUP_GAP_MS: i64 = 5 * 60 * 1000;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Author {
    Me,
    Peer,
}

/// Delivery status of a message we sent (spec 0004). Incoming messages have
/// none.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Status {
    /// Handed to the core, no answer yet.
    Sending,
    /// The core accepted it; the server has it.
    Sent,
    /// The peer was online and got it.
    Delivered,
    /// The peer was offline; the server queued it.
    Queued,
    /// The server refused it — `queue_full` and friends.
    Rejected { message: String },
}

impl Status {
    pub fn label(&self) -> &str {
        match self {
            Self::Sending => "Отправляется",
            Self::Sent => "Отправлено",
            Self::Delivered => "Доставлено",
            Self::Queued => "В очереди — собеседник не в сети",
            Self::Rejected { message } => message,
        }
    }

    pub fn is_rejected(&self) -> bool {
        matches!(self, Self::Rejected { .. })
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Message {
    pub id: String,
    pub author: Author,
    pub body: String,
    /// Unix time in milliseconds, as the protocol carries it (spec 0003).
    pub sent_at: i64,
    pub status: Option<Status>,
}

/// Messages from one author, close enough in time to share a header.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Group {
    pub author: Author,
    pub messages: Vec<Message>,
}

impl Group {
    /// Timestamp shown under the group — the last message in it.
    pub fn sent_at(&self) -> i64 {
        self.messages.last().map_or(0, |m| m.sent_at)
    }

    /// Status shown under the group — the last one that carries any.
    pub fn status(&self) -> Option<&Status> {
        self.messages.iter().rev().find_map(|m| m.status.as_ref())
    }
}

/// Splits a chronological list into display groups: same author, less than
/// five minutes apart (spec 0008).
pub fn group(messages: &[Message]) -> Vec<Group> {
    let mut groups: Vec<Group> = Vec::new();
    for message in messages {
        let fits = groups.last().is_some_and(|group| {
            group.author == message.author
                && message.sent_at - group.sent_at() < GROUP_GAP_MS
                && message.sent_at >= group.sent_at()
        });
        if fits {
            groups
                .last_mut()
                .expect("a group exists when fits is true")
                .messages
                .push(message.clone());
        } else {
            groups.push(Group {
                author: message.author,
                messages: vec![message.clone()],
            });
        }
    }
    groups
}

/// Day the message falls on, in the viewer's timezone — the list puts a
/// divider wherever this changes.
pub fn day(sent_at: i64) -> String {
    let date = js_sys::Date::new(&(sent_at as f64).into());
    date.to_locale_date_string("ru-RU", &js_sys::Object::new())
        .into()
}

/// `HH:MM` in the local timezone of the browser.
pub fn clock(sent_at: i64) -> String {
    let date = js_sys::Date::new(&(sent_at as f64).into());
    format!("{:02}:{:02}", date.get_hours(), date.get_minutes())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn msg(id: &str, author: Author, sent_at: i64) -> Message {
        Message {
            id: id.into(),
            author,
            body: "…".into(),
            sent_at,
            status: None,
        }
    }

    const MINUTE: i64 = 60 * 1000;

    #[test]
    fn keeps_one_author_within_five_minutes_together() {
        let messages = vec![
            msg("a", Author::Peer, 0),
            msg("b", Author::Peer, 2 * MINUTE),
            msg("c", Author::Peer, 4 * MINUTE),
        ];
        let groups = group(&messages);
        assert_eq!(groups.len(), 1);
        assert_eq!(groups[0].messages.len(), 3);
    }

    #[test]
    fn splits_on_a_five_minute_gap() {
        let messages = vec![
            msg("a", Author::Peer, 0),
            msg("b", Author::Peer, 5 * MINUTE),
        ];
        assert_eq!(group(&messages).len(), 2);
    }

    #[test]
    fn splits_on_a_change_of_author() {
        let messages = vec![msg("a", Author::Peer, 0), msg("b", Author::Me, MINUTE)];
        let groups = group(&messages);
        assert_eq!(groups.len(), 2);
        assert_eq!(groups[1].author, Author::Me);
    }

    #[test]
    fn measures_the_gap_from_the_last_message_not_the_first() {
        // Four minutes apart each: one group, though the ends are 8 minutes apart.
        let messages = vec![
            msg("a", Author::Peer, 0),
            msg("b", Author::Peer, 4 * MINUTE),
            msg("c", Author::Peer, 8 * MINUTE),
        ];
        assert_eq!(group(&messages).len(), 1);
    }

    #[test]
    fn a_message_out_of_order_starts_its_own_group() {
        let messages = vec![
            msg("a", Author::Peer, 10 * MINUTE),
            msg("b", Author::Peer, 0),
        ];
        assert_eq!(group(&messages).len(), 2);
    }

    #[test]
    fn group_shows_the_last_status_it_carries() {
        let mut first = msg("a", Author::Me, 0);
        first.status = Some(Status::Delivered);
        let mut second = msg("b", Author::Me, MINUTE);
        second.status = Some(Status::Sending);
        let groups = group(&[first, second]);
        assert_eq!(groups[0].status(), Some(&Status::Sending));
    }

    #[test]
    fn empty_history_has_no_groups() {
        assert!(group(&[]).is_empty());
    }
}
