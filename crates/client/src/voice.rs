//! Voice settings as the core stores them (spec 0013).

use serde::{Deserialize, Serialize};

/// Quietest level the meter shows; everything below reads as silence.
const FLOOR_DBFS: f32 = -70.0;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum InputMode {
    #[default]
    VoiceActivity,
    PushToTalk,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum NoiseSuppression {
    Off,
    #[default]
    Standard,
    Strong,
}

impl NoiseSuppression {
    pub const ALL: [Self; 3] = [Self::Off, Self::Standard, Self::Strong];

    pub fn label(self) -> &'static str {
        match self {
            Self::Off => "Off",
            Self::Standard => "Standard",
            Self::Strong => "Strong (RNNoise)",
        }
    }

    pub fn id(self) -> &'static str {
        match self {
            Self::Off => "off",
            Self::Standard => "standard",
            Self::Strong => "strong",
        }
    }

    pub fn from_id(id: &str) -> Self {
        Self::ALL
            .into_iter()
            .find(|value| value.id() == id)
            .unwrap_or_default()
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct VoiceSettings {
    pub input_device: Option<String>,
    pub output_device: Option<String>,
    pub echo_cancellation: bool,
    pub noise_suppression: NoiseSuppression,
    pub auto_gain: bool,
    pub vad_threshold_dbfs: i32,
    pub input_mode: InputMode,
    pub ptt_release_delay_ms: u32,
    pub ptt_shortcut: Option<String>,
}

impl Default for VoiceSettings {
    fn default() -> Self {
        Self {
            input_device: None,
            output_device: None,
            echo_cancellation: true,
            noise_suppression: NoiseSuppression::Standard,
            auto_gain: true,
            vad_threshold_dbfs: -45,
            input_mode: InputMode::VoiceActivity,
            ptt_release_delay_ms: 200,
            ptt_shortcut: None,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Deserialize)]
pub struct AudioDevice {
    pub id: String,
    pub name: String,
    pub is_default: bool,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Deserialize)]
pub struct AudioDevices {
    pub inputs: Vec<AudioDevice>,
    pub outputs: Vec<AudioDevice>,
}

/// Where a global push-to-talk key comes from, if anywhere (spec 0013).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum GlobalSource {
    /// Wayland global shortcuts portal — the key is bound in the compositor.
    Portal,
    /// A global accelerator registered by the app itself.
    Shortcut,
    #[default]
    #[serde(other)]
    Unavailable,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Deserialize)]
pub struct PushToTalkStatus {
    pub global: GlobalSource,
    pub reason: Option<String>,
}

/// Position of `dbfs` on the meter, 0…100.
pub fn level_percent(dbfs: f32) -> f32 {
    ((dbfs - FLOOR_DBFS) / -FLOOR_DBFS * 100.0).clamp(0.0, 100.0)
}

/// Hyprland binds the portal shortcut by name; `app_id` is empty for a build
/// without an installed desktop file (spec 0013).
pub fn hyprland_bind(app_id: Option<&str>) -> String {
    format!(
        "bind = , F13, global, {}:push-to-talk",
        app_id.unwrap_or_default()
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_meter_spans_the_floor_to_full_scale() {
        assert_eq!(level_percent(0.0), 100.0);
        assert_eq!(level_percent(-35.0), 50.0);
        assert_eq!(level_percent(-70.0), 0.0);
    }

    #[test]
    fn levels_outside_the_scale_are_clamped() {
        assert_eq!(level_percent(-120.0), 0.0);
        assert_eq!(level_percent(6.0), 100.0);
    }

    #[test]
    fn noise_suppression_round_trips_through_its_id() {
        for value in NoiseSuppression::ALL {
            assert_eq!(NoiseSuppression::from_id(value.id()), value);
        }
        // An unknown value from a newer core falls back to the default.
        assert_eq!(
            NoiseSuppression::from_id("hyper"),
            NoiseSuppression::Standard
        );
    }

    #[test]
    fn an_unknown_global_source_reads_as_unavailable() {
        let status: PushToTalkStatus =
            serde_json::from_str(r#"{"global":"telepathy","reason":null}"#).unwrap();
        assert_eq!(status.global, GlobalSource::Unavailable);
    }

    #[test]
    fn settings_parse_from_the_core_payload() {
        let settings: VoiceSettings = serde_json::from_str(
            r#"{"input_device":null,"output_device":"hdmi","echo_cancellation":true,
                "noise_suppression":"strong","auto_gain":false,"vad_threshold_dbfs":-50,
                "input_mode":"push_to_talk","ptt_release_delay_ms":300,"ptt_shortcut":"F13"}"#,
        )
        .unwrap();
        assert_eq!(settings.noise_suppression, NoiseSuppression::Strong);
        assert_eq!(settings.input_mode, InputMode::PushToTalk);
        assert_eq!(settings.output_device.as_deref(), Some("hdmi"));
    }

    #[test]
    fn the_hyprland_hint_names_the_app_id_when_there_is_one() {
        assert_eq!(
            hyprland_bind(Some("dev.gemshrine.aster")),
            "bind = , F13, global, dev.gemshrine.aster:push-to-talk"
        );
        assert_eq!(hyprland_bind(None), "bind = , F13, global, :push-to-talk");
    }
}
