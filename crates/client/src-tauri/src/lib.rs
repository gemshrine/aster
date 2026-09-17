mod attention;
mod commands;
pub mod core;

use std::sync::Arc;

use tauri::Manager;
use tokio::sync::mpsc;

use crate::commands::AppState;
use crate::core::chat::{Chat, ChatStore};
use crate::core::settings::Settings;
use crate::core::signaling::{signaling, Timing};
use crate::core::voice::audio::AudioBackend;
use crate::core::voice::call::{voice, CallConfig};
use crate::core::voice::settings::VoiceSettings;

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        .plugin(tauri_plugin_notification::init())
        .on_window_event(attention::hide_on_close)
        .setup(|app| {
            let attention = Arc::new(attention::Attention::install(app.handle())?);
            let settings_path = app.path().app_config_dir()?.join("settings.json");
            let voice_settings_path = app.path().app_config_dir()?.join("voice.json");
            let history = ChatStore::open(&app.path().app_data_dir()?.join("history.sqlite3"))
                .map_err(|err| err.to_string())?;

            let (events_tx, mut events) = mpsc::unbounded_channel();
            let (signaling, actor) = signaling(events_tx, Timing::default());
            tauri::async_runtime::spawn(actor);

            let (voice_tx, mut voice_events) = mpsc::unbounded_channel();
            let audio = audio_backend();
            let (voice, voice_actor) = voice(
                signaling.clone(),
                audio.clone(),
                voice_tx,
                CallConfig::default(),
                VoiceSettings::load(&voice_settings_path),
            );
            tauri::async_runtime::spawn(voice_actor);

            let (upserts_tx, mut upserts) = mpsc::unbounded_channel();
            let chat = Arc::new(Chat::new(history, signaling.clone(), upserts_tx));

            let handle = app.handle().clone();
            let chat_events = chat.clone();
            let voice_events_in = voice.clone();
            let connection_tray = attention.clone();
            tauri::async_runtime::spawn(async move {
                while let Some(event) = events.recv().await {
                    if let crate::core::signaling::CoreEvent::State(state) = &event {
                        connection_tray.set_tooltip(&tray_tooltip(state));
                    }
                    commands::emit(&handle, event.clone());
                    voice_events_in.core_event(&event);
                    chat_events.handle_event(&event).await;
                }
            });

            let handle = app.handle().clone();
            let message_attention = attention.clone();
            tauri::async_runtime::spawn(async move {
                while let Some(message) = upserts.recv().await {
                    message_attention.message(&handle, &message);
                    commands::emit_upsert(&handle, message);
                }
            });

            let handle = app.handle().clone();
            let call_attention = attention.clone();
            tauri::async_runtime::spawn(async move {
                while let Some(event) = voice_events.recv().await {
                    if let crate::core::voice::call::VoiceEvent::State(state) = &event {
                        call_attention.call(&handle, state);
                    }
                    commands::emit_voice(&handle, event);
                }
            });

            match Settings::load(&settings_path) {
                Ok(Some(settings)) => signaling.connect(settings),
                Ok(None) => {}
                Err(err) => eprintln!("ignoring unreadable settings: {err}"),
            }

            app.manage(AppState {
                signaling,
                chat,
                voice,
                audio,
                settings_path,
                attention,
                voice_settings_path,
            });
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            commands::get_settings,
            commands::save_settings,
            commands::connect,
            commands::disconnect,
            commands::connection_state,
            commands::load_history,
            commands::send_message,
            commands::retry_message,
            commands::start_call,
            commands::accept_call,
            commands::decline_call,
            commands::hang_up,
            commands::set_muted,
            commands::call_state,
            commands::set_window_focused,
            commands::focus_window,
            commands::list_audio_devices,
            commands::get_voice_settings,
            commands::set_voice_settings,
        ])
        .run(tauri::generate_context!())
        .expect("error while running Aster client");
}

/// Connection state as the tray tooltip words it.
fn tray_tooltip(state: &crate::core::signaling::ConnectionState) -> String {
    use crate::core::signaling::ConnectionState::*;
    match state {
        NotConfigured => "Aster — not configured".into(),
        Connecting { .. } => "Aster — connecting".into(),
        Connected {
            peer_online: true, ..
        } => "Aster — peer online".into(),
        Connected { .. } => "Aster — connected".into(),
        Reconnecting { .. } => "Aster — reconnecting".into(),
        Disconnected => "Aster — disconnected".into(),
        Failed { .. } => "Aster — connection failed".into(),
    }
}

#[cfg(feature = "audio-device")]
fn audio_backend() -> Arc<dyn AudioBackend> {
    Arc::new(crate::core::voice::audio::CpalBackend)
}

#[cfg(not(feature = "audio-device"))]
fn audio_backend() -> Arc<dyn AudioBackend> {
    eprintln!("built without audio-device: calls connect without sound");
    Arc::new(crate::core::voice::audio::NullBackend)
}
