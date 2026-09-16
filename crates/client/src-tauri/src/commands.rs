//! Tauri commands and events, see `specs/0009-client-core.md`.

use std::path::PathBuf;

use serde::Serialize;
use tauri::{AppHandle, Emitter, State};
use uuid::Uuid;

use crate::core::settings::Settings;
use crate::core::signaling::{AckStatus, ConnectionState, CoreEvent, SendError, SignalingHandle};

pub struct AppState {
    pub signaling: SignalingHandle,
    pub settings_path: PathBuf,
}

#[derive(Debug, Serialize)]
pub struct CommandError {
    code: &'static str,
    message: String,
}

impl CommandError {
    fn new(code: &'static str, message: impl Into<String>) -> Self {
        Self {
            code,
            message: message.into(),
        }
    }
}

#[derive(Serialize)]
pub struct SettingsView {
    server_url: Option<String>,
    has_token: bool,
}

#[tauri::command(rename_all = "snake_case")]
pub fn get_settings(state: State<'_, AppState>) -> Result<SettingsView, CommandError> {
    let settings = Settings::load(&state.settings_path)
        .map_err(|err| CommandError::new("io", err.to_string()))?;
    Ok(SettingsView {
        has_token: settings.is_some(),
        server_url: settings.map(|s| s.server_url),
    })
}

#[tauri::command(rename_all = "snake_case")]
pub fn save_settings(
    state: State<'_, AppState>,
    server_url: String,
    token: String,
) -> Result<(), CommandError> {
    let settings = Settings::new(&server_url, &token)
        .map_err(|err| CommandError::new("invalid_settings", err))?;
    settings
        .save(&state.settings_path)
        .map_err(|err| CommandError::new("io", err.to_string()))?;
    state.signaling.connect(settings);
    Ok(())
}

#[tauri::command(rename_all = "snake_case")]
pub fn connect(state: State<'_, AppState>) -> Result<(), CommandError> {
    let settings = Settings::load(&state.settings_path)
        .map_err(|err| CommandError::new("io", err.to_string()))?
        .ok_or_else(|| CommandError::new("not_configured", "server url and token are not set"))?;
    state.signaling.connect(settings);
    Ok(())
}

#[tauri::command(rename_all = "snake_case")]
pub fn disconnect(state: State<'_, AppState>) {
    state.signaling.disconnect();
}

#[tauri::command(rename_all = "snake_case")]
pub fn connection_state(state: State<'_, AppState>) -> ConnectionState {
    state.signaling.state()
}

#[tauri::command(rename_all = "snake_case")]
pub async fn send_chat(
    state: State<'_, AppState>,
    id: Uuid,
    body: String,
    sent_at: i64,
) -> Result<(), CommandError> {
    state
        .signaling
        .send_chat(id, body, sent_at)
        .await
        .map_err(|err| match err {
            SendError::NotConnected => CommandError::new("not_connected", "not connected"),
            SendError::Io => CommandError::new("io", "connection lost while sending"),
        })
}

#[derive(Clone, Serialize)]
struct PeerStatus {
    online: bool,
}

#[derive(Clone, Serialize)]
struct ChatMessage {
    id: Uuid,
    from: String,
    body: String,
    sent_at: i64,
}

#[derive(Clone, Serialize)]
struct ChatAck {
    id: Uuid,
    status: AckStatus,
}

#[derive(Clone, Serialize)]
struct ChatRejected {
    id: Option<Uuid>,
    code: protocol::ErrorCode,
    message: String,
}

/// Forwards UI-facing core events to every window.
pub fn emit(app: &AppHandle, event: CoreEvent) {
    let result = match event {
        CoreEvent::State(state) => app.emit("connection-state", state),
        CoreEvent::PeerStatus { online } => app.emit("peer-status", PeerStatus { online }),
        CoreEvent::ChatMessage {
            id,
            from,
            body,
            sent_at,
        } => app.emit(
            "chat-message",
            ChatMessage {
                id,
                from,
                body,
                sent_at,
            },
        ),
        CoreEvent::ChatAck { id, status } => app.emit("chat-ack", ChatAck { id, status }),
        CoreEvent::ChatRejected { id, code, message } => {
            app.emit("chat-rejected", ChatRejected { id, code, message })
        }
        CoreEvent::Signal(_) => Ok(()),
        CoreEvent::ServerError { code, message } => {
            eprintln!("signaling-server error {code:?}: {message}");
            Ok(())
        }
    };
    if let Err(err) = result {
        eprintln!("failed to emit event: {err}");
    }
}
