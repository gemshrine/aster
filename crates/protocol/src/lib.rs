//! Shared wire types for the signaling protocol between `client` and
//! `signaling-server`. See `specs/0003-signaling-protocol.md`.
//!
//! Every message is a JSON object with a `type` discriminator in snake_case,
//! e.g. `{"type":"hello","protocol_version":1,"auth_token":"..."}`.

use serde::{Deserialize, Serialize};
use uuid::Uuid;

/// Bump on any wire-incompatible change to the signaling protocol.
pub const PROTOCOL_VERSION: u32 = 2;

/// Identifier of one of the two configured users.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct UserId(pub String);

/// Messages sent from a client to the signaling server.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ClientMessage {
    /// Must be the first message on a connection.
    Hello {
        protocol_version: u32,
        auth_token: String,
    },
    /// WebRTC signaling, relayed as-is to the other user.
    Signal { payload: SignalPayload },
    ChatMessage {
        /// Client-generated, used for dedup and delivery acks.
        id: Uuid,
        body: String,
        /// Unix time in milliseconds, set by the sender.
        sent_at: i64,
    },
}

/// Messages sent from the signaling server to a client.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ServerMessage {
    Welcome {
        user_id: UserId,
        peer_online: bool,
        /// STUN/TURN servers for `RTCPeerConnection`; TURN credentials are
        /// short-lived, a reconnect yields fresh ones.
        ice_servers: Vec<IceServer>,
    },
    /// Handshake failed; the server closes the connection afterwards.
    Rejected {
        reason: RejectReason,
    },
    PeerStatus {
        online: bool,
    },
    Signal {
        payload: SignalPayload,
    },
    ChatMessage {
        id: Uuid,
        from: UserId,
        body: String,
        sent_at: i64,
    },
    /// The recipient was online and the message was handed to them.
    ChatDelivered {
        id: Uuid,
    },
    /// The recipient is offline; the message waits in the server's in-memory queue.
    ChatQueued {
        id: Uuid,
    },
    Error {
        code: ErrorCode,
        message: String,
    },
}

/// Mirrors WebRTC's `RTCIceServer`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct IceServer {
    pub urls: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub username: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub credential: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum SignalPayload {
    Offer {
        sdp: String,
    },
    Answer {
        sdp: String,
    },
    IceCandidate {
        candidate: String,
        sdp_mid: Option<String>,
        sdp_mline_index: Option<u16>,
    },
    CallEnd,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RejectReason {
    InvalidToken,
    UnsupportedProtocolVersion,
    /// Any reason a newer server adds; never sent by this version.
    #[serde(other)]
    Unknown,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ErrorCode {
    /// The message could not be parsed.
    MalformedMessage,
    /// A non-`Hello` message arrived before a successful handshake.
    NotAuthenticated,
    /// A `Signal` was sent while the other user is offline.
    PeerOffline,
    /// Another connection authenticated as the same user; this one is closed.
    SessionReplaced,
    /// The offline queue for the recipient is full; the message was dropped.
    QueueFull,
    /// Any code a newer server adds; never sent by this version.
    #[serde(other)]
    Unknown,
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde::de::DeserializeOwned;
    use serde_json::json;
    use std::fmt::Debug;

    fn round_trip<T>(msg: &T)
    where
        T: Serialize + DeserializeOwned + PartialEq + Debug,
    {
        let encoded = serde_json::to_string(msg).unwrap();
        let decoded: T = serde_json::from_str(&encoded).unwrap();
        assert_eq!(&decoded, msg, "round trip changed message: {encoded}");
    }

    fn signal_payloads() -> Vec<SignalPayload> {
        vec![
            SignalPayload::Offer { sdp: "v=0".into() },
            SignalPayload::Answer { sdp: "v=0".into() },
            SignalPayload::IceCandidate {
                candidate: "candidate:1 1 UDP 2122252543 10.0.0.1 5000 typ host".into(),
                sdp_mid: Some("0".into()),
                sdp_mline_index: Some(0),
            },
            SignalPayload::IceCandidate {
                candidate: "candidate:2".into(),
                sdp_mid: None,
                sdp_mline_index: None,
            },
            SignalPayload::CallEnd,
        ]
    }

    #[test]
    fn client_messages_round_trip() {
        let mut messages = vec![
            ClientMessage::Hello {
                protocol_version: PROTOCOL_VERSION,
                auth_token: "secret".into(),
            },
            ClientMessage::ChatMessage {
                id: Uuid::new_v4(),
                body: "привет".into(),
                sent_at: 1_700_000_000_000,
            },
        ];
        messages.extend(
            signal_payloads()
                .into_iter()
                .map(|payload| ClientMessage::Signal { payload }),
        );
        messages.iter().for_each(round_trip);
    }

    #[test]
    fn server_messages_round_trip() {
        let id = Uuid::new_v4();
        let mut messages = vec![
            ServerMessage::Welcome {
                user_id: UserId("alice".into()),
                peer_online: true,
                ice_servers: vec![
                    IceServer {
                        urls: vec!["stun:turn.example.com:3478".into()],
                        username: None,
                        credential: None,
                    },
                    IceServer {
                        urls: vec!["turn:turn.example.com:3478?transport=udp".into()],
                        username: Some("1700000000:alice".into()),
                        credential: Some("c2VjcmV0".into()),
                    },
                ],
            },
            ServerMessage::Rejected {
                reason: RejectReason::InvalidToken,
            },
            ServerMessage::Rejected {
                reason: RejectReason::UnsupportedProtocolVersion,
            },
            ServerMessage::PeerStatus { online: false },
            ServerMessage::ChatMessage {
                id,
                from: UserId("bob".into()),
                body: "hi".into(),
                sent_at: 1,
            },
            ServerMessage::ChatDelivered { id },
            ServerMessage::ChatQueued { id },
        ];
        messages.extend(
            [
                ErrorCode::MalformedMessage,
                ErrorCode::NotAuthenticated,
                ErrorCode::PeerOffline,
                ErrorCode::SessionReplaced,
                ErrorCode::QueueFull,
            ]
            .into_iter()
            .map(|code| ServerMessage::Error {
                code,
                message: "details".into(),
            }),
        );
        messages.extend(
            signal_payloads()
                .into_iter()
                .map(|payload| ServerMessage::Signal { payload }),
        );
        messages.iter().for_each(round_trip);
    }

    #[test]
    fn wire_format_matches_spec() {
        let hello = ClientMessage::Hello {
            protocol_version: 1,
            auth_token: "t".into(),
        };
        assert_eq!(
            serde_json::to_value(&hello).unwrap(),
            json!({"type": "hello", "protocol_version": 1, "auth_token": "t"})
        );

        let signal = ServerMessage::Signal {
            payload: SignalPayload::IceCandidate {
                candidate: "c".into(),
                sdp_mid: None,
                sdp_mline_index: Some(1),
            },
        };
        assert_eq!(
            serde_json::to_value(&signal).unwrap(),
            json!({
                "type": "signal",
                "payload": {
                    "type": "ice_candidate",
                    "candidate": "c",
                    "sdp_mid": null,
                    "sdp_mline_index": 1
                }
            })
        );

        let rejected = ServerMessage::Rejected {
            reason: RejectReason::UnsupportedProtocolVersion,
        };
        assert_eq!(
            serde_json::to_value(&rejected).unwrap(),
            json!({"type": "rejected", "reason": "unsupported_protocol_version"})
        );

        let welcome = ServerMessage::Welcome {
            user_id: UserId("alice".into()),
            peer_online: false,
            ice_servers: vec![
                IceServer {
                    urls: vec!["stun:h:3478".into()],
                    username: None,
                    credential: None,
                },
                IceServer {
                    urls: vec!["turn:h:3478?transport=udp".into()],
                    username: Some("1:alice".into()),
                    credential: Some("c".into()),
                },
            ],
        };
        assert_eq!(
            serde_json::to_value(&welcome).unwrap(),
            json!({
                "type": "welcome",
                "user_id": "alice",
                "peer_online": false,
                "ice_servers": [
                    {"urls": ["stun:h:3478"]},
                    {"urls": ["turn:h:3478?transport=udp"], "username": "1:alice", "credential": "c"}
                ]
            })
        );
    }

    #[test]
    fn unknown_reasons_and_codes_fall_back() {
        assert_eq!(
            serde_json::from_str::<ServerMessage>(r#"{"type":"rejected","reason":"banned"}"#)
                .unwrap(),
            ServerMessage::Rejected {
                reason: RejectReason::Unknown
            }
        );
        assert_eq!(
            serde_json::from_str::<ServerMessage>(
                r#"{"type":"error","code":"rate_limited","message":"slow down"}"#
            )
            .unwrap(),
            ServerMessage::Error {
                code: ErrorCode::Unknown,
                message: "slow down".into()
            }
        );
    }

    #[test]
    fn unknown_message_type_is_rejected() {
        let result = serde_json::from_str::<ClientMessage>(r#"{"type":"nope"}"#);
        assert!(result.is_err());
    }
}
