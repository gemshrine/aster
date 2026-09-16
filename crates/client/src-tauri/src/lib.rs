mod commands;
pub mod core;

use std::sync::Arc;

use tauri::Manager;
use tokio::sync::mpsc;

use crate::commands::AppState;
use crate::core::chat::{Chat, ChatStore};
use crate::core::settings::Settings;
use crate::core::signaling::{signaling, Timing};

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        .setup(|app| {
            let settings_path = app.path().app_config_dir()?.join("settings.json");
            let history = ChatStore::open(&app.path().app_data_dir()?.join("history.sqlite3"))
                .map_err(|err| err.to_string())?;

            let (events_tx, mut events) = mpsc::unbounded_channel();
            let (signaling, actor) = signaling(events_tx, Timing::default());
            tauri::async_runtime::spawn(actor);

            let (upserts_tx, mut upserts) = mpsc::unbounded_channel();
            let chat = Arc::new(Chat::new(history, signaling.clone(), upserts_tx));

            let handle = app.handle().clone();
            let chat_events = chat.clone();
            tauri::async_runtime::spawn(async move {
                while let Some(event) = events.recv().await {
                    commands::emit(&handle, event.clone());
                    chat_events.handle_event(&event).await;
                }
            });

            let handle = app.handle().clone();
            tauri::async_runtime::spawn(async move {
                while let Some(message) = upserts.recv().await {
                    commands::emit_upsert(&handle, message);
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
                settings_path,
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
        ])
        .run(tauri::generate_context!())
        .expect("error while running Aster client");
}
