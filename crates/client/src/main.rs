use leptos::prelude::*;

fn main() {
    console_error_panic_hook::set_once();
    leptos::mount::mount_to_body(App);
}

#[component]
fn App() -> impl IntoView {
    view! {
        <main>
            <h1>"gavno-voice"</h1>
            <p>"Client scaffold. Chat/presence UI lands in "<code>"#4"</code>", voice calling in "<code>"#6"</code>"."</p>
        </main>
    }
}
