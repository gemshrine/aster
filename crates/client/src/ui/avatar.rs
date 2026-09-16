//! Avatar — initials in the display face at 0.42x the box, 12px status ring
//! outlined 2.5px in the surface colour behind it (spec 0008).

use leptos::prelude::*;

#[allow(dead_code)]
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Presence {
    Online,
    Idle,
    Dnd,
    #[default]
    Offline,
    /// Offline-style dot, plus the speaking ring around the face.
    Speaking,
}

impl Presence {
    fn status_class(self) -> &'static str {
        match self {
            Self::Online | Self::Speaking => "avatar__status avatar__status--online",
            Self::Idle => "avatar__status avatar__status--idle",
            Self::Dnd => "avatar__status avatar__status--dnd",
            Self::Offline => "avatar__status avatar__status--offline",
        }
    }

    fn label(self) -> &'static str {
        match self {
            Self::Online => "online",
            Self::Idle => "idle",
            Self::Dnd => "do not disturb",
            Self::Offline => "offline",
            Self::Speaking => "speaking",
        }
    }
}

/// First letters of up to two words — "Theo Lindqvist" becomes "TL".
fn initials(name: &str) -> String {
    name.split_whitespace()
        .filter_map(|word| word.chars().next())
        .take(2)
        .flat_map(char::to_uppercase)
        .collect()
}

#[component]
pub fn Avatar(
    /// Full name; initials and the accessible label come from it.
    name: String,
    #[prop(default = 38.0)] size: f64,
    /// Takes a plain presence or a signal, so a card can follow the peer.
    #[prop(optional, into)]
    presence: Option<Signal<Presence>>,
    /// Surface the avatar sits on, so the status ring cuts out of it.
    #[prop(default = "var(--bg)")]
    ring_bg: &'static str,
) -> impl IntoView {
    let text = initials(&name);
    let label = Signal::derive(move || match presence {
        Some(presence) => format!("{name} — {}", presence.get().label()),
        None => name.clone(),
    });
    let speaking = move || presence.is_some_and(|p| p.get() == Presence::Speaking);
    view! {
        <span
            class="avatar"
            class:avatar--speaking=speaking
            style=format!("--size:{size}px; --status-ring-bg:{ring_bg}")
            title=move || label.get()
            aria-label=move || label.get()
            role="img"
        >
            <span class="avatar__face">{text}</span>
            {presence
                .map(|presence| view! { <span class=move || presence.get().status_class()></span> })}
        </span>
    }
}

#[cfg(test)]
mod tests {
    use super::initials;

    #[test]
    fn takes_first_letter_of_first_two_words() {
        assert_eq!(initials("Theo Lindqvist"), "TL");
        assert_eq!(initials("Mira Voss Anderson"), "MV");
    }

    #[test]
    fn handles_single_word_and_blank() {
        assert_eq!(initials("kent"), "K");
        assert_eq!(initials("   "), "");
    }

    #[test]
    fn uppercases_non_ascii() {
        assert_eq!(initials("кент ивaнов"), "КИ");
    }
}
