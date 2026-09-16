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

/// A message as the core's history emits it (spec 0010).
#[derive(serde::Deserialize)]
struct CoreMessage {
    id: String,
    direction: Direction,
    body: String,
    sent_at: i64,
    status: Option<CoreStatus>,
}

#[derive(Clone, Copy, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
enum Direction {
    Out,
    In,
}

#[derive(Clone, Copy, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
enum CoreStatus {
    Sending,
    Sent,
    Queued,
    Delivered,
    Failed,
}

impl From<CoreMessage> for Message {
    fn from(core: CoreMessage) -> Self {
        Message {
            id: core.id,
            author: match core.direction {
                Direction::Out => Author::Me,
                Direction::In => Author::Peer,
            },
            body: core.body,
            sent_at: core.sent_at,
            status: core.status.map(|status| match status {
                CoreStatus::Sending => Status::Sending,
                CoreStatus::Sent => Status::Sent,
                CoreStatus::Queued => Status::Queued,
                CoreStatus::Delivered => Status::Delivered,
                CoreStatus::Failed => Status::Rejected {
                    message: "Не доставлено — очередь собеседника переполнена".into(),
                },
            }),
        }
    }
}

#[derive(serde::Serialize)]
struct HistoryArgs {
    before: Option<String>,
    limit: u32,
}

#[derive(serde::Serialize)]
struct SendArgs {
    body: String,
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

    leptos::task::spawn_local(async move {
        let args = HistoryArgs {
            before: None,
            limit: 200,
        };
        if let Ok(history) = bridge::invoke::<_, Vec<CoreMessage>>("load_history", &args).await {
            messages.update(|messages| {
                // Upserts that raced the load are newer than the stored copy.
                let live = std::mem::take(messages);
                *messages = history.into_iter().map(Message::from).collect();
                for message in live {
                    upsert(messages, message);
                }
            });
        }
    });

    bridge::listen::<CoreMessage>("message-upserted", move |message| {
        messages.update(|messages| upsert(messages, message.into()));
    });

    let on_send = Callback::new(move |body: String| {
        leptos::task::spawn_local(async move {
            // The core stores the message and announces it via
            // `message-upserted`; the reply only matters on refusal.
            let result: Result<CoreMessage, CommandError> =
                bridge::invoke("send_message", &SendArgs { body }).await;
            if let Err(err) = result {
                web_sys::console::error_1(
                    &format!("сообщение не отправлено: {}", err.message).into(),
                );
            }
        });
    });

    // An offline peer still gets messages through the server's queue, so
    // only a dead link disables the composer (spec 0004).
    let offline = Signal::derive(move || !state.get().can_send());
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

/// Replaces a message by id, or appends it in chronological order.
fn upsert(messages: &mut Vec<Message>, message: Message) {
    if let Some(existing) = messages.iter_mut().find(|m| m.id == message.id) {
        *existing = message;
        return;
    }
    let at = messages
        .iter()
        .rposition(|m| m.sent_at <= message.sent_at)
        .map_or(0, |i| i + 1);
    messages.insert(at, message);
}
