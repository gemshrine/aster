use leptos::prelude::*;

mod ui;

use ui::icons::{Icon, IconView};

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

    view! {
        <main style="padding:24px; display:flex; align-items:center; gap:10px;">
            <IconView icon=Icon::Compass size=24.0 />
            <h1 class="t-display-m">"Aster"</h1>
            <p class="t-ui" style="color:var(--text-secondary);">
                "Client scaffold. Chat lands in "<code>"#6"</code>", voice calling in "
                <code>"#7"</code>"."
            </p>
        </main>
    }
    .into_any()
}
