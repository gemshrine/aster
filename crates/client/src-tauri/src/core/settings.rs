use std::fmt;
use std::io::{self, Write};
use std::path::Path;

use serde::{Deserialize, Serialize};
use url::{Host, Url};

#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Settings {
    pub server_url: String,
    pub token: String,
}

impl fmt::Debug for Settings {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Settings")
            .field("server_url", &self.server_url)
            .field("token", &"<redacted>")
            .finish()
    }
}

impl Settings {
    pub fn new(server_url: &str, token: &str) -> Result<Self, String> {
        let server_url = server_url.trim();
        let token = token.trim();
        let url = Url::parse(server_url).map_err(|err| format!("invalid server url: {err}"))?;
        match url.scheme() {
            "wss" => {}
            "ws" if is_loopback(&url) => {}
            "ws" => return Err("ws:// is only allowed for localhost, use wss://".to_string()),
            other => return Err(format!("unsupported url scheme `{other}`, use wss://")),
        }
        if url.host().is_none() {
            return Err("server url has no host".to_string());
        }
        if token.is_empty() {
            return Err("token must not be empty".to_string());
        }
        Ok(Self {
            server_url: server_url.to_string(),
            token: token.to_string(),
        })
    }

    /// `Ok(None)` if the file does not exist yet.
    pub fn load(path: &Path) -> io::Result<Option<Self>> {
        let raw = match std::fs::read_to_string(path) {
            Ok(raw) => raw,
            Err(err) if err.kind() == io::ErrorKind::NotFound => return Ok(None),
            Err(err) => return Err(err),
        };
        let stored: Settings = serde_json::from_str(&raw)
            .map_err(|err| io::Error::new(io::ErrorKind::InvalidData, err))?;
        Settings::new(&stored.server_url, &stored.token)
            .map(Some)
            .map_err(|err| io::Error::new(io::ErrorKind::InvalidData, err))
    }

    /// Writes atomically with `0600` permissions on Unix.
    pub fn save(&self, path: &Path) -> io::Result<()> {
        let dir = path
            .parent()
            .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidInput, "path has no parent"))?;
        std::fs::create_dir_all(dir)?;

        let tmp = path.with_extension("json.tmp");
        let mut options = std::fs::OpenOptions::new();
        options.write(true).create(true).truncate(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            options.mode(0o600);
        }
        let mut file = options.open(&tmp)?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            file.set_permissions(std::fs::Permissions::from_mode(0o600))?;
        }
        file.write_all(serde_json::to_string_pretty(self)?.as_bytes())?;
        file.sync_all()?;
        std::fs::rename(&tmp, path)
    }
}

fn is_loopback(url: &Url) -> bool {
    match url.host() {
        Some(Host::Domain(domain)) => domain == "localhost",
        Some(Host::Ipv4(ip)) => ip.is_loopback(),
        Some(Host::Ipv6(ip)) => ip.is_loopback(),
        None => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn accepts_wss_and_local_ws() {
        for url in [
            "wss://aster.example.com/ws",
            "ws://localhost:8080/ws",
            "ws://127.0.0.1:8080/ws",
            "ws://[::1]:8080/ws",
        ] {
            assert!(Settings::new(url, "token").is_ok(), "{url}");
        }
    }

    #[test]
    fn rejects_unsafe_or_broken_settings() {
        for (url, token) in [
            ("ws://aster.example.com/ws", "token"),
            ("ws://192.168.1.10:8080/ws", "token"),
            ("https://aster.example.com/ws", "token"),
            ("not a url", "token"),
            ("wss://aster.example.com/ws", "   "),
        ] {
            assert!(Settings::new(url, token).is_err(), "{url} / {token:?}");
        }
    }

    #[test]
    fn save_and_load_round_trip_with_private_permissions() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("nested").join("settings.json");
        assert_eq!(Settings::load(&path).unwrap(), None);

        let settings = Settings::new("wss://aster.example.com/ws", "secret-token").unwrap();
        settings.save(&path).unwrap();
        assert_eq!(Settings::load(&path).unwrap(), Some(settings));

        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mode = std::fs::metadata(&path).unwrap().permissions().mode();
            assert_eq!(mode & 0o777, 0o600);
        }
    }

    #[test]
    fn load_rejects_tampered_file() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("settings.json");
        std::fs::write(
            &path,
            r#"{"server_url":"ws://evil.example.com/ws","token":"t"}"#,
        )
        .unwrap();
        assert!(Settings::load(&path).is_err());
    }

    #[test]
    fn debug_does_not_leak_token() {
        let settings = Settings::new("wss://a.example/ws", "secret-token").unwrap();
        assert!(!format!("{settings:?}").contains("secret-token"));
    }
}
