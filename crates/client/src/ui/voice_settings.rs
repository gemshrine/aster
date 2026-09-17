//! Voice settings screen — devices, processing, input mode (spec 0013).
//!
//! Every change is sent to the core immediately: `set_voice_settings` applies
//! during a call too, so there is nothing to "save".

use leptos::prelude::*;

use crate::bridge::{self, CommandError};
use crate::voice::{
    hyprland_bind, level_percent, AudioDevice, AudioDevices, GlobalSource, InputMode,
    NoiseSuppression, PushToTalkStatus, VoiceSettings,
};

use super::icons::{Icon, IconView};

#[derive(serde::Serialize)]
struct SettingsArgs {
    settings: VoiceSettings,
}

#[component]
fn Row(#[prop(into)] label: String, children: Children) -> impl IntoView {
    view! {
        <label class="vset__row">
            <span class="t-ui-strong vset__label">{label}</span>
            {children()}
        </label>
    }
}

#[component]
fn DeviceSelect(
    #[prop(into)] label: String,
    devices: Signal<Vec<AudioDevice>>,
    selected: Signal<Option<String>>,
    on_pick: Callback<Option<String>>,
) -> impl IntoView {
    view! {
        <Row label=label>
            <select
                class="vset__control t-ui"
                on:change:target=move |ev| {
                    let value = ev.target().value();
                    on_pick.run((!value.is_empty()).then_some(value));
                }
            >
                <option value="" selected=move || selected.get().is_none()>
                    "System default"
                </option>
                {move || {
                    let selected = selected.get();
                    devices
                        .get()
                        .into_iter()
                        .map(|device| {
                            let picked = selected.as_deref() == Some(device.id.as_str());
                            let name = if device.is_default {
                                format!("{} (default)", device.name)
                            } else {
                                device.name.clone()
                            };
                            view! {
                                <option value=device.id selected=picked>
                                    {name}
                                </option>
                            }
                        })
                        .collect_view()
                }}
            </select>
        </Row>
    }
}

#[component]
fn Toggle(
    #[prop(into)] label: String,
    checked: Signal<bool>,
    on_toggle: Callback<bool>,
) -> impl IntoView {
    view! {
        <Row label=label>
            <input
                class="vset__checkbox"
                type="checkbox"
                prop:checked=move || checked.get()
                on:change:target=move |ev| on_toggle.run(ev.target().checked())
            />
        </Row>
    }
}

/// The key the window listens for while it has focus. Stored per install, not
/// in the core: spec 0013 keeps it on the UI side.
pub fn stored_key() -> Option<String> {
    let storage = window().local_storage().ok().flatten()?;
    storage.get_item("ptt_key").ok().flatten()
}

fn store_key(code: Option<&str>) {
    let Some(storage) = window().local_storage().ok().flatten() else {
        return;
    };
    let _ = match code {
        Some(code) => storage.set_item("ptt_key", code),
        None => storage.remove_item("ptt_key"),
    };
}

#[component]
pub fn VoiceSettingsPanel(
    /// Level of the microphone after processing, from `input-level`.
    input_level: Signal<f32>,
    /// True while the gate is open and audio is actually going out.
    transmitting: Signal<bool>,
    /// Key captured for the in-window push-to-talk source.
    ptt_key: RwSignal<Option<String>>,
) -> impl IntoView {
    let settings = RwSignal::new(VoiceSettings::default());
    let devices = RwSignal::new(AudioDevices::default());
    let status = RwSignal::new(PushToTalkStatus::default());
    let error = RwSignal::new(None::<String>);
    let capturing = RwSignal::new(false);

    leptos::task::spawn_local(async move {
        if let Ok(loaded) = bridge::invoke::<_, VoiceSettings>("get_voice_settings", &()).await {
            settings.set(loaded);
        }
        if let Ok(found) = bridge::invoke::<_, AudioDevices>("list_audio_devices", &()).await {
            devices.set(found);
        }
        if let Ok(found) = bridge::invoke::<_, PushToTalkStatus>("push_to_talk_status", &()).await {
            status.set(found);
        }
    });

    // The meter only runs while this screen is open.
    leptos::task::spawn_local(async move {
        let _: Result<(), CommandError> = bridge::invoke("set_input_monitor", &Monitor(true)).await;
    });
    on_cleanup(|| {
        leptos::task::spawn_local(async move {
            let _: Result<(), CommandError> =
                bridge::invoke("set_input_monitor", &Monitor(false)).await;
        });
    });

    let apply = Callback::new(move |next: VoiceSettings| {
        settings.set(next.clone());
        leptos::task::spawn_local(async move {
            match bridge::invoke::<_, VoiceSettings>(
                "set_voice_settings",
                &SettingsArgs { settings: next },
            )
            .await
            {
                // The core normalises values, so take back what it stored.
                Ok(stored) => {
                    settings.set(stored);
                    error.set(None);
                    if let Ok(found) =
                        bridge::invoke::<_, PushToTalkStatus>("push_to_talk_status", &()).await
                    {
                        status.set(found);
                    }
                }
                Err(err) => error.set(Some(err.message)),
            }
        });
    });

    let edit = move |change: fn(&mut VoiceSettings)| {
        let mut next = settings.get_untracked();
        change(&mut next);
        apply.run(next);
    };

    let push_to_talk = Signal::derive(move || settings.get().input_mode == InputMode::PushToTalk);

    view! {
        <div class="settings vset">
            <h2 class="t-display-s">"Voice"</h2>
            <p class="t-caption settings__hint">
                "Changes apply at once, during a call as well."
            </p>

            <DeviceSelect
                label="Input device"
                devices=Signal::derive(move || devices.get().inputs)
                selected=Signal::derive(move || settings.get().input_device)
                on_pick=Callback::new(move |id: Option<String>| {
                    let mut next = settings.get_untracked();
                    next.input_device = id;
                    apply.run(next);
                })
            />
            <DeviceSelect
                label="Output device"
                devices=Signal::derive(move || devices.get().outputs)
                selected=Signal::derive(move || settings.get().output_device)
                on_pick=Callback::new(move |id: Option<String>| {
                    let mut next = settings.get_untracked();
                    next.output_device = id;
                    apply.run(next);
                })
            />

            <Toggle
                label="Echo cancellation"
                checked=Signal::derive(move || settings.get().echo_cancellation)
                on_toggle=Callback::new(move |_| {
                    edit(|s| s.echo_cancellation = !s.echo_cancellation)
                })
            />
            <Toggle
                label="Automatic gain"
                checked=Signal::derive(move || settings.get().auto_gain)
                on_toggle=Callback::new(move |_| edit(|s| s.auto_gain = !s.auto_gain))
            />

            <Row label="Noise suppression">
                <select
                    class="vset__control t-ui"
                    on:change:target=move |ev| {
                        let picked = NoiseSuppression::from_id(&ev.target().value());
                        let mut next = settings.get_untracked();
                        next.noise_suppression = picked;
                        apply.run(next);
                    }
                >
                    {move || {
                        let current = settings.get().noise_suppression;
                        NoiseSuppression::ALL
                            .into_iter()
                            .map(|value| {
                                view! {
                                    <option value=value.id() selected=value == current>
                                        {value.label()}
                                    </option>
                                }
                            })
                            .collect_view()
                    }}
                </select>
            </Row>

            <Row label="Input mode">
                <select
                    class="vset__control t-ui"
                    on:change:target=move |ev| {
                        let mode = if ev.target().value() == "push_to_talk" {
                            InputMode::PushToTalk
                        } else {
                            InputMode::VoiceActivity
                        };
                        let mut next = settings.get_untracked();
                        next.input_mode = mode;
                        apply.run(next);
                    }
                >
                    <option value="voice_activity" selected=move || !push_to_talk.get()>
                        "Voice activity"
                    </option>
                    <option value="push_to_talk" selected=move || push_to_talk.get()>
                        "Push to talk"
                    </option>
                </select>
            </Row>

            <div class="vset__meter-row">
                <span class="t-ui-strong vset__label">"Microphone"</span>
                <div class="vset__meter">
                    <div
                        class="vset__meter-fill"
                        class:vset__meter-fill--live=move || transmitting.get()
                        style=move || format!("width:{:.1}%", level_percent(input_level.get()))
                    ></div>
                    <Show when=move || !push_to_talk.get()>
                        <div
                            class="vset__meter-mark"
                            style=move || {
                                format!(
                                    "left:{:.1}%",
                                    level_percent(settings.get().vad_threshold_dbfs as f32),
                                )
                            }
                        ></div>
                    </Show>
                </div>
            </div>

            <Show when=move || !push_to_talk.get()>
                <Row label="Voice activity threshold">
                    <input
                        class="vset__control"
                        type="range"
                        min="-70"
                        max="-20"
                        prop:value=move || settings.get().vad_threshold_dbfs.to_string()
                        on:input:target=move |ev| {
                            let value = ev.target().value().parse().unwrap_or(-45);
                            let mut next = settings.get_untracked();
                            next.vad_threshold_dbfs = value;
                            apply.run(next);
                        }
                    />
                </Row>
            </Show>

            <Show when=move || push_to_talk.get()>
                <Row label="Release delay, ms">
                    <input
                        class="vset__control"
                        type="number"
                        min="0"
                        max="1000"
                        step="50"
                        prop:value=move || settings.get().ptt_release_delay_ms.to_string()
                        on:change:target=move |ev| {
                            let value = ev.target().value().parse().unwrap_or(200);
                            let mut next = settings.get_untracked();
                            next.ptt_release_delay_ms = value;
                            apply.run(next);
                        }
                    />
                </Row>

                <Row label="Key (while the window is focused)">
                    <button
                        class="vset__control vset__key t-ui"
                        type="button"
                        on:click=move |_| capturing.set(true)
                        on:keydown=move |ev| {
                            if !capturing.get_untracked() {
                                return;
                            }
                            ev.prevent_default();
                            capturing.set(false);
                            let code = ev.code();
                            if code == "Escape" {
                                store_key(None);
                                ptt_key.set(None);
                            } else {
                                store_key(Some(&code));
                                ptt_key.set(Some(code));
                            }
                        }
                    >
                        {move || {
                            if capturing.get() {
                                "Press a key — Esc clears".to_string()
                            } else {
                                ptt_key.get().unwrap_or_else(|| "Not set".into())
                            }
                        }}
                    </button>
                </Row>

                <p class="t-caption vset__hint">
                    {move || match status.get().global {
                        GlobalSource::Portal => {
                            "A global key comes from the desktop portal. In Hyprland add:"
                                .to_string()
                        }
                        GlobalSource::Shortcut => {
                            "A global key is registered by the app.".to_string()
                        }
                        GlobalSource::Unavailable => {
                            let reason = status
                                .get()
                                .reason
                                .unwrap_or_else(|| "no global shortcut source".into());
                            format!(
                                "No global key ({reason}) — push to talk works only while \
                                 the window is focused."
                            )
                        }
                    }}
                </p>
                <Show when=move || status.get().global == GlobalSource::Portal>
                    <pre class="t-caption vset__code selectable">
                        {hyprland_bind(Some("dev.gemshrine.aster"))}
                    </pre>
                </Show>
            </Show>

            {move || {
                error
                    .get()
                    .map(|message| view! { <p class="t-caption settings__error">{message}</p> })
            }}

            <p class="t-caption vset__status">
                <IconView icon=Icon::Mic size=14.0 />
                {move || if transmitting.get() { "Transmitting" } else { "Silent" }}
            </p>
        </div>
    }
}

#[derive(serde::Serialize)]
struct Monitor(#[serde(rename = "enabled")] bool);
