//! Component gallery — every primitive in every state, for eyeballing changes
//! against the canvas. Debug builds only (spec 0008), reached at `#/gallery`.

use leptos::prelude::*;

use crate::call::{CallState, EndReason};
use crate::chat::{Author, Message, Status};
use crate::connection::{ConnectionState, FailureReason};

use super::avatar::{Avatar, Presence};
use super::call::{CallOutcome, CallPane};
use super::chat::{Composer, MessageList};
use super::icon_button::{IconButton, IconButtonKind};
use super::icons::Icon;
use super::shell::Shell;
use super::text_field::TextField;

/// True when the page was opened at the gallery's dev route.
pub fn requested() -> bool {
    window()
        .location()
        .hash()
        .is_ok_and(|hash| hash == "#/gallery")
}

#[component]
fn Section(title: &'static str, note: &'static str, children: Children) -> impl IntoView {
    view! {
        <section style="margin-bottom:34px;">
            <h2 class="t-overline" style="margin-bottom:4px;">{title}</h2>
            <p class="t-caption" style="margin-bottom:14px;">{note}</p>
            <div style="display:flex; flex-wrap:wrap; align-items:flex-end; gap:22px;">
                {children()}
            </div>
        </section>
    }
}

#[component]
fn Case(label: &'static str, children: Children) -> impl IntoView {
    view! {
        <div style="display:flex; flex-direction:column; align-items:flex-start; gap:8px;">
            {children()}
            <span class="t-caption">{label}</span>
        </div>
    }
}

/// Mock conversation covering grouping, both bubble sides and every status.
fn mock_messages() -> Vec<Message> {
    let minute = 60 * 1000;
    let base = js_sys::Date::now() as i64 - 90 * minute;
    let msg = |id: &str, author, body: &str, offset: i64, status| Message {
        id: id.into(),
        author,
        body: body.into(),
        sent_at: base + offset * minute,
        status,
    };
    vec![
        msg(
            "1",
            Author::Peer,
            "Слушай, сервер поднял — TURN тоже отвечает.",
            0,
            None,
        ),
        msg(
            "2",
            Author::Peer,
            "Проверь у себя, я оставил токен в закреплённом.",
            1,
            None,
        ),
        msg(
            "3",
            Author::Me,
            "Подключился, всё видно.",
            3,
            Some(Status::Delivered),
        ),
        msg(
            "4",
            Author::Me,
            "Осталось прикрутить историю к SQLite — это #6.",
            4,
            Some(Status::Sent),
        ),
        msg(
            "5",
            Author::Peer,
            "Ага, а звонок тогда следующим.",
            20,
            None,
        ),
        msg(
            "6",
            Author::Me,
            "Сейчас отправлю правки.",
            25,
            Some(Status::Sending),
        ),
        msg(
            "7",
            Author::Me,
            "И ещё вот это, пока ты офлайн.",
            40,
            Some(Status::Queued),
        ),
        msg(
            "8",
            Author::Me,
            "А это уже не влезло.",
            60,
            Some(Status::Rejected {
                message: "Очередь переполнена — не доставлено".into(),
            }),
        ),
    ]
}

#[component]
fn CallCase() -> impl IntoView {
    let states = [
        ("calling", CallState::Calling),
        ("ringing", CallState::Ringing),
        ("connecting", CallState::Connecting),
        (
            "connected",
            CallState::Connected {
                since: js_sys::Date::now() as i64 - 74_000,
            },
        ),
    ];
    let state = RwSignal::new(CallState::Connected {
        since: js_sys::Date::now() as i64 - 74_000,
    });
    let muted = RwSignal::new(false);
    let remote_speaking = RwSignal::new(true);

    view! {
        <div style="width:100%;">
            <div style="display:flex; flex-wrap:wrap; gap:8px; margin-bottom:14px;">
                {states
                    .into_iter()
                    .map(|(label, value)| {
                        let value = StoredValue::new(value);
                        view! {
                            <button
                                class="peer__call t-ui-strong"
                                style="width:auto; margin:0; padding:0 12px; height:28px;"
                                type="button"
                                on:click=move |_| state.set(value.get_value())
                            >
                                {label}
                            </button>
                        }
                    })
                    .collect_view()}
                <button
                    class="peer__call t-ui-strong"
                    style="width:auto; margin:0; padding:0 12px; height:28px;"
                    type="button"
                    on:click=move |_| remote_speaking.update(|v| *v = !*v)
                >
                    "собеседник говорит"
                </button>
            </div>
            <div style="height:520px; border:1px solid var(--border); border-radius:var(--r-lg); overflow:hidden; background:var(--bg);">
                <CallPane
                    state=state.into()
                    peer_name="Кент"
                    self_name="Ты"
                    muted=muted
                    local_speaking=Signal::derive(|| false)
                    remote_speaking=remote_speaking
                    on_mute=Callback::new(move |()| muted.update(|m| *m = !*m))
                    on_accept=Callback::new(move |()| {
                        state.set(CallState::Connected { since: js_sys::Date::now() as i64 })
                    })
                    on_decline=Callback::new(move |()| state.set(CallState::Idle))
                    on_hang_up=Callback::new(move |()| state.set(CallState::Idle))
                />
            </div>
            <div style="margin-top:14px; background:var(--bg); padding:10px 0;">
                <CallOutcome
                    message=EndReason::Declined.message()
                    on_dismiss=Callback::new(|()| ())
                />
            </div>
        </div>
    }
}

#[component]
fn ChatCase() -> impl IntoView {
    let messages = RwSignal::new(mock_messages());
    let offline = RwSignal::new(false);
    let on_send = Callback::new(move |body: String| {
        messages.update(|messages| {
            messages.push(Message {
                id: format!("local-{}", messages.len()),
                author: Author::Me,
                body,
                sent_at: js_sys::Date::now() as i64,
                status: Some(Status::Sending),
            })
        })
    });

    view! {
        <div style="width:100%;">
            <div style="display:flex; gap:8px; margin-bottom:14px;">
                <button
                    class="peer__call t-ui-strong"
                    style="width:auto; margin:0; padding:0 12px; height:28px;"
                    type="button"
                    on:click=move |_| offline.update(|v| *v = !*v)
                >
                    {move || {
                        if offline.get() { "композер: disabled" } else { "композер: default" }
                    }}
                </button>
                <button
                    class="peer__call t-ui-strong"
                    style="width:auto; margin:0; padding:0 12px; height:28px;"
                    type="button"
                    on:click=move |_| messages.set(Vec::new())
                >
                    "пустая лента"
                </button>
                <button
                    class="peer__call t-ui-strong"
                    style="width:auto; margin:0; padding:0 12px; height:28px;"
                    type="button"
                    on:click=move |_| messages.set(mock_messages())
                >
                    "вернуть переписку"
                </button>
            </div>
            <div style="height:520px; border:1px solid var(--border); border-radius:var(--r-lg); overflow:hidden; background:var(--bg);">
                <div class="chat-pane">
                    <MessageList messages=messages.into() peer_name="Кент" self_name="Ты" />
                    <Composer
                        disabled=offline
                        placeholder="Сообщение Кенту"
                        on_send=on_send
                    />
                </div>
            </div>
        </div>
    }
}

#[component]
fn ShellCase() -> impl IntoView {
    let states = [
        ("not_configured", ConnectionState::NotConfigured),
        ("connecting", ConnectionState::Connecting { attempt: 2 }),
        (
            "connected",
            ConnectionState::Connected {
                user_id: "morphe".into(),
                peer_online: true,
            },
        ),
        (
            "reconnecting",
            ConnectionState::Reconnecting {
                attempt: 3,
                retry_in_ms: 4000,
            },
        ),
        ("disconnected", ConnectionState::Disconnected),
        (
            "failed",
            ConnectionState::Failed {
                reason: FailureReason::SessionReplaced,
            },
        ),
    ];
    let state = RwSignal::new(ConnectionState::Connected {
        user_id: "morphe".into(),
        peer_online: true,
    });

    view! {
        <div style="width:100%;">
            <div style="display:flex; flex-wrap:wrap; gap:8px; margin-bottom:14px;">
                {states
                    .into_iter()
                    .map(|(label, value)| {
                        let value = StoredValue::new(value);
                        view! {
                            <button
                                class="peer__call t-ui-strong"
                                style="width:auto; margin:0; padding:0 12px; height:28px;"
                                type="button"
                                on:click=move |_| state.set(value.get_value())
                            >
                                {label}
                            </button>
                        }
                    })
                    .collect_view()}
            </div>
            <div style="height:420px; resize:both; overflow:hidden; border:1px solid var(--border); border-radius:var(--r-lg);">
                <div style="height:100%; zoom:0.72;">
                    <Shell
                        state=state.into()
                        peer_name="Кент"
                        self_name="Ты"
                        on_settings=Callback::new(|()| ())
                        in_call=false
                        on_call=Callback::new(|()| ())
                    >
                        <div style="padding:26px 22px;">
                            <p class="t-ui" style="color:var(--text-secondary);">
                                "Основная область — лента и композер в #6."
                            </p>
                        </div>
                    </Shell>
                </div>
            </div>
        </div>
    }
}

#[component]
pub fn Gallery() -> impl IntoView {
    let empty = RwSignal::new(String::new());
    let filled = RwSignal::new(String::from("Botanical Society"));
    let wrong = RwSignal::new(String::from("kent@"));
    let frozen = RwSignal::new(String::from("Disconnected"));

    view! {
        <main style="padding:34px 40px; max-width:900px;">
            <h1 class="t-display-m" style="margin-bottom:2px;">"UI primitives"</h1>
            <p class="t-ui" style="color:var(--text-secondary); margin-bottom:30px;">
                "Debug-only gallery — spec 0008, artboard "<em>"Spec"</em>"."
            </p>

            <Section title="Shell" note="title 44 · sidebar 264 · main flex 1 — all six connection states">
                <ShellCase />
            </Section>

            <Section title="Call" note="плитки r-arch · таймер из since · контролы 46 · входящий вызов">
                <CallCase />
            </Section>

            <Section title="Chat" note="лента 22/18 · gap 15 · группировка < 5 мин · пузыри max 520 · композер h52">
                <ChatCase />
            </Section>

            <Section title="Icon button" note="32 · r8 · icon 18 / sm 26 · icon 15">
                <Case label="default">
                    <IconButton icon=Icon::Search label="Search" />
                </Case>
                <Case label="hover — point at it">
                    <IconButton icon=Icon::Pin label="Pin" />
                </Case>
                <Case label="toggled">
                    <IconButton icon=Icon::Mic label="Microphone" kind=IconButtonKind::Toggled />
                </Case>
                <Case label="danger">
                    <IconButton icon=Icon::Phone label="Leave call" kind=IconButtonKind::Danger />
                </Case>
                <Case label="disabled">
                    <IconButton icon=Icon::Send label="Send" disabled=true />
                </Case>
                <Case label="sm">
                    <IconButton icon=Icon::Close label="Close" kind=IconButtonKind::Sm />
                </Case>
            </Section>

            <Section title="Avatar · sizes" note="18 · 28 · 30 · 34 · 38 · 56 · 80 · 88">
                {[18.0, 28.0, 30.0, 34.0, 38.0, 56.0, 80.0, 88.0]
                    .into_iter()
                    .map(|size| {
                        view! {
                            <div style="display:flex; flex-direction:column; align-items:center; gap:8px;">
                                <Avatar name="Theo Lindqvist".into() size=size />
                                <span class="t-caption">{format!("{size:.0}")}</span>
                            </div>
                        }
                    })
                    .collect_view()}
            </Section>

            <Section title="Avatar · presence" note="ring 12px, 2.5px cut-out in the surface colour">
                <Case label="online">
                    <Avatar name="Mira Voss".into() presence=Presence::Online />
                </Case>
                <Case label="idle">
                    <Avatar name="Sam Okafor".into() presence=Presence::Idle />
                </Case>
                <Case label="dnd">
                    <Avatar name="Priya Anand".into() presence=Presence::Dnd />
                </Case>
                <Case label="offline">
                    <Avatar name="Kaya Moreau".into() presence=Presence::Offline />
                </Case>
                <Case label="speaking">
                    <Avatar name="Juna Okoye".into() presence=Presence::Speaking />
                </Case>
                <Case label="no presence">
                    <Avatar name="Noah Bergström".into() />
                </Case>
            </Section>

            <Section title="Text field" note="h34 · r9 · px12 (icon 32) · 13px">
                <div style="width:240px;">
                    <TextField value=empty placeholder="Find a conversation" icon=Icon::Search />
                    <p class="t-caption" style="margin-top:8px;">"default · with icon"</p>
                </div>
                <div style="width:240px;">
                    <TextField value=filled aria_label="Server" />
                    <p class="t-caption" style="margin-top:8px;">"filled — click to focus"</p>
                </div>
                <div style="width:240px;">
                    <TextField value=wrong error="Не похоже на адрес сервера" />
                    <p class="t-caption" style="margin-top:8px;">"error"</p>
                </div>
                <div style="width:240px;">
                    <TextField value=frozen disabled=true />
                    <p class="t-caption" style="margin-top:8px;">"disabled"</p>
                </div>
            </Section>

            <Section title="Icons" note="24×24 · stroke 1.6 · currentColor">
                {Icon::ALL
                    .iter()
                    .map(|icon| {
                        view! {
                            <div style="display:flex; flex-direction:column; align-items:center; gap:6px; width:62px; color:var(--text-secondary);">
                                <super::icons::IconView icon=*icon size=20.0 />
                                <span class="t-caption">{icon.name()}</span>
                            </div>
                        }
                    })
                    .collect_view()}
            </Section>
        </main>
    }
}
