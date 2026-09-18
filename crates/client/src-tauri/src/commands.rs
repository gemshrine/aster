//! Tauri commands and events, see `specs/0009-client-core.md`.

use std::path::PathBuf;
use std::sync::Arc;

use serde::Serialize;
use tauri::{AppHandle, Emitter, Manager, State};
use uuid::Uuid;

use crate::core::chat::{Chat, ChatError, Message};
use crate::core::settings::Settings;
use crate::core::signaling::{ConnectionState, CoreEvent, SignalingHandle};
use crate::core::voice::audio::{AudioBackend, AudioDevices};
use crate::core::voice::call::{
    CallError, CallState, PushToTalkSource, TurnCheck, VoiceEvent, VoiceHandle,
};
use crate::core::voice::settings::VoiceSettings;
use crate::push_to_talk::PushToTalkStatus;

pub struct AppState {
    pub attention: std::sync::Arc<crate::attention::Attention>,
    pub signaling: SignalingHandle,
    pub chat: Arc<Chat>,
    pub voice: VoiceHandle,
    pub audio: Arc<dyn AudioBackend>,
    pub settings_path: PathBuf,
    pub voice_settings_path: PathBuf,
    pub push_to_talk: Arc<crate::push_to_talk::PushToTalk>,
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

impl From<CallError> for CommandError {
    fn from(err: CallError) -> Self {
        let (code, message) = match err {
            CallError::InvalidState => ("invalid_state", "not possible in the current call state"),
            CallError::NotConnected => ("not_connected", "not connected to the server"),
            CallError::PeerOffline => ("peer_offline", "the other user is offline"),
            CallError::Failed => ("failed", "could not set up the call"),
        };
        CommandError::new(code, message)
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

#[tauri::command(rename_all = "snake_case")]
pub async fn start_call(state: State<'_, AppState>) -> Result<(), CommandError> {
    Ok(state.voice.start_call().await?)
}

#[tauri::command(rename_all = "snake_case")]
pub async fn accept_call(state: State<'_, AppState>) -> Result<(), CommandError> {
    Ok(state.voice.accept_call().await?)
}

#[tauri::command(rename_all = "snake_case")]
pub async fn decline_call(state: State<'_, AppState>) -> Result<(), CommandError> {
    Ok(state.voice.decline_call().await?)
}

#[tauri::command(rename_all = "snake_case")]
pub async fn hang_up(state: State<'_, AppState>) -> Result<(), CommandError> {
    Ok(state.voice.hang_up().await?)
}

#[tauri::command(rename_all = "snake_case")]
pub async fn set_muted(state: State<'_, AppState>, muted: bool) -> Result<(), CommandError> {
    Ok(state.voice.set_muted(muted).await?)
}

#[tauri::command(rename_all = "snake_case")]
pub fn call_state(state: State<'_, AppState>) -> CallState {
    state.voice.state()
}

#[tauri::command(rename_all = "snake_case")]
pub async fn list_audio_devices(state: State<'_, AppState>) -> Result<AudioDevices, CommandError> {
    let audio = state.audio.clone();
    // Device enumeration can block on the sound server.
    tauri::async_runtime::spawn_blocking(move || audio.devices())
        .await
        .map_err(|err| CommandError::new("io", err.to_string()))
}

#[tauri::command(rename_all = "snake_case")]
pub fn get_voice_settings(state: State<'_, AppState>) -> VoiceSettings {
    VoiceSettings::load(&state.voice_settings_path)
}

#[tauri::command(rename_all = "snake_case")]
pub async fn set_voice_settings(
    state: State<'_, AppState>,
    app: AppHandle,
    settings: VoiceSettings,
) -> Result<VoiceSettings, CommandError> {
    let settings = settings.normalized();
    settings
        .save(&state.voice_settings_path)
        .map_err(|err| CommandError::new("io", err.to_string()))?;
    state.voice.set_settings(settings.clone()).await?;
    state.push_to_talk.apply(&app, &settings);
    Ok(settings)
}

/// The push-to-talk key as seen by the focused window.
#[tauri::command(rename_all = "snake_case")]
pub fn set_push_to_talk(state: State<'_, AppState>, pressed: bool) {
    state
        .voice
        .set_push_to_talk(PushToTalkSource::Window, pressed);
}

#[tauri::command(rename_all = "snake_case")]
pub fn push_to_talk_status(state: State<'_, AppState>) -> PushToTalkStatus {
    state.push_to_talk.status()
}

#[tauri::command(rename_all = "snake_case")]
pub async fn set_input_monitor(
    state: State<'_, AppState>,
    enabled: bool,
) -> Result<(), CommandError> {
    Ok(state.voice.set_input_monitor(enabled).await?)
}

#[derive(Clone, Serialize)]
struct AudioError {
    direction: crate::core::voice::call::AudioDirection,
    message: String,
}

#[derive(Clone, Serialize)]
struct InputLevel {
    dbfs: f32,
}

pub fn emit_voice(app: &AppHandle, event: VoiceEvent) {
    let result = match event {
        VoiceEvent::State(state) => app.emit("call-state", state),
        VoiceEvent::Audio(status) => app.emit("call-audio", status),
        VoiceEvent::AudioError { direction, message } => {
            app.emit("call-audio-error", AudioError { direction, message })
        }
        VoiceEvent::InputLevel { dbfs } => app.emit("input-level", InputLevel { dbfs }),
        VoiceEvent::Transport(status) => app.emit("call-transport", status),
    };
    if let Err(err) = result {
        crate::log_line!("failed to emit event: {err}");
    }
}

#[tauri::command(rename_all = "snake_case")]
pub fn set_window_focused(state: State<'_, AppState>, app: AppHandle, focused: bool) {
    state.attention.set_focused(&app, focused);
    if !focused {
        // A key released outside the window never reaches it.
        state
            .voice
            .set_push_to_talk(PushToTalkSource::Window, false);
    }
}

/// Gathers candidates on an empty connection: tells STUN/TURN trouble apart
/// from a problem between the two peers (specs/0015).
#[tauri::command(rename_all = "snake_case")]
pub async fn check_turn(state: State<'_, AppState>) -> Result<TurnCheck, CommandError> {
    crate::core::voice::call::check_turn(
        &state.signaling,
        &crate::core::voice::call::CallConfig::default(),
        std::time::Duration::from_secs(5),
    )
    .await
    .map_err(|err| CommandError::new("failed", err))
}

/// Opens the folder holding `aster.log` so it can be sent over (specs/0015).
#[tauri::command(rename_all = "snake_case")]
pub fn open_log_dir(app: AppHandle) -> Result<(), CommandError> {
    let dir = app
        .path()
        .app_log_dir()
        .map_err(|err| CommandError::new("io", err.to_string()))?;
    let opener = if cfg!(target_os = "macos") {
        "open"
    } else if cfg!(target_os = "windows") {
        "explorer"
    } else {
        "xdg-open"
    };
    std::process::Command::new(opener)
        .arg(&dir)
        .spawn()
        .map(|_| ())
        .map_err(|err| CommandError::new("io", format!("{opener}: {err}")))
}

#[tauri::command(rename_all = "snake_case")]
pub fn focus_window(app: AppHandle) {
    crate::attention::focus_window(&app);
}

#[derive(Clone, Serialize)]
struct PeerStatus {
    online: bool,
}

pub fn emit_upsert(app: &AppHandle, message: Message) {
    if let Err(err) = app.emit("message-upserted", message) {
        crate::log_line!("failed to emit event: {err}");
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
            crate::log_line!("signaling-server error {code:?}: {message}");
            Ok(())
        }
    };
    if let Err(err) = result {
        crate::log_line!("failed to emit event: {err}");
    }
}
