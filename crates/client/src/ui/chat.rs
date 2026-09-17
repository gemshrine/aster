//! Message list and composer (spec 0008, artboard "Direct message").
//!
//! Data comes in as plain values; #6 will feed them from the local history.

use leptos::prelude::*;

use crate::chat::{
    clock, day, follows_bottom, group, restored_scroll_top, wants_older, Author, Group, Message,
    Status,
};

use super::avatar::Avatar;
use super::icon_button::{IconButton, IconButtonKind};
use super::icons::Icon;

#[component]
fn StatusLine(status: Status) -> impl IntoView {
    let rejected = status.is_rejected();
    view! {
        <span
            class="t-caption msg-group__status"
            class:msg-group__status--rejected=rejected
        >
            {status.label().to_string()}
        </span>
    }
}

#[component]
fn Bubble(message: Message, first: bool, last: bool) -> impl IntoView {
    let mine = message.author == Author::Me;
    view! {
        <div
            class="bubble"
            class:bubble--out=mine
            class:bubble--first=first
            class:bubble--last=last
        >
            <span class="t-body selectable">{message.body}</span>
        </div>
    }
}

#[component]
fn MessageGroup(group: Group, peer_name: String, self_name: String) -> impl IntoView {
    let mine = group.author == Author::Me;
    let name = if mine { self_name } else { peer_name };
    let time = clock(group.sent_at());
    let status = group.status().cloned();
    let last = group.messages.len() - 1;
    view! {
        <div class="msg-group" class:msg-group--out=mine>
            <Avatar name=name.clone() size=38.0 ring_bg="var(--bg)" />
            <div class="msg-group__body">
                <div class="msg-group__head">
                    <span class="t-body-strong">{name}</span>
                    <span class="t-caption">{time}</span>
                </div>
                <div class="msg-group__bubbles">
                    {group
                        .messages
                        .into_iter()
                        .enumerate()
                        .map(|(i, message)| {
                            view! { <Bubble message=message first=i == 0 last=i == last /> }
                        })
                        .collect_view()}
                </div>
                {status.map(|status| view! { <StatusLine status=status /> })}
            </div>
        </div>
    }
}

#[component]
pub fn MessageList(
    messages: Signal<Vec<Message>>,
    #[prop(into)] peer_name: String,
    #[prop(into)] self_name: String,
    /// Asks for the page before the oldest message currently held.
    #[prop(optional)]
    on_load_older: Option<Callback<()>>,
    #[prop(into, optional)] loading_older: Signal<bool>,
) -> impl IntoView {
    let groups = Signal::derive(move || group(&messages.get()));
    let list: NodeRef<leptos::html::Div> = NodeRef::new();
    // Where the viewport sat before older messages were prepended, so the
    // same message can be put back under the reader's eyes.
    let anchor = RwSignal::new(None::<(f64, f64)>);
    // Start pinned to the bottom: a chat that opens on last week is useless.
    let at_bottom = RwSignal::new(true);

    Effect::new(move |_| {
        groups.track();
        let Some(list) = list.get() else { return };
        let height = f64::from(list.scroll_height());
        match anchor.get_untracked() {
            Some((previous_top, previous_height)) => {
                anchor.set(None);
                list.set_scroll_top(
                    restored_scroll_top(previous_top, previous_height, height) as i32
                );
            }
            None if at_bottom.get_untracked() => list.set_scroll_top(height as i32),
            None => (),
        }
    });

    let on_scroll = move |_| {
        let Some(list) = list.get_untracked() else {
            return;
        };
        let (top, client, height) = (
            f64::from(list.scroll_top()),
            f64::from(list.client_height()),
            f64::from(list.scroll_height()),
        );
        at_bottom.set(follows_bottom(top, client, height));
        if let Some(on_load_older) = on_load_older {
            if wants_older(top)
                && !loading_older.get_untracked()
                && anchor.get_untracked().is_none()
            {
                anchor.set(Some((top, height)));
                on_load_older.run(());
            }
        }
    };

    view! {
        <div class="msg-list" node_ref=list on:scroll=on_scroll>
            <Show when=move || loading_older.get()>
                <p class="t-caption msg-list__loading">"Loading earlier messages…"</p>
            </Show>
            <Show when=move || groups.get().is_empty() && !loading_older.get()>
                <p class="t-ui msg-list__empty">"No messages yet."</p>
            </Show>
            {move || {
                let peer_name = peer_name.clone();
                let self_name = self_name.clone();
                let mut current_day = String::new();
                groups
                    .get()
                    .into_iter()
                    .map(|group| {
                        let this_day = day(group.sent_at());
                        let divider = (this_day != current_day).then(|| this_day.clone());
                        current_day = this_day;
                        view! {
                            {divider
                                .map(|label| {
                                    view! { <p class="t-caption day-divider">{label}</p> }
                                })}
                            <MessageGroup
                                group=group
                                peer_name=peer_name.clone()
                                self_name=self_name.clone()
                            />
                        }
                    })
                    .collect_view()
            }}
        </div>
    }
}

#[component]
pub fn Composer(
    /// Disabled while there is no connection — the core refuses sends anyway.
    #[prop(into)]
    disabled: Signal<bool>,
    #[prop(into)] placeholder: String,
    on_send: Callback<String>,
) -> impl IntoView {
    let draft = RwSignal::new(String::new());
    let can_send = Signal::derive(move || !disabled.get() && !draft.get().trim().is_empty());
    let send = move || {
        if !can_send.get() {
            return;
        }
        on_send.run(draft.get().trim().to_string());
        draft.set(String::new());
    };

    view! {
        <div class="composer" class:composer--disabled=move || disabled.get()>
            <IconButton icon=Icon::Plus label="Attachment" kind=IconButtonKind::Sm disabled=disabled />
            <input
                class="composer__input t-body selectable"
                type="text"
                placeholder=placeholder
                prop:value=move || draft.get()
                disabled=move || disabled.get()
                on:input:target=move |ev| draft.set(ev.target().value())
                on:keydown=move |ev| {
                    if ev.key() == "Enter" && !ev.shift_key() {
                        ev.prevent_default();
                        send();
                    }
                }
            />
            <IconButton icon=Icon::Emoji label="Emoji" kind=IconButtonKind::Sm disabled=disabled />
            <button
                class="composer__send"
                type="button"
                aria-label="Send"
                title="Send"
                disabled=move || !can_send.get()
                on:click=move |_| send()
            >
                <super::icons::IconView icon=Icon::Send size=16.0 />
            </button>
        </div>
    }
}
