//! Server address and token form (spec 0009 `save_settings`).
//!
//! The token is write-only: the core never hands it back, so an existing one
//! shows up only as "задан".

use leptos::prelude::*;

use crate::bridge::{self, CommandError};

use super::icons::Icon;
use super::text_field::TextField;

#[derive(Clone, Debug, serde::Deserialize)]
pub struct SettingsView {
    pub server_url: Option<String>,
    pub has_token: bool,
}

#[derive(serde::Serialize)]
struct SaveArgs {
    server_url: String,
    token: String,
}

#[component]
pub fn SettingsPanel(
    /// Current settings, refreshed by the caller after a save.
    settings: Signal<Option<SettingsView>>,
    on_saved: Callback<()>,
) -> impl IntoView {
    let url = RwSignal::new(String::new());
    let token = RwSignal::new(String::new());
    let error = RwSignal::new(None::<String>);
    let saving = RwSignal::new(false);

    // Prefill the address once it arrives; the token stays empty by design.
    Effect::new(move |_| {
        if let Some(current) = settings.get().and_then(|s| s.server_url) {
            if url.get_untracked().is_empty() {
                url.set(current);
            }
        }
    });

    let save = move || {
        if saving.get() {
            return;
        }
        let (server_url, secret) = (url.get().trim().to_string(), token.get());
        if server_url.is_empty() || secret.is_empty() {
            error.set(Some("Нужны и адрес сервера, и токен".into()));
            return;
        }
        saving.set(true);
        error.set(None);
        leptos::task::spawn_local(async move {
            let result: Result<(), CommandError> = bridge::invoke(
                "save_settings",
                &SaveArgs {
                    server_url,
                    token: secret,
                },
            )
            .await;
            saving.set(false);
            match result {
                Ok(()) => {
                    token.set(String::new());
                    on_saved.run(());
                }
                Err(err) => error.set(Some(err.message)),
            }
        });
    };

    view! {
        <div class="settings">
            <h2 class="t-display-s">"Подключение"</h2>
            <p class="t-caption settings__hint">
                "Адрес сервера сигнализации и общий токен. Токен хранится в "
                <code>"settings.json"</code>" с правами 0600 и обратно не показывается."
            </p>
            <label class="t-ui-strong settings__label">"Адрес сервера"</label>
            <TextField
                value=url
                placeholder="wss://aster.example.com/ws"
                icon=Icon::Compass
                aria_label="Адрес сервера"
            />
            <label class="t-ui-strong settings__label">
                {move || {
                    match settings.get() {
                        Some(s) if s.has_token => "Токен (задан — введите, чтобы заменить)",
                        _ => "Токен",
                    }
                }}
            </label>
            <TextField value=token placeholder="общий секрет" aria_label="Токен" />
            {move || {
                error
                    .get()
                    .map(|message| view! { <p class="t-caption settings__error">{message}</p> })
            }}
            <button
                class="settings__save t-ui-strong"
                type="button"
                disabled=move || saving.get()
                on:click=move |_| save()
            >
                {move || if saving.get() { "Сохраняем…" } else { "Сохранить и подключиться" }}
            </button>
        </div>
    }
}
