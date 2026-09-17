//! `voice.json`, see `specs/0013-voice-settings.md`.

use std::io;
use std::path::Path;

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct VoiceSettings {
    /// Device id from `list_audio_devices`; `None` is the system default.
    pub input_device: Option<String>,
    pub output_device: Option<String>,
}

impl VoiceSettings {
    /// Missing or unreadable files yield defaults: a broken settings file
    /// must not keep the user from calling.
    pub fn load(path: &Path) -> Self {
        match std::fs::read_to_string(path) {
            Ok(raw) => match serde_json::from_str::<VoiceSettings>(&raw) {
                Ok(settings) => settings.normalized(),
                Err(err) => {
                    eprintln!("ignoring unreadable voice settings: {err}");
                    Self::default()
                }
            },
            Err(err) if err.kind() == io::ErrorKind::NotFound => Self::default(),
            Err(err) => {
                eprintln!("ignoring unreadable voice settings: {err}");
                Self::default()
            }
        }
    }

    pub fn save(&self, path: &Path) -> io::Result<()> {
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir)?;
        }
        let tmp = path.with_extension("json.tmp");
        std::fs::write(&tmp, serde_json::to_string_pretty(self)?)?;
        std::fs::rename(tmp, path)
    }

    pub fn normalized(mut self) -> Self {
        for device in [&mut self.input_device, &mut self.output_device] {
            if device.as_deref().is_some_and(|id| id.trim().is_empty()) {
                *device = None;
            }
        }
        self
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn defaults_for_missing_broken_and_partial_files() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("voice.json");
        assert_eq!(VoiceSettings::load(&path), VoiceSettings::default());

        std::fs::write(&path, "{ not json").unwrap();
        assert_eq!(VoiceSettings::load(&path), VoiceSettings::default());

        std::fs::write(
            &path,
            r#"{"output_device":"alsa:hw:1","from_the_future":true}"#,
        )
        .unwrap();
        assert_eq!(
            VoiceSettings::load(&path),
            VoiceSettings {
                input_device: None,
                output_device: Some("alsa:hw:1".into()),
            }
        );
    }

    #[test]
    fn save_round_trips_and_blank_ids_mean_default() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("nested").join("voice.json");
        let settings = VoiceSettings {
            input_device: Some("  ".into()),
            output_device: Some("pulse:headphones".into()),
        }
        .normalized();
        assert_eq!(settings.input_device, None);

        settings.save(&path).unwrap();
        assert_eq!(VoiceSettings::load(&path), settings);
    }
}
