//! Text field — h34 r9, 13px, padded 12 (32 with a leading icon); spec 0008.

use leptos::prelude::*;

use super::icons::{Icon, IconView};

#[component]
pub fn TextField(
    /// Bound text. The caller owns it so fields stay controlled.
    value: RwSignal<String>,
    #[prop(optional, into)] placeholder: Option<String>,
    #[prop(optional)] icon: Option<Icon>,
    /// Error text; its presence is what puts the field in the error state.
    #[prop(optional, into)]
    error: Option<String>,
    #[prop(default = false)] disabled: bool,
    #[prop(optional, into)] aria_label: Option<String>,
) -> impl IntoView {
    let mut class = String::from("field");
    if icon.is_some() {
        class.push_str(" field--with-icon");
    }
    if error.is_some() {
        class.push_str(" field--error");
    }
    let message = error.clone();
    view! {
        <div>
            <div class=class>
                {icon
                    .map(|icon| {
                        view! {
                            <span class="field__icon">
                                <IconView icon=icon size=15.0 />
                            </span>
                        }
                    })}
                <input
                    class="field__input selectable"
                    type="text"
                    prop:value=move || value.get()
                    placeholder=placeholder
                    disabled=disabled
                    aria-label=aria_label
                    aria-invalid=error.is_some().then_some("true")
                    on:input:target=move |ev| value.set(ev.target().value())
                />
            </div>
            {message
                .map(|message| {
                    view! { <p class="field__message t-caption">{message}</p> }
                })}
        </div>
    }
}
