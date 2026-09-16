//! Wiring between the core (spec 0009) and the UI (spec 0008).

use leptos::prelude::*;

use crate::bridge::{self, CommandError};
use crate::chat::{Author, Message, Status};
use crate::connection::ConnectionState;
use crate::ui::chat::{Composer, MessageList};
use crate::ui::settings::{SettingsPanel, SettingsView};
use crate::ui::shell::Shell;

const PEER_NAME: &str = "Кент";
const SELF_NAME: &str = "Ты";

#[derive(serde::Deserialize)]
struct PeerStatus {
    online: bool,
}

#[derive(serde::Deserialize)]
struct IncomingMessage {
    id: String,
    body: String,
    sent_at: i64,
}

#[derive(serde::Deserialize)]
struct ChatAck {
    id: String,
    status: AckStatus,
}

#[derive(Clone, Copy, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
enum AckStatus {
    Delivered,
    Queued,
}

#[derive(serde::Deserialize)]
struct ChatRejected {
    id: Option<String>,
    message: String,
}

#[derive(serde::Serialize)]
struct SendArgs {
    id: String,
    body: String,
    sent_at: i64,
}

#[component]
pub fn App() -> impl IntoView {
    let state = RwSignal::new(ConnectionState::NotConfigured);
    let messages = RwSignal::new(Vec::<Message>::new());
    let settings = RwSignal::new(None::<SettingsView>);
    let show_settings = RwSignal::new(false);

    let refresh_settings = Callback::new(move |()| {
        leptos::task::spawn_local(async move {
            if let Ok(view) = bridge::invoke::<_, SettingsView>("get_settings", &()).await {
                settings.set(Some(view));
            }
        });
    });

    // First paint gets the state the core is already in; everything after
    // that arrives as events.
    leptos::task::spawn_local(async move {
        if let Ok(current) = bridge::invoke::<_, ConnectionState>("connection_state", &()).await {
            state.set(current);
        }
    });
    refresh_settings.run(());

    bridge::listen::<ConnectionState>("connection-state", move |next| {
        show_settings.set(matches!(next, ConnectionState::NotConfigured));
        state.set(next);
    });

    bridge::listen::<PeerStatus>("peer-status", move |status| {
        // `peer-status` is authoritative for presence once connected.
        state.update(|state| {
            if let ConnectionState::Connected { peer_online, .. } = state {
                *peer_online = status.online;
            }
        });
    });

    bridge::listen::<IncomingMessage>("chat-message", move |incoming| {
        messages.update(|messages| {
            if messages.iter().any(|m| m.id == incoming.id) {
                return; // a redelivered message from the offline queue
            }
            messages.push(Message {
                id: incoming.id,
                author: Author::Peer,
                body: incoming.body,
                sent_at: incoming.sent_at,
                status: None,
            });
        });
    });

    bridge::listen::<ChatAck>("chat-ack", move |ack| {
        let status = match ack.status {
            AckStatus::Delivered => Status::Delivered,
            AckStatus::Queued => Status::Queued,
        };
        set_status(messages, &ack.id, status);
    });

    bridge::listen::<ChatRejected>("chat-rejected", move |rejected| {
        // A rejection with no id is server trouble at large, which the
        // connection indicator already reports.
        if let Some(id) = rejected.id {
            set_status(
                messages,
                &id,
                Status::Rejected {
                    message: rejected.message,
                },
            );
        }
    });

    let on_send = Callback::new(move |body: String| {
        let id = bridge::uuid();
        let sent_at = js_sys::Date::now() as i64;
        messages.update(|messages| {
            messages.push(Message {
                id: id.clone(),
                author: Author::Me,
                body: body.clone(),
                sent_at,
                status: Some(Status::Sending),
            });
        });
        leptos::task::spawn_local(async move {
            let args = SendArgs {
                id: id.clone(),
                body,
                sent_at,
            };
            let result: Result<(), CommandError> = bridge::invoke("send_chat", &args).await;
            match result {
                // The core took it; delivery is confirmed later by chat-ack.
                Ok(()) => set_status(messages, &id, Status::Sent),
                Err(err) => set_status(
                    messages,
                    &id,
                    Status::Rejected {
                        message: err.message,
                    },
                ),
            }
        });
    });

    let offline = Signal::derive(move || !state.get().peer_online());
    let settings_open = Signal::derive(move || {
        show_settings.get() || matches!(state.get(), ConnectionState::NotConfigured)
    });

    view! {
        <Shell
            state=state.into()
            peer_name=PEER_NAME
            self_name=SELF_NAME
            on_settings=Callback::new(move |()| show_settings.update(|open| *open = !*open))
        >
            <Show
                when=move || settings_open.get()
                fallback=move || {
                    view! {
                        <div class="chat-pane">
                            <MessageList
                                messages=messages.into()
                                peer_name=PEER_NAME
                                self_name=SELF_NAME
                            />
                            <Composer
                                disabled=offline
                                placeholder="Сообщение Кенту"
                                on_send=on_send
                            />
                        </div>
                    }
                }
            >
                <SettingsPanel
                    settings=settings.into()
                    on_saved=Callback::new(move |()| {
                        show_settings.set(false);
                        refresh_settings.run(());
                    })
                />
            </Show>
        </Shell>
    }
}

/// Moves one message to a new delivery status, by id.
fn set_status(messages: RwSignal<Vec<Message>>, id: &str, status: Status) {
    messages.update(|messages| {
        if let Some(message) = messages.iter_mut().find(|m| m.id == id) {
            message.status = Some(status);
        }
    });
}
