//! Tray, notifications and the unread counter (spec 0012).
//!
//! The window is never really closed — it hides, so a call still reaches the
//! user while Aster sits in the tray.

use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};

use serde::Serialize;
use tauri::image::Image;
use tauri::menu::{Menu, MenuItem};
use tauri::tray::{MouseButton, MouseButtonState, TrayIcon, TrayIconBuilder, TrayIconEvent};
use tauri::{AppHandle, Emitter, Manager, WindowEvent};
use tauri_plugin_notification::NotificationExt;

use crate::core::chat::{Direction, Message};
use crate::core::voice::call::CallState;

/// Longest message body a notification shows.
const NOTIFICATION_BODY: usize = 120;

#[derive(Clone, Serialize)]
struct Unread {
    count: u32,
}

/// Focus and unread state, plus the tray icon that displays it.
pub struct Attention {
    focused: AtomicBool,
    unread: AtomicU32,
    tray: TrayIcon,
    plain: Image<'static>,
    marked: Image<'static>,
}

impl Attention {
    /// Builds the tray icon and its menu.
    pub fn install(app: &AppHandle) -> tauri::Result<Self> {
        let open = MenuItem::with_id(app, "open", "Open", true, None::<&str>)?;
        let quit = MenuItem::with_id(app, "quit", "Quit", true, None::<&str>)?;
        let menu = Menu::with_items(app, &[&open, &quit])?;

        // `default_window_icon` borrows from the app; own the pixels so the
        // tray can keep both variants around.
        let plain = app
            .default_window_icon()
            .map(|icon| Image::new_owned(icon.rgba().to_vec(), icon.width(), icon.height()))
            .unwrap_or_else(|| Image::new_owned(vec![0; 4], 1, 1));
        let marked = with_dot(&plain);

        let tray = TrayIconBuilder::new()
            .icon(plain.clone())
            .menu(&menu)
            .show_menu_on_left_click(false)
            .tooltip("Aster")
            .on_menu_event(|app, event| match event.id.as_ref() {
                "open" => focus_window(app),
                "quit" => app.exit(0),
                _ => (),
            })
            .on_tray_icon_event(|tray, event| {
                // Left click toggles the window; the menu is on right click.
                if let TrayIconEvent::Click {
                    button: MouseButton::Left,
                    button_state: MouseButtonState::Up,
                    ..
                } = event
                {
                    toggle_window(tray.app_handle());
                }
            })
            .build(app)?;

        Ok(Self {
            focused: AtomicBool::new(true),
            unread: AtomicU32::new(0),
            tray,
            plain,
            marked,
        })
    }

    /// The UI reports whether the user is actually looking at the window.
    /// Focusing it clears the unread count.
    pub fn set_focused(&self, app: &AppHandle, focused: bool) {
        self.focused.store(focused, Ordering::Relaxed);
        if focused && self.unread.swap(0, Ordering::Relaxed) != 0 {
            self.publish(app, 0);
        }
    }

    /// A message was stored. Incoming ones seen by nobody raise the count and
    /// pop a notification.
    pub fn message(&self, app: &AppHandle, message: &Message) {
        if message.direction != Direction::In || self.focused.load(Ordering::Relaxed) {
            return;
        }
        let count = self.unread.fetch_add(1, Ordering::Relaxed) + 1;
        self.publish(app, count);
        notify(app, "New message", &shorten(&message.body));
    }

    /// An incoming call always notifies, focused or not: it is the one thing
    /// that cannot wait.
    pub fn call(&self, app: &AppHandle, state: &CallState) {
        if matches!(state, CallState::Ringing) {
            notify(app, "Incoming call", "Someone is calling you");
        }
    }

    /// Keeps the tray tooltip in step with the connection.
    pub fn set_tooltip(&self, text: &str) {
        let _ = self.tray.set_tooltip(Some(text));
    }

    fn publish(&self, app: &AppHandle, count: u32) {
        let icon = if count > 0 { &self.marked } else { &self.plain };
        let _ = self.tray.set_icon(Some(icon.clone()));
        if let Err(err) = app.emit("unread", Unread { count }) {
            crate::log_line!("failed to emit unread: {err}");
        }
    }
}

/// Hides the window instead of closing it, so the core keeps running.
pub fn hide_on_close(window: &tauri::Window, event: &WindowEvent) {
    if let WindowEvent::CloseRequested { api, .. } = event {
        api.prevent_close();
        let _ = window.hide();
    }
}

/// Brings the main window up, wherever it was.
pub fn focus_window(app: &AppHandle) {
    if let Some(window) = app.get_webview_window("main") {
        let _ = window.show();
        let _ = window.unminimize();
        let _ = window.set_focus();
    }
}

fn toggle_window(app: &AppHandle) {
    let Some(window) = app.get_webview_window("main") else {
        return;
    };
    match window.is_visible() {
        Ok(true) => {
            let _ = window.hide();
        }
        _ => focus_window(app),
    }
}

fn notify(app: &AppHandle, title: &str, body: &str) {
    if let Err(err) = app.notification().builder().title(title).body(body).show() {
        crate::log_line!("notification not shown: {err}");
    }
}

/// First `NOTIFICATION_BODY` characters, with an ellipsis when cut.
fn shorten(body: &str) -> String {
    let mut short: String = body.chars().take(NOTIFICATION_BODY).collect();
    if body.chars().nth(NOTIFICATION_BODY).is_some() {
        short.push('…');
    }
    short
}

/// The tray icon with a dot in the corner, drawn over the app icon so the two
/// states differ without shipping a second image.
fn with_dot(icon: &Image<'_>) -> Image<'static> {
    let (width, height) = (icon.width(), icon.height());
    let mut rgba = icon.rgba().to_vec();
    let radius = (width.min(height) as f32 * 0.22).max(2.0);
    let (cx, cy) = (width as f32 - radius - 1.0, radius + 1.0);
    for y in 0..height {
        for x in 0..width {
            let (dx, dy) = (x as f32 - cx, y as f32 - cy);
            if dx * dx + dy * dy > radius * radius {
                continue;
            }
            let i = ((y * width + x) * 4) as usize;
            // --accent from spec 0008, fully opaque.
            rgba[i..i + 4].copy_from_slice(&[0xa3, 0x92, 0xdd, 0xff]);
        }
    }
    Image::new_owned(rgba, width, height)
}

#[cfg(test)]
mod tests {
    use super::{shorten, with_dot, NOTIFICATION_BODY};
    use tauri::image::Image;

    #[test]
    fn short_bodies_are_left_alone() {
        assert_eq!(shorten("привет"), "привет");
    }

    #[test]
    fn long_bodies_are_cut_on_a_character_boundary() {
        let body = "я".repeat(NOTIFICATION_BODY + 20);
        let short = shorten(&body);
        assert_eq!(short.chars().count(), NOTIFICATION_BODY + 1);
        assert!(short.ends_with('…'));
    }

    #[test]
    fn the_marked_icon_differs_from_the_plain_one() {
        let plain = Image::new_owned(vec![0; 32 * 32 * 4], 32, 32);
        let marked = with_dot(&plain);
        assert_eq!((marked.width(), marked.height()), (32, 32));
        assert_ne!(marked.rgba(), plain.rgba());
        // The dot sits in the top right corner, not over the whole icon.
        let bottom_left = (31 * 32 * 4) as usize;
        assert_eq!(&marked.rgba()[bottom_left..bottom_left + 4], &[0, 0, 0, 0]);
    }
}
