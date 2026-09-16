use leptos::prelude::*;

mod chat;
mod connection;
mod ui;

use chat::{Author, Message, Status};
use connection::ConnectionState;
use ui::chat::{Composer, MessageList};
use ui::shell::Shell;

fn main() {
    console_error_panic_hook::set_once();
    leptos::mount::mount_to_body(App);
}

#[component]
fn App() -> impl IntoView {
    // The gallery ships in debug builds only (spec 0008); release bundles
    // never contain it, so this collapses to the scaffold there.
    #[cfg(debug_assertions)]
    if ui::gallery::requested() {
        return ui::gallery::Gallery().into_any();
    }

    // Until #5 wires the core's `connection-state` event, the shell starts
    // from the state a fresh install is in.
    let state = RwSignal::new(ConnectionState::NotConfigured);

    // History comes from local SQLite in #6; until then the list starts empty
    // and sends only land in it optimistically.
    let messages = RwSignal::new(Vec::<Message>::new());
    let offline = Signal::derive(move || !state.get().peer_online());
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
        <Shell state=state.into() peer_name="Кент" self_name="Ты">
            <div class="chat-pane">
                <MessageList messages=messages.into() peer_name="Кент" self_name="Ты" />
                <Composer
                    disabled=offline
                    placeholder="Сообщение Кенту"
                    on_send=on_send
                />
            </div>
        </Shell>
    }
    .into_any()
}
