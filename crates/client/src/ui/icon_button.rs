//! Icon button — 32 r8 with an 18px icon, `sm` 26 with 15px (spec 0008).

use leptos::prelude::*;

use super::icons::{Icon, IconView};

/// Visual role. `Default` and `Sm` differ only in size; `Toggled` marks an
/// active tool (mic on, members shown), `Danger` a destructive one.
#[allow(dead_code)]
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum IconButtonKind {
    #[default]
    Default,
    Sm,
    Toggled,
    Danger,
}

impl IconButtonKind {
    fn size(self) -> (f64, f64) {
        match self {
            Self::Sm => (26.0, 15.0),
            _ => (32.0, 18.0),
        }
    }

    fn class(self) -> &'static str {
        match self {
            Self::Toggled => "icon-btn icon-btn--toggled",
            Self::Danger => "icon-btn icon-btn--danger",
            _ => "icon-btn",
        }
    }
}

#[component]
pub fn IconButton(
    icon: Icon,
    /// Accessible name — every icon button is unlabelled visually.
    label: &'static str,
    /// Takes a plain kind or a signal, so a toggle can flip it live.
    #[prop(optional, into)]
    kind: Signal<IconButtonKind>,
    #[prop(default = false.into(), into)] disabled: Signal<bool>,
    #[prop(optional)] on_click: Option<Callback<()>>,
) -> impl IntoView {
    let sizes = Signal::derive(move || kind.get().size());
    view! {
        <button
            type="button"
            class=move || kind.get().class()
            style=move || format!("--btn-size:{}px", sizes.get().0)
            aria-label=label
            title=label
            disabled=move || disabled.get()
            on:click=move |_| {
                if disabled.get() {
                    return;
                }
                if let Some(cb) = on_click {
                    cb.run(());
                }
            }
        >
            <IconView icon=icon size=Signal::derive(move || sizes.get().1) />
        </button>
    }
}
