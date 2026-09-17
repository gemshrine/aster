//! Call state as the core publishes it (spec 0011).

use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "state", rename_all = "snake_case")]
pub enum CallState {
    #[default]
    Idle,
    /// We called and are waiting for the peer to pick up.
    Calling,
    /// The peer is calling us.
    Ringing,
    /// Answered on both sides; media is still being negotiated.
    Connecting,
    Connected {
        /// Unix ms of the moment media came up — the timer counts from here.
        since: i64,
    },
    /// Momentary: the core emits it and goes back to `idle`, so the outcome
    /// is ours to display.
    Ended { reason: EndReason },
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
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
    /// A reason a later core version adds.
    #[serde(other)]
    Other,
}

impl EndReason {
    pub fn message(self) -> &'static str {
        match self {
            Self::Hangup | Self::RemoteHangup => "Call ended",
            Self::Declined => "Call declined",
            Self::Cancelled => "Call cancelled",
            Self::Timeout => "No answer",
            Self::Failed => "Could not connect",
            Self::ConnectionLost => "Connection lost",
            Self::PeerOffline => "Peer offline",
            Self::Other => "Call ended",
        }
    }
}

impl CallState {
    /// Whether the call screen takes over the main area.
    pub fn is_active(&self) -> bool {
        !matches!(self, Self::Idle | Self::Ended { .. })
    }

    /// Short line under the peer's tile while the call is not up yet.
    pub fn waiting_label(&self) -> Option<&'static str> {
        match self {
            Self::Calling => Some("Calling…"),
            Self::Ringing => Some("Incoming call"),
            Self::Connecting => Some("Connecting…"),
            _ => None,
        }
    }
}

/// `MM:SS`, or `H:MM:SS` once the call runs past an hour.
pub fn duration(since: i64, now: i64) -> String {
    let total = (now - since).max(0) / 1000;
    let (hours, minutes, seconds) = (total / 3600, (total / 60) % 60, total % 60);
    if hours > 0 {
        format!("{hours}:{minutes:02}:{seconds:02}")
    } else {
        format!("{minutes:02}:{seconds:02}")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parse(json: &str) -> CallState {
        serde_json::from_str(json).expect("core payload should deserialize")
    }

    #[test]
    fn parses_the_payloads_from_spec_0011() {
        assert_eq!(parse(r#"{"state":"idle"}"#), CallState::Idle);
        assert_eq!(parse(r#"{"state":"calling"}"#), CallState::Calling);
        assert_eq!(parse(r#"{"state":"ringing"}"#), CallState::Ringing);
        assert_eq!(parse(r#"{"state":"connecting"}"#), CallState::Connecting);
        assert_eq!(
            parse(r#"{"state":"connected","since":1726500000000}"#),
            CallState::Connected {
                since: 1_726_500_000_000
            }
        );
        assert_eq!(
            parse(r#"{"state":"ended","reason":"remote_hangup"}"#),
            CallState::Ended {
                reason: EndReason::RemoteHangup
            }
        );
    }

    #[test]
    fn unknown_end_reason_falls_back_instead_of_erroring() {
        assert_eq!(
            parse(r#"{"state":"ended","reason":"moved_to_another_planet"}"#),
            CallState::Ended {
                reason: EndReason::Other
            }
        );
    }

    #[test]
    fn only_a_live_call_takes_over_the_screen() {
        assert!(CallState::Calling.is_active());
        assert!(CallState::Connected { since: 0 }.is_active());
        assert!(!CallState::Idle.is_active());
        assert!(!CallState::Ended {
            reason: EndReason::Hangup
        }
        .is_active());
    }

    #[test]
    fn duration_counts_from_the_connected_moment() {
        assert_eq!(duration(0, 0), "00:00");
        assert_eq!(duration(0, 65_000), "01:05");
        assert_eq!(duration(0, 3_725_000), "1:02:05");
        // A clock that jumped backwards must not render a negative timer.
        assert_eq!(duration(5_000, 0), "00:00");
    }
}
