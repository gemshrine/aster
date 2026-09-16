mod commands;
pub mod core;

use tauri::Manager;
use tokio::sync::mpsc;

use crate::commands::AppState;
use crate::core::settings::Settings;
use crate::core::signaling::{signaling, Timing};

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        .setup(|app| {
            let settings_path = app.path().app_config_dir()?.join("settings.json");

            let (events_tx, mut events) = mpsc::unbounded_channel();
            let (signaling, actor) = signaling(events_tx, Timing::default());
            tauri::async_runtime::spawn(actor);

            let handle = app.handle().clone();
            tauri::async_runtime::spawn(async move {
                while let Some(event) = events.recv().await {
                    commands::emit(&handle, event);
                }
            });

            match Settings::load(&settings_path) {
                Ok(Some(settings)) => signaling.connect(settings),
                Ok(None) => {}
                Err(err) => eprintln!("ignoring unreadable settings: {err}"),
            }

            app.manage(AppState {
                signaling,
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
            commands::send_chat,
        ])
        .run(tauri::generate_context!())
        .expect("error while running Aster client");
}
