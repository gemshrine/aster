mod app;
mod bridge;
mod call;
mod chat;
mod connection;
mod sound;
mod ui;

fn main() {
    console_error_panic_hook::set_once();

    // The component gallery ships in debug builds only (spec 0008).
    #[cfg(debug_assertions)]
    if ui::gallery::requested() {
        leptos::mount::mount_to_body(ui::gallery::Gallery);
        return;
    }

    leptos::mount::mount_to_body(app::App);
}
