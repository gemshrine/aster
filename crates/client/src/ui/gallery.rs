//! Component gallery — every primitive in every state, for eyeballing changes
//! against the canvas. Debug builds only (spec 0008), reached at `#/gallery`.

use leptos::prelude::*;

use super::avatar::{Avatar, Presence};
use super::icon_button::{IconButton, IconButtonKind};
use super::icons::Icon;
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
