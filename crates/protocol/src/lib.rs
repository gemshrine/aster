//! Shared wire types for the signaling protocol between `client` and
//! `signaling-server`. See `specs/0003-signaling-protocol.md`.

/// Bump on any wire-incompatible change to the signaling protocol.
pub const PROTOCOL_VERSION: u32 = 1;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn protocol_version_is_set() {
        assert_eq!(PROTOCOL_VERSION, 1);
    }
}
