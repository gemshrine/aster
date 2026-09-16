//! Call screen — tiles, controls and the incoming call card
//! (spec 0008, artboard "Active voice call"; contract in spec 0011).

use leptos::prelude::*;

use crate::call::{duration, CallState};

use super::avatar::Avatar;
use super::icon_button::{IconButton, IconButtonKind};
use super::icons::{Icon, IconView};

/// Ticks once a second so the call timer moves.
fn now_signal() -> Signal<i64> {
    let now = RwSignal::new(js_sys::Date::now() as i64);
    let handle = set_interval_with_handle(
        move || now.set(js_sys::Date::now() as i64),
        std::time::Duration::from_secs(1),
    );
    if let Ok(handle) = handle {
        on_cleanup(move || handle.clear());
    }
    now.into()
}

#[component]
fn Tile(
    #[prop(into)] name: String,
    #[prop(into)] speaking: Signal<bool>,
    #[prop(into)] muted: Signal<bool>,
    #[prop(into)] caption: Signal<Option<String>>,
) -> impl IntoView {
    let label = name.clone();
    view! {
        <div class="call-tile" class:call-tile--speaking=move || speaking.get()>
            // No presence dot on a tile: who is in the call is not in
            // question, and the speaking ring is on the tile itself.
            <Avatar name=name size=88.0 ring_bg="var(--bg-panel)" />
            <div class="call-tile__name">
                <span class="t-body-strong">{label}</span>
                <Show when=move || muted.get()>
                    <span class="call-tile__muted" title="Микрофон выключен">
                        <IconView icon=Icon::Mic size=14.0 />
                    </span>
                </Show>
            </div>
            {move || {
                caption
                    .get()
                    .map(|caption| view! { <span class="t-caption">{caption}</span> })
            }}
        </div>
    }
}

#[component]
fn Controls(
    #[prop(into)] muted: Signal<bool>,
    on_mute: Callback<()>,
    on_hang_up: Callback<()>,
) -> impl IntoView {
    view! {
        <div class="call-controls">
            <button
                class="call-control"
                class:call-control--off=move || muted.get()
                type="button"
                aria-label=move || {
                    if muted.get() { "Включить микрофон" } else { "Выключить микрофон" }
                }
                on:click=move |_| on_mute.run(())
            >
                <IconView icon=Icon::Mic size=19.0 />
            </button>
            <button
                class="call-control call-control--leave t-ui-strong"
                type="button"
                on:click=move |_| on_hang_up.run(())
            >
                <IconView icon=Icon::Phone size=19.0 />
                "Завершить"
            </button>
        </div>
    }
}

#[component]
pub fn CallPane(
    state: Signal<CallState>,
    #[prop(into)] peer_name: String,
    #[prop(into)] self_name: String,
    #[prop(into)] muted: Signal<bool>,
    #[prop(into)] local_speaking: Signal<bool>,
    #[prop(into)] remote_speaking: Signal<bool>,
    on_mute: Callback<()>,
    on_accept: Callback<()>,
    on_decline: Callback<()>,
    on_hang_up: Callback<()>,
) -> impl IntoView {
    let now = now_signal();
    let ringing = Signal::derive(move || state.get() == CallState::Ringing);
    let timer = Signal::derive(move || match state.get() {
        CallState::Connected { since } => Some(duration(since, now.get())),
        _ => None,
    });
    let waiting = Signal::derive(move || state.get().waiting_label().map(str::to_string));

    view! {
        <div class="call">
            <div class="call__grid">
                <Tile
                    name=peer_name
                    speaking=remote_speaking
                    muted=Signal::derive(|| false)
                    caption=waiting
                />
                <Tile
                    name=self_name
                    speaking=local_speaking
                    muted=muted
                    caption=Signal::derive(|| None::<String>)
                />
            </div>
            {move || {
                timer.get().map(|timer| view! { <p class="t-display-s call__timer">{timer}</p> })
            }}
            <Show
                when=move || ringing.get()
                fallback=move || {
                    view! { <Controls muted=muted on_mute=on_mute on_hang_up=on_hang_up /> }
                }
            >
                <div class="call-controls">
                    <button
                        class="call-control call-control--accept t-ui-strong"
                        type="button"
                        on:click=move |_| on_accept.run(())
                    >
                        <IconView icon=Icon::Phone size=19.0 />
                        "Принять"
                    </button>
                    <button
                        class="call-control call-control--leave t-ui-strong"
                        type="button"
                        on:click=move |_| on_decline.run(())
                    >
                        <IconView icon=Icon::Close size=19.0 />
                        "Отклонить"
                    </button>
                </div>
            </Show>
        </div>
    }
}

/// Outcome of the call just finished — the core is already back in `idle`,
/// so the UI is what remembers it.
#[component]
pub fn CallOutcome(#[prop(into)] message: String, on_dismiss: Callback<()>) -> impl IntoView {
    view! {
        <div class="call-outcome">
            <span class="t-ui">{message}</span>
            <IconButton
                icon=Icon::Close
                label="Скрыть"
                kind=IconButtonKind::Sm
                on_click=on_dismiss
            />
        </div>
    }
}
