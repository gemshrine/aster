use std::fmt;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use base64::Engine;
use base64::engine::general_purpose::STANDARD;
use hmac::{Hmac, KeyInit, Mac};
use protocol::{IceServer, UserId};
use sha1::Sha1;

const DEFAULT_TTL: Duration = Duration::from_secs(24 * 60 * 60);
const PORT: u16 = 3478;

/// coturn with `use-auth-secret`, see `specs/0007-deployment.md`.
#[derive(Clone)]
pub struct TurnConfig {
    host: String,
    secret: String,
    ttl: Duration,
}

impl fmt::Debug for TurnConfig {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("TurnConfig")
            .field("host", &self.host)
            .field("secret", &"<redacted>")
            .field("ttl", &self.ttl)
            .finish()
    }
}

impl TurnConfig {
    pub fn new(host: &str, secret: &str) -> Result<Self, String> {
        let host = host.trim();
        if host.is_empty() || host.contains(['/', ' ', '?']) {
            return Err(format!("invalid TURN host `{host}`"));
        }
        if secret.len() < 32 {
            return Err("TURN secret must be at least 32 characters".to_string());
        }
        Ok(Self {
            host: host.to_string(),
            secret: secret.to_string(),
            ttl: DEFAULT_TTL,
        })
    }

    /// STUN plus TURN (UDP and TCP) with TURN REST API credentials:
    /// `username = "<expiry>:<user>"`, `credential = base64(HMAC-SHA1(secret, username))`.
    pub fn ice_servers(&self, user: &UserId, now: SystemTime) -> Vec<IceServer> {
        let expiry = (now + self.ttl)
            .duration_since(UNIX_EPOCH)
            .expect("clock is after 1970")
            .as_secs();
        let username = format!("{expiry}:{}", user.0);
        let credential = credential(&self.secret, &username);
        vec![
            IceServer {
                urls: vec![format!("stun:{}:{PORT}", self.host)],
                username: None,
                credential: None,
            },
            IceServer {
                urls: vec![
                    format!("turn:{}:{PORT}?transport=udp", self.host),
                    format!("turn:{}:{PORT}?transport=tcp", self.host),
                ],
                username: Some(username),
                credential: Some(credential),
            },
        ]
    }
}

fn credential(secret: &str, username: &str) -> String {
    let mut mac =
        Hmac::<Sha1>::new_from_slice(secret.as_bytes()).expect("HMAC accepts keys of any size");
    mac.update(username.as_bytes());
    STANDARD.encode(mac.finalize().into_bytes())
}

#[cfg(test)]
mod tests {
    use super::*;

    const SECRET: &str = "coturn-shared-secret-0123456789abcdef";

    #[test]
    fn credential_matches_reference_vector() {
        // python3: base64(hmac.new(b"coturn-shared-secret", b"1700086400:alice", sha1))
        assert_eq!(
            credential("coturn-shared-secret", "1700086400:alice"),
            "BptBXEECUc4aF1huCrjGND+FqW4="
        );
    }

    #[test]
    fn ice_servers_use_expiring_username() {
        let turn = TurnConfig::new("turn.example.com", SECRET).unwrap();
        let now = UNIX_EPOCH + Duration::from_secs(1_700_000_000);
        let servers = turn.ice_servers(&UserId("alice".into()), now);

        assert_eq!(servers[0].urls, ["stun:turn.example.com:3478"]);
        assert_eq!(servers[0].username, None);

        let relay = &servers[1];
        assert_eq!(
            relay.urls,
            [
                "turn:turn.example.com:3478?transport=udp",
                "turn:turn.example.com:3478?transport=tcp",
            ]
        );
        let username = relay.username.as_deref().unwrap();
        assert_eq!(username, "1700086400:alice");
        assert_eq!(
            relay.credential.as_deref(),
            Some(credential(SECRET, username).as_str())
        );
    }

    #[test]
    fn rejects_bad_config_and_redacts_secret() {
        assert!(TurnConfig::new("", SECRET).is_err());
        assert!(TurnConfig::new("host/path", SECRET).is_err());
        assert!(TurnConfig::new("turn.example.com", "short").is_err());

        let turn = TurnConfig::new("turn.example.com", SECRET).unwrap();
        assert!(!format!("{turn:?}").contains(SECRET));
    }
}
