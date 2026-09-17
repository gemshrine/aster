//! Global push-to-talk key: the GlobalShortcuts portal on Wayland, a global
//! shortcut elsewhere. See `specs/0013-voice-settings.md`.

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

use serde::Serialize;
use tauri::{AppHandle, Emitter, Manager};
use tokio::sync::oneshot;

use crate::core::voice::call::{PushToTalkSource, VoiceHandle};
use crate::core::voice::settings::{InputMode, VoiceSettings};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum GlobalSource {
    Portal,
    Shortcut,
    Unavailable,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct PushToTalkStatus {
    pub global: GlobalSource,
    pub reason: Option<String>,
}

impl PushToTalkStatus {
    fn available(global: GlobalSource) -> Self {
        Self {
            global,
            reason: None,
        }
    }

    fn unavailable(reason: impl Into<String>) -> Self {
        Self {
            global: GlobalSource::Unavailable,
            reason: Some(reason.into()),
        }
    }
}

/// X11 key grabs see nothing on Wayland, so there only the portal works.
pub fn is_wayland() -> bool {
    cfg!(target_os = "linux") && std::env::var_os("WAYLAND_DISPLAY").is_some()
}

pub struct PushToTalk {
    voice: VoiceHandle,
    status: Mutex<PushToTalkStatus>,
    /// Stops the running portal session.
    portal: Mutex<Option<oneshot::Sender<()>>>,
    /// Bumped on every change so a slow portal setup cannot report over a
    /// newer state.
    generation: AtomicU64,
}

impl PushToTalk {
    pub fn new(voice: VoiceHandle) -> Arc<Self> {
        Arc::new(Self {
            voice,
            status: Mutex::new(PushToTalkStatus::unavailable("push-to-talk is off")),
            portal: Mutex::new(None),
            generation: AtomicU64::new(0),
        })
    }

    pub fn status(&self) -> PushToTalkStatus {
        self.status.lock().unwrap().clone()
    }

    /// Call at startup and after every settings change.
    pub fn apply(self: &Arc<Self>, app: &AppHandle, settings: &VoiceSettings) {
        let generation = self.generation.fetch_add(1, Ordering::Relaxed) + 1;
        if settings.input_mode != InputMode::PushToTalk {
            self.stop(app);
            self.set_status(app, PushToTalkStatus::unavailable("push-to-talk is off"));
            return;
        }
        if is_wayland() {
            self.start_portal(app, generation);
        } else {
            self.register_shortcut(app, settings.ptt_shortcut.as_deref());
        }
    }

    fn stop(&self, app: &AppHandle) {
        if let Some(stop) = self.portal.lock().unwrap().take() {
            let _ = stop.send(());
        }
        if let Some(shortcuts) =
            app.try_state::<tauri_plugin_global_shortcut::GlobalShortcut<tauri::Wry>>()
        {
            let _ = shortcuts.unregister_all();
        }
        self.voice.set_push_to_talk(PushToTalkSource::Global, false);
    }

    fn set_status(&self, app: &AppHandle, status: PushToTalkStatus) {
        let mut current = self.status.lock().unwrap();
        if *current != status {
            *current = status.clone();
            if let Err(err) = app.emit("push-to-talk-status", status) {
                eprintln!("failed to emit event: {err}");
            }
        }
    }

    fn register_shortcut(&self, app: &AppHandle, accelerator: Option<&str>) {
        use tauri_plugin_global_shortcut::{GlobalShortcut, ShortcutState};

        let Some(shortcuts) = app.try_state::<GlobalShortcut<tauri::Wry>>() else {
            self.set_status(
                app,
                PushToTalkStatus::unavailable("global shortcuts are not supported here"),
            );
            return;
        };
        let _ = shortcuts.unregister_all();
        self.voice.set_push_to_talk(PushToTalkSource::Global, false);
        let Some(accelerator) = accelerator else {
            self.set_status(app, PushToTalkStatus::unavailable("no shortcut set"));
            return;
        };
        let voice = self.voice.clone();
        let status = match shortcuts.on_shortcut(accelerator, move |_, _, event| {
            voice.set_push_to_talk(
                PushToTalkSource::Global,
                event.state == ShortcutState::Pressed,
            );
        }) {
            Ok(()) => PushToTalkStatus::available(GlobalSource::Shortcut),
            Err(err) => PushToTalkStatus::unavailable(format!("{accelerator}: {err}")),
        };
        self.set_status(app, status);
    }

    #[cfg(target_os = "linux")]
    fn start_portal(self: &Arc<Self>, app: &AppHandle, generation: u64) {
        let mut running = self.portal.lock().unwrap();
        if running.as_ref().is_some_and(|stop| !stop.is_closed()) {
            return;
        }
        let (stop_tx, stop) = oneshot::channel();
        *running = Some(stop_tx);
        drop(running);

        let this = self.clone();
        let app = app.clone();
        tauri::async_runtime::spawn(async move {
            let voice = this.voice.clone();
            let ready = || {
                if this.generation.load(Ordering::Relaxed) == generation {
                    this.set_status(&app, PushToTalkStatus::available(GlobalSource::Portal));
                }
            };
            let press = |pressed| voice.set_push_to_talk(PushToTalkSource::Global, pressed);
            if let Err(err) = portal::run(ready, press, stop).await {
                eprintln!("push-to-talk portal: {err}");
                if this.generation.load(Ordering::Relaxed) == generation {
                    this.set_status(
                        &app,
                        PushToTalkStatus::unavailable(format!("GlobalShortcuts portal: {err}")),
                    );
                }
            }
        });
    }

    #[cfg(not(target_os = "linux"))]
    fn start_portal(self: &Arc<Self>, app: &AppHandle, _generation: u64) {
        self.set_status(
            app,
            PushToTalkStatus::unavailable("no portal on this system"),
        );
    }
}

#[cfg(target_os = "linux")]
mod portal {
    use ashpd::desktop::global_shortcuts::{GlobalShortcuts, NewShortcut};
    use futures_util::StreamExt;
    use tokio::sync::oneshot;

    const APP_ID: &str = "dev.gemshrine.aster";
    const SHORTCUT_ID: &str = "push-to-talk";

    /// Binds the shortcut and forwards presses until `stop` fires or the
    /// portal goes away.
    pub async fn run(
        ready: impl FnOnce(),
        press: impl Fn(bool),
        mut stop: oneshot::Receiver<()>,
    ) -> ashpd::Result<()> {
        // Needs an installed `dev.gemshrine.aster.desktop`; without it the
        // portal sees an empty app id (Hyprland: `global, :push-to-talk`).
        if let Ok(app_id) = APP_ID.try_into() {
            if let Err(err) = ashpd::register_host_app(app_id).await {
                eprintln!("push-to-talk portal runs without an app id: {err}");
            }
        }
        let portal = GlobalShortcuts::new().await?;
        let session = portal.create_session(Default::default()).await?;
        let mut activated = portal.receive_activated().await?;
        let mut deactivated = portal.receive_deactivated().await?;
        portal
            .bind_shortcuts(
                &session,
                &[NewShortcut::new(SHORTCUT_ID, "Push-to-talk")],
                None,
                Default::default(),
            )
            .await?
            .response()?;
        ready();

        // The portal sends these only to the session owner, and this
        // process has one session.
        loop {
            tokio::select! {
                Some(event) = activated.next() => {
                    if event.shortcut_id() == SHORTCUT_ID {
                        press(true);
                    }
                }
                Some(event) = deactivated.next() => {
                    if event.shortcut_id() == SHORTCUT_ID {
                        press(false);
                    }
                }
                _ = &mut stop => break,
                else => break,
            }
        }
        press(false);
        let _ = session.close().await;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn status_json_matches_spec() {
        assert_eq!(
            serde_json::to_value(PushToTalkStatus::available(GlobalSource::Portal)).unwrap(),
            serde_json::json!({ "global": "portal", "reason": null })
        );
        assert_eq!(
            serde_json::to_value(PushToTalkStatus::unavailable("no shortcut set")).unwrap(),
            serde_json::json!({ "global": "unavailable", "reason": "no shortcut set" })
        );
    }

    /// Binds the shortcut in the running desktop session:
    /// `cargo test -p client-tauri --lib binds_through_the_portal -- --ignored`.
    /// With `DBUS_SESSION_BUS_ADDRESS=unix:path=/nonexistent` it must fail
    /// instead of hanging.
    #[cfg(target_os = "linux")]
    #[tokio::test]
    #[ignore = "needs a desktop session with the GlobalShortcuts portal"]
    async fn binds_through_the_portal() {
        let (stop_tx, stop) = oneshot::channel();
        let (ready_tx, ready) = oneshot::channel();
        let session = tokio::spawn(portal::run(
            move || {
                let _ = ready_tx.send(());
            },
            |_| {},
            stop,
        ));
        let bound = tokio::time::timeout(std::time::Duration::from_secs(10), ready).await;
        let _ = stop_tx.send(());
        let result = session.await.unwrap();
        println!("portal result: {result:?}");
        assert!(
            bound.is_ok_and(|ready| ready.is_ok()),
            "not bound: {result:?}"
        );
        result.unwrap();
    }
}
