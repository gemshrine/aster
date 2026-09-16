use leptos::prelude::*;

mod ui;

use ui::icons::{Icon, IconView};

fn main() {
    console_error_panic_hook::set_once();
    leptos::mount::mount_to_body(App);
}

#[component]
fn App() -> impl IntoView {
    view! {
        <main style="padding:24px; display:flex; align-items:center; gap:10px;">
            <IconView icon=Icon::Compass size=24.0 />
            <h1 class="t-display-m">"Aster"</h1>
            <p class="t-ui" style="color:var(--text-secondary);">
                "Client scaffold. UI primitives land in "<code>"#18"</code>
                ", chat in "<code>"#6"</code>", voice calling in "<code>"#7"</code>"."
            </p>
        </main>
    }
}
