//! Connection state as the core publishes it (spec 0009).
//!
//! The shape mirrors the `connection-state` event verbatim so the UI can
//! deserialize the payload without a translation layer.

use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "state", rename_all = "snake_case")]
pub enum ConnectionState {
    /// No server address or token yet — the user has never set one up.
    #[default]
    NotConfigured,
    Connecting {
        attempt: u32,
    },
    Connected {
        user_id: String,
        peer_online: bool,
    },
    Reconnecting {
        attempt: u32,
        retry_in_ms: u64,
    },
    Disconnected,
    /// Terminal until the user acts; the core never retries out of it.
    Failed {
        reason: FailureReason,
    },
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum FailureReason {
    InvalidToken,
    UnsupportedProtocolVersion,
    SessionReplaced,
    /// Any reason a later protocol version adds. Without it an unknown value
    /// would fail to deserialize and take the whole event down (#31).
    #[serde(other)]
    Other,
}

impl FailureReason {
    pub fn message(&self) -> &'static str {
        match self {
            Self::InvalidToken => "Server rejected the token",
            Self::UnsupportedProtocolVersion => "Protocol version does not match the server",
            Self::SessionReplaced => "Another client signed in as this user",
            Self::Other => "Server refused the connection",
        }
    }
}

impl ConnectionState {
    /// Short label for the title strip.
    pub fn label(&self) -> String {
        match self {
            Self::NotConfigured => "Not configured".into(),
            Self::Connecting { attempt } if *attempt > 1 => {
                format!("Connecting… (attempt {attempt})")
            }
            Self::Connecting { .. } => "Connecting…".into(),
            Self::Connected { .. } => "Connected".into(),
            Self::Reconnecting { retry_in_ms, .. } => {
                format!("Reconnecting in {}", secs(*retry_in_ms))
            }
            Self::Disconnected => "Disconnected".into(),
            Self::Failed { reason } => reason.message().into(),
        }
    }

    /// Dot colour token for the indicator.
    pub fn tone(&self) -> &'static str {
        match self {
            Self::Connected { .. } => "var(--sage)",
            Self::Connecting { .. } | Self::Reconnecting { .. } => "var(--idle)",
            Self::Failed { .. } | Self::NotConfigured => "var(--danger)",
            Self::Disconnected => "var(--text-tertiary)",
        }
    }

    /// States the user has to act on: they get a visible strip, not grey text.
    pub fn needs_attention(&self) -> bool {
        matches!(self, Self::Failed { .. } | Self::NotConfigured)
    }

    /// Whether the composer accepts input: the link to the server is up. A
    /// peer who is offline is not a reason to block typing — the server
    /// queues the message for them (spec 0004).
    pub fn can_send(&self) -> bool {
        matches!(self, Self::Connected { .. })
    }

    pub fn peer_online(&self) -> bool {
        matches!(
            self,
            Self::Connected {
                peer_online: true,
                ..
            }
        )
    }
}

/// Rounds a retry delay to whole seconds, floored at one.
fn secs(ms: u64) -> String {
    format!("{}s", ms.div_ceil(1000).max(1))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parse(json: &str) -> ConnectionState {
        serde_json::from_str(json).expect("core payload should deserialize")
    }

    #[test]
    fn parses_the_payloads_from_spec_0009() {
        assert_eq!(
            parse(r#"{"state":"not_configured"}"#),
            ConnectionState::NotConfigured
        );
        assert_eq!(
            parse(r#"{"state":"connecting","attempt":2}"#),
            ConnectionState::Connecting { attempt: 2 }
        );
        assert_eq!(
            parse(r#"{"state":"connected","user_id":"kent","peer_online":true}"#),
            ConnectionState::Connected {
                user_id: "kent".into(),
                peer_online: true
            }
        );
        assert_eq!(
            parse(r#"{"state":"reconnecting","attempt":3,"retry_in_ms":4000}"#),
            ConnectionState::Reconnecting {
                attempt: 3,
                retry_in_ms: 4000
            }
        );
        assert_eq!(
            parse(r#"{"state":"disconnected"}"#),
            ConnectionState::Disconnected
        );
        assert_eq!(
            parse(r#"{"state":"failed","reason":"session_replaced"}"#),
            ConnectionState::Failed {
                reason: FailureReason::SessionReplaced
            }
        );
    }

    #[test]
    fn unknown_failure_reason_falls_back_instead_of_erroring() {
        assert_eq!(
            parse(r#"{"state":"failed","reason":"server_shutting_down"}"#),
            ConnectionState::Failed {
                reason: FailureReason::Other
            }
        );
    }

    #[test]
    fn rounds_retry_delay_up_to_whole_seconds() {
        assert_eq!(secs(4000), "4s");
        assert_eq!(secs(4500), "5s");
        assert_eq!(secs(200), "1s");
    }

    #[test]
    fn sending_needs_the_server_not_the_peer() {
        assert!(ConnectionState::Connected {
            user_id: "k".into(),
            peer_online: false
        }
        .can_send());
        assert!(!ConnectionState::Reconnecting {
            attempt: 1,
            retry_in_ms: 1000
        }
        .can_send());
        assert!(!ConnectionState::NotConfigured.can_send());
    }

    #[test]
    fn only_a_live_peer_counts_as_online() {
        assert!(ConnectionState::Connected {
            user_id: "k".into(),
            peer_online: true
        }
        .peer_online());
        assert!(!ConnectionState::Connected {
            user_id: "k".into(),
            peer_online: false
        }
        .peer_online());
        assert!(!ConnectionState::Reconnecting {
            attempt: 1,
            retry_in_ms: 1000
        }
        .peer_online());
    }
}
