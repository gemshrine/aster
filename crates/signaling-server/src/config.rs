use std::fmt;

use protocol::UserId;
use subtle::ConstantTimeEq;

const MIN_TOKEN_LEN: usize = 32;

#[derive(Clone)]
pub struct User {
    pub id: UserId,
    token: String,
}

impl fmt::Debug for User {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("User")
            .field("id", &self.id)
            .field("token", &"<redacted>")
            .finish()
    }
}

/// The two users allowed to connect, see `specs/0006-auth-security.md`.
#[derive(Debug, Clone)]
pub struct Users([User; 2]);

impl Users {
    /// Parses `id:token,id:token` (the `ASTER_USERS` env var).
    pub fn parse(raw: &str) -> Result<Self, String> {
        let users = raw
            .split(',')
            .map(str::trim)
            .filter(|entry| !entry.is_empty())
            .map(|entry| {
                let (id, token) = entry
                    .split_once(':')
                    .ok_or_else(|| format!("expected `id:token`, got `{entry}`"))?;
                let (id, token) = (id.trim(), token.trim());
                if id.is_empty() {
                    return Err("user id must not be empty".to_string());
                }
                if token.len() < MIN_TOKEN_LEN {
                    return Err(format!(
                        "token for `{id}` must be at least {MIN_TOKEN_LEN} characters"
                    ));
                }
                Ok(User {
                    id: UserId(id.to_string()),
                    token: token.to_string(),
                })
            })
            .collect::<Result<Vec<_>, _>>()?;

        let users: [User; 2] = users
            .try_into()
            .map_err(|users: Vec<User>| format!("expected exactly 2 users, got {}", users.len()))?;
        if users[0].id == users[1].id {
            return Err("user ids must be distinct".to_string());
        }
        if users[0].token == users[1].token {
            return Err("user tokens must be distinct".to_string());
        }
        Ok(Self(users))
    }

    /// Constant-time token lookup across both users.
    pub fn authenticate(&self, token: &str) -> Option<UserId> {
        let mut found = None;
        for user in &self.0 {
            if bool::from(user.token.as_bytes().ct_eq(token.as_bytes())) {
                found = Some(user.id.clone());
            }
        }
        found
    }

    /// The other user. `id` must be one of the configured users.
    pub fn peer_of(&self, id: &UserId) -> &UserId {
        if &self.0[0].id == id {
            &self.0[1].id
        } else {
            &self.0[0].id
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const A: &str = "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";
    const B: &str = "bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb";

    #[test]
    fn parses_two_users() {
        let users = Users::parse(&format!("alice:{A}, bob:{B}")).unwrap();
        assert_eq!(users.authenticate(A), Some(UserId("alice".into())));
        assert_eq!(users.authenticate(B), Some(UserId("bob".into())));
        assert_eq!(users.authenticate("nope"), None);
        assert_eq!(
            users.peer_of(&UserId("alice".into())),
            &UserId("bob".into())
        );
        assert_eq!(
            users.peer_of(&UserId("bob".into())),
            &UserId("alice".into())
        );
    }

    #[test]
    fn rejects_invalid_configs() {
        for raw in [
            "",
            &format!("alice:{A}"),
            &format!("alice:{A},bob:{B},carol:{A}c"),
            &format!("alice:{A},alice:{B}"),
            &format!("alice:{A},bob:{A}"),
            &format!("alice:{A},bob:short"),
            &format!("alice{A},bob:{B}"),
            &format!(":{A},bob:{B}"),
        ] {
            assert!(Users::parse(raw).is_err(), "should reject `{raw}`");
        }
    }

    #[test]
    fn debug_does_not_leak_tokens() {
        let users = Users::parse(&format!("alice:{A},bob:{B}")).unwrap();
        let debug = format!("{users:?}");
        assert!(!debug.contains(A) && !debug.contains(B));
    }
}
