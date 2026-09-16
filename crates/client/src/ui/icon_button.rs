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
    #[prop(default = IconButtonKind::Default)] kind: IconButtonKind,
    #[prop(default = false)] disabled: bool,
    #[prop(optional)] on_click: Option<Callback<()>>,
) -> impl IntoView {
    let (box_size, icon_size) = kind.size();
    view! {
        <button
            type="button"
            class=kind.class()
            style=format!("--btn-size:{box_size}px")
            aria-label=label
            title=label
            disabled=disabled
            on:click=move |_| {
                if let Some(cb) = on_click {
                    cb.run(());
                }
            }
        >
            <IconView icon=icon size=icon_size />
        </button>
    }
}
