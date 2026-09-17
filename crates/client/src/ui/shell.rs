//! Window shell — title strip, sidebar and the main area (spec 0008).
//!
//! The window frame itself is the WM's (`decorations: true`), so this is a
//! strip inside the window, not a drag handle. Connection state arrives as a
//! signal; #5 feeds it from the core's `connection-state` event, and until
//! then the gallery drives it by hand.

use leptos::prelude::*;

use crate::connection::ConnectionState;

use super::avatar::{Avatar, Presence};
use super::icon_button::{IconButton, IconButtonKind};
use super::icons::Icon;

#[component]
fn ConnectionIndicator(state: Signal<ConnectionState>) -> impl IntoView {
    view! {
        <div
            class="conn"
            class:conn--attention=move || state.get().needs_attention()
            role="status"
        >
            <span class="conn__dot" style=move || format!("background:{}", state.get().tone())></span>
            <span class="t-caption conn__label">{move || state.get().label()}</span>
        </div>
    }
}

#[component]
fn PeerCard(
    peer_name: String,
    state: Signal<ConnectionState>,
    /// True while a call is already up — no second call to start.
    in_call: Signal<bool>,
    on_call: Callback<()>,
) -> impl IntoView {
    let name = peer_name.clone();
    let online = Signal::derive(move || state.get().peer_online());
    let can_call = Signal::derive(move || online.get() && !in_call.get());
    view! {
        <div class="peer">
            <div class="peer__row">
                <Avatar
                    name=peer_name
                    size=38.0
                    presence=Signal::derive(move || {
                        if online.get() { Presence::Online } else { Presence::Offline }
                    })
                    ring_bg="var(--bg-panel)"
                />
                <div class="peer__text">
                    <span class="t-body-strong">{name}</span>
                    <span class="t-caption">
                        {move || if online.get() { "Online" } else { "Offline" }}
                    </span>
                </div>
            </div>
            <button
                class="peer__call t-ui-strong"
                type="button"
                disabled=move || !can_call.get()
                on:click=move |_| on_call.run(())
            >
                {move || if in_call.get() { "In a call" } else { "Call" }}
            </button>
        </div>
    }
}

#[component]
fn SelfPanel(self_name: String) -> impl IntoView {
    let muted = RwSignal::new(false);
    let name = self_name.clone();
    view! {
        <div class="self-panel">
            <Avatar name=self_name size=30.0 ring_bg="var(--bg-elevated)" />
            <span class="t-ui-strong self-panel__name">{name}</span>
            <IconButton
                icon=Icon::Mic
                label="Microphone"
                kind=Signal::derive(move || {
                    if muted.get() { IconButtonKind::Danger } else { IconButtonKind::Toggled }
                })
                on_click=Callback::new(move |()| muted.update(|m| *m = !*m))
            />
            <IconButton icon=Icon::Headphones label="Sound" kind=IconButtonKind::Sm />
        </div>
    }
}

#[component]
pub fn Shell(
    state: Signal<ConnectionState>,
    #[prop(into)] peer_name: String,
    #[prop(into)] self_name: String,
    /// Opens the connection settings; the title strip is the only way in.
    on_settings: Callback<()>,
    #[prop(into)] in_call: Signal<bool>,
    on_call: Callback<()>,
    children: Children,
) -> impl IntoView {
    view! {
        <div class="shell">
            <header class="shell__title">
                <span class="t-ui-strong shell__brand">"Aster"</span>
                <div class="shell__title-right">
                    <ConnectionIndicator state=state />
                    <IconButton
                        icon=Icon::Edit
                        label="Connection settings"
                        kind=IconButtonKind::Sm
                        on_click=on_settings
                    />
                </div>
            </header>
            <div class="shell__body">
                <aside class="shell__sidebar">
                    <PeerCard
                        peer_name=peer_name
                        state=state
                        in_call=in_call
                        on_call=on_call
                    />
                    <div class="shell__spacer"></div>
                    <SelfPanel self_name=self_name />
                </aside>
                <main class="shell__main">{children()}</main>
            </div>
        </div>
    }
}
