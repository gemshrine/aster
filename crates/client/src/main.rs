use leptos::prelude::*;

mod connection;
mod ui;

use connection::ConnectionState;
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

    view! {
        <Shell state=state.into() peer_name="Кент" self_name="Ты">
            <div style="padding:26px 22px;">
                <p class="t-ui" style="color:var(--text-secondary);">
                    "Лента и композер появятся в "<code>"#6"</code>", звонок — в "
                    <code>"#7"</code>"."
                </p>
            </div>
        </Shell>
    }
    .into_any()
}
