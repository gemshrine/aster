//! Tauri commands and events, see `specs/0009-client-core.md`.

use std::path::PathBuf;
use std::sync::Arc;

use serde::Serialize;
use tauri::{AppHandle, Emitter, State};
use uuid::Uuid;

use crate::core::chat::{Chat, ChatError, Message};
use crate::core::settings::Settings;
use crate::core::signaling::{ConnectionState, CoreEvent, SignalingHandle};

pub struct AppState {
    pub signaling: SignalingHandle,
    pub chat: Arc<Chat>,
    pub settings_path: PathBuf,
}

#[derive(Debug, Serialize)]
pub struct CommandError {
    code: &'static str,
    message: String,
}

impl From<ChatError> for CommandError {
    fn from(err: ChatError) -> Self {
        CommandError::new(err.code(), err.to_string())
    }
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
pub fn load_history(
    state: State<'_, AppState>,
    before: Option<Uuid>,
    limit: u32,
) -> Result<Vec<Message>, CommandError> {
    Ok(state.chat.load_history(before, limit)?)
}

#[tauri::command(rename_all = "snake_case")]
pub async fn send_message(
    state: State<'_, AppState>,
    body: String,
) -> Result<Message, CommandError> {
    Ok(state.chat.send_message(&body).await?)
}

#[tauri::command(rename_all = "snake_case")]
pub async fn retry_message(state: State<'_, AppState>, id: Uuid) -> Result<Message, CommandError> {
    Ok(state.chat.retry_message(id).await?)
}

#[derive(Clone, Serialize)]
struct PeerStatus {
    online: bool,
}

pub fn emit_upsert(app: &AppHandle, message: Message) {
    if let Err(err) = app.emit("message-upserted", message) {
        eprintln!("failed to emit event: {err}");
    }
}

/// Forwards UI-facing core events to every window.
pub fn emit(app: &AppHandle, event: CoreEvent) {
    let result = match event {
        CoreEvent::State(state) => app.emit("connection-state", state),
        CoreEvent::PeerStatus { online } => app.emit("peer-status", PeerStatus { online }),
        // Chat events reach the UI as `message-upserted` via the history.
        CoreEvent::ChatMessage { .. }
        | CoreEvent::ChatAck { .. }
        | CoreEvent::ChatRejected { .. } => Ok(()),
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
