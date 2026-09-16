//! Wiring between the core (spec 0009) and the UI (spec 0008).

use leptos::prelude::*;
use wasm_bindgen::prelude::Closure;
use wasm_bindgen::JsCast;

use crate::bridge::{self, CommandError};
use crate::call::{CallState, EndReason};
use crate::chat::{Author, Message, Status};
use crate::connection::ConnectionState;
use crate::sound::{self, Loop, Signal as Sound};
use crate::ui::call::{CallOutcome, CallPane};
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

/// `call-audio` (spec 0011).
#[derive(Clone, Copy, serde::Deserialize)]
struct CallAudio {
    muted: bool,
    local_speaking: bool,
    remote_speaking: bool,
}

#[derive(serde::Deserialize)]
struct CallAudioError {
    message: String,
}

#[derive(serde::Serialize)]
struct FocusArgs {
    focused: bool,
}

#[derive(serde::Serialize)]
struct MuteArgs {
    muted: bool,
}

#[derive(serde::Serialize)]
struct SendArgs {
    body: String,
}

#[component]
pub fn App() -> impl IntoView {
    let state = RwSignal::new(ConnectionState::NotConfigured);
    let messages = RwSignal::new(Vec::<Message>::new());
    // The core counts unread messages and needs to know when the user is
    // actually looking at the window (spec 0012).
    let focused = RwSignal::new(true);
    watch_focus(focused);
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
        let message: Message = message.into();
        // Only a new incoming message earns a sound, and only when the window
        // is in the background — a status change is not news (spec 0012).
        let announce = message.author == Author::Peer
            && !focused.get_untracked()
            && !messages.read_untracked().iter().any(|m| m.id == message.id);
        if announce {
            sound::play(Sound::Message);
        }
        messages.update(|messages| upsert(messages, message));
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
    // --- call (spec 0011) ---

    let call = RwSignal::new(CallState::Idle);
    let outcome = RwSignal::new(None::<EndReason>);
    let muted = RwSignal::new(false);
    let local_speaking = RwSignal::new(false);
    let remote_speaking = RwSignal::new(false);

    leptos::task::spawn_local(async move {
        if let Ok(current) = bridge::invoke::<_, CallState>("call_state", &()).await {
            call.set(current);
        }
    });

    // Attention signals (spec 0012). The ringtone and the dial tone repeat
    // until the call moves on, so they live in a slot that dropping silences.
    let ringer = StoredValue::new(None::<Loop>);
    let silence = move || ringer.set_value(None);

    bridge::listen::<CallState>("call-state", move |next| {
        match &next {
            CallState::Ringing => ringer.set_value(Some(Loop::start(Sound::Ring, 900.0))),
            CallState::Calling => ringer.set_value(Some(Loop::start(Sound::Dial, 2200.0))),
            CallState::Connected { .. } => {
                silence();
                sound::play(Sound::Connected);
            }
            CallState::Ended { .. } => {
                silence();
                sound::play(Sound::Ended);
            }
            CallState::Connecting | CallState::Idle => silence(),
        }
        // `ended` is momentary: the core returns to idle right after it, so
        // the outcome lives here until the next call or a dismissal.
        if let CallState::Ended { reason } = next {
            outcome.set(Some(reason));
            local_speaking.set(false);
            remote_speaking.set(false);
        } else if next != CallState::Idle {
            outcome.set(None);
        }
        call.set(next);
    });

    bridge::listen::<CallAudio>("call-audio", move |audio| {
        muted.set(audio.muted);
        local_speaking.set(audio.local_speaking);
        remote_speaking.set(audio.remote_speaking);
    });

    bridge::listen::<CallAudioError>("call-audio-error", move |err| {
        web_sys::console::error_1(&format!("звук: {}", err.message).into());
    });

    /// Fires a call command and reports a refusal to the console; the core
    /// keeps authoritative state, so there is nothing to roll back here.
    fn call_command(cmd: &'static str) {
        leptos::task::spawn_local(async move {
            let result: Result<(), CommandError> = bridge::invoke(cmd, &()).await;
            if let Err(err) = result {
                web_sys::console::error_1(&format!("{cmd}: {}", err.message).into());
            }
        });
    }

    let on_call = Callback::new(move |()| call_command("start_call"));
    let on_accept = Callback::new(move |()| call_command("accept_call"));
    let on_decline = Callback::new(move |()| call_command("decline_call"));
    let on_hang_up = Callback::new(move |()| call_command("hang_up"));
    let on_mute = Callback::new(move |()| {
        let next = !muted.get();
        // Optimistic: `call-audio` confirms it a moment later.
        muted.set(next);
        leptos::task::spawn_local(async move {
            let result: Result<(), CommandError> =
                bridge::invoke("set_muted", &MuteArgs { muted: next }).await;
            if let Err(err) = result {
                web_sys::console::error_1(&format!("set_muted: {}", err.message).into());
            }
        });
    });

    let in_call = Signal::derive(move || call.get().is_active());

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
            in_call=in_call
            on_call=on_call
        >
            <Show
                when=move || settings_open.get()
                fallback=move || {
                    view! {
                        <Show
                            when=move || in_call.get()
                            fallback=move || {
                                view! {
                                    <div class="chat-pane">
                                        <MessageList
                                            messages=messages.into()
                                            peer_name=PEER_NAME
                                            self_name=SELF_NAME
                                        />
                                        {move || {
                                            outcome
                                                .get()
                                                .map(|reason| {
                                                    view! {
                                                        <CallOutcome
                                                            message=reason.message()
                                                            on_dismiss=Callback::new(move |()| {
                                                                outcome.set(None)
                                                            })
                                                        />
                                                    }
                                                })
                                        }}
                                        <Composer
                                            disabled=offline
                                            placeholder="Сообщение Кенту"
                                            on_send=on_send
                                        />
                                    </div>
                                }
                            }
                        >
                            <CallPane
                                state=call.into()
                                peer_name=PEER_NAME
                                self_name=SELF_NAME
                                muted=muted
                                local_speaking=local_speaking
                                remote_speaking=remote_speaking
                                on_mute=on_mute
                                on_accept=on_accept
                                on_decline=on_decline
                                on_hang_up=on_hang_up
                            />
                        </Show>
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

/// Mirrors window focus into `focused` and tells the core about it.
fn watch_focus(focused: RwSignal<bool>) {
    let Some(window) = web_sys::window() else {
        return;
    };
    for (event, value) in [("focus", true), ("blur", false)] {
        let handler = Closure::<dyn FnMut()>::new(move || {
            if focused.get_untracked() == value {
                return;
            }
            focused.set(value);
            leptos::task::spawn_local(async move {
                // The command lands with the tray work (#61); until then the
                // bridge just reports that the core does not know it.
                let _: Result<(), CommandError> =
                    bridge::invoke("set_window_focused", &FocusArgs { focused: value }).await;
            });
        });
        let _ = window.add_event_listener_with_callback(event, handler.as_ref().unchecked_ref());
        handler.forget();
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
