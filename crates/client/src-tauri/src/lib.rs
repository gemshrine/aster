mod attention;
mod commands;
pub mod core;
pub mod logging;
mod push_to_talk;

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
        // Must be the first plugin: a second launch has to hand over to the
        // running instance before anything else starts (#80).
        .plugin(tauri_plugin_single_instance::init(|app, _argv, _cwd| {
            attention::focus_window(app);
        }))
        .plugin(tauri_plugin_notification::init())
        .on_window_event(attention::hide_on_close)
        .setup(|app| {
            if let Ok(dir) = app.path().app_log_dir() {
                logging::init(&dir.join("aster.log"));
            }
            crate::log_line!("aster client starting");
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
            let voice_settings = VoiceSettings::load(&voice_settings_path);
            let (voice, voice_actor) = voice(
                signaling.clone(),
                audio.clone(),
                voice_tx,
                CallConfig::default(),
                voice_settings.clone(),
            );
            tauri::async_runtime::spawn(voice_actor);

            if !push_to_talk::is_wayland() {
                // Its X11 key grab fails without a display; PTT then reports
                // `unavailable` rather than the app failing to start.
                let plugin = tauri_plugin_global_shortcut::Builder::new().build();
                if let Err(err) = app.handle().plugin(plugin) {
                    crate::log_line!("global shortcuts unavailable: {err}");
                }
            }
            let push_to_talk = push_to_talk::PushToTalk::new(voice.clone());
            push_to_talk.apply(app.handle(), &voice_settings);

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
                // Only a panic or a closed core ends this loop, and then the
                // window stops hearing about anything (#81).
                crate::log_line!("core event loop stopped");
            });

            let handle = app.handle().clone();
            let message_attention = attention.clone();
            tauri::async_runtime::spawn(async move {
                while let Some(message) = upserts.recv().await {
                    message_attention.message(&handle, &message);
                    commands::emit_upsert(&handle, message);
                }
                crate::log_line!("message event loop stopped");
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
                crate::log_line!("voice event loop stopped");
            });

            match Settings::load(&settings_path) {
                Ok(Some(settings)) => signaling.connect(settings),
                Ok(None) => {}
                Err(err) => crate::log_line!("ignoring unreadable settings: {err}"),
            }

            app.manage(AppState {
                signaling,
                chat,
                voice,
                audio,
                settings_path,
                attention,
                voice_settings_path,
                push_to_talk,
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
            commands::open_log_dir,
            commands::check_turn,
            commands::list_audio_devices,
            commands::get_voice_settings,
            commands::set_voice_settings,
            commands::set_input_monitor,
            commands::set_push_to_talk,
            commands::push_to_talk_status,
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
    crate::log_line!("built without audio-device: calls connect without sound");
    Arc::new(crate::core::voice::audio::NullBackend)
}
