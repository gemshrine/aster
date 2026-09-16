//! Lucide icons (ISC), inlined.
//!
//! Only the set spec 0008 names is vendored — 24x24 viewBox, round caps,
//! `currentColor`. Stroke is 1.6 per the canvas, thickened to 1.8 below 16px
//! so small icons keep their weight.

use leptos::prelude::*;

/// Every icon the client draws. Variants are wired up screen by screen
/// (#18 and the screens after it), so unused ones are expected for now.
#[allow(dead_code)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Icon {
    Hash,
    Voice,
    Search,
    Mic,
    Headphones,
    Camera,
    Screen,
    Phone,
    Users,
    Pin,
    Inbox,
    Plus,
    Send,
    Emoji,
    Chevron,
    Close,
    Edit,
    Dm,
    Image,
    Compass,
}

impl Icon {
    /// Every icon, in declaration order — the debug gallery walks this.
    pub const ALL: [Self; 20] = [
        Self::Hash,
        Self::Voice,
        Self::Search,
        Self::Mic,
        Self::Headphones,
        Self::Camera,
        Self::Screen,
        Self::Phone,
        Self::Users,
        Self::Pin,
        Self::Inbox,
        Self::Plus,
        Self::Send,
        Self::Emoji,
        Self::Chevron,
        Self::Close,
        Self::Edit,
        Self::Dm,
        Self::Image,
        Self::Compass,
    ];

    /// Lowercase variant name, for the gallery's captions.
    pub fn name(self) -> &'static str {
        match self {
            Self::Hash => "hash",
            Self::Voice => "voice",
            Self::Search => "search",
            Self::Mic => "mic",
            Self::Headphones => "headphones",
            Self::Camera => "camera",
            Self::Screen => "screen",
            Self::Phone => "phone",
            Self::Users => "users",
            Self::Pin => "pin",
            Self::Inbox => "inbox",
            Self::Plus => "plus",
            Self::Send => "send",
            Self::Emoji => "emoji",
            Self::Chevron => "chevron",
            Self::Close => "close",
            Self::Edit => "edit",
            Self::Dm => "dm",
            Self::Image => "image",
            Self::Compass => "compass",
        }
    }

    /// Inner markup of the Lucide source SVG, without the `<svg>` wrapper.
    fn paths(self) -> &'static str {
        match self {
            Self::Hash => "<line x1=\"4\" x2=\"20\" y1=\"9\" y2=\"9\" /> <line x1=\"4\" x2=\"20\" y1=\"15\" y2=\"15\" /> <line x1=\"10\" x2=\"8\" y1=\"3\" y2=\"21\" /> <line x1=\"16\" x2=\"14\" y1=\"3\" y2=\"21\" />",
            Self::Voice => "<path d=\"M11 4.702a.705.705 0 0 0-1.203-.498L6.413 7.587A1.4 1.4 0 0 1 5.416 8H3a1 1 0 0 0-1 1v6a1 1 0 0 0 1 1h2.416a1.4 1.4 0 0 1 .997.413l3.383 3.384A.705.705 0 0 0 11 19.298z\" /> <path d=\"M16 9a5 5 0 0 1 0 6\" /> <path d=\"M19.364 18.364a9 9 0 0 0 0-12.728\" />",
            Self::Search => "<path d=\"m21 21-4.34-4.34\" /> <circle cx=\"11\" cy=\"11\" r=\"8\" />",
            Self::Mic => "<path d=\"M12 19v3\" /> <path d=\"M19 10v2a7 7 0 0 1-14 0v-2\" /> <rect x=\"9\" y=\"2\" width=\"6\" height=\"13\" rx=\"3\" />",
            Self::Headphones => "<path d=\"M3 14h3a2 2 0 0 1 2 2v3a2 2 0 0 1-2 2H5a2 2 0 0 1-2-2v-7a9 9 0 0 1 18 0v7a2 2 0 0 1-2 2h-1a2 2 0 0 1-2-2v-3a2 2 0 0 1 2-2h3\" />",
            Self::Camera => "<path d=\"M13.997 4a2 2 0 0 1 1.76 1.05l.486.9A2 2 0 0 0 18.003 7H20a2 2 0 0 1 2 2v9a2 2 0 0 1-2 2H4a2 2 0 0 1-2-2V9a2 2 0 0 1 2-2h1.997a2 2 0 0 0 1.759-1.048l.489-.904A2 2 0 0 1 10.004 4z\" /> <circle cx=\"12\" cy=\"13\" r=\"3\" />",
            Self::Screen => "<path d=\"M13 3H4a2 2 0 0 0-2 2v10a2 2 0 0 0 2 2h16a2 2 0 0 0 2-2v-3\" /> <path d=\"M8 21h8\" /> <path d=\"M12 17v4\" /> <path d=\"m17 8 5-5\" /> <path d=\"M17 3h5v5\" />",
            Self::Phone => "<path d=\"M13.832 16.568a1 1 0 0 0 1.213-.303l.355-.465A2 2 0 0 1 17 15h3a2 2 0 0 1 2 2v3a2 2 0 0 1-2 2A18 18 0 0 1 2 4a2 2 0 0 1 2-2h3a2 2 0 0 1 2 2v3a2 2 0 0 1-.8 1.6l-.468.351a1 1 0 0 0-.292 1.233 14 14 0 0 0 6.392 6.384\" />",
            Self::Users => "<path d=\"M16 21v-2a4 4 0 0 0-4-4H6a4 4 0 0 0-4 4v2\" /> <path d=\"M16 3.128a4 4 0 0 1 0 7.744\" /> <path d=\"M22 21v-2a4 4 0 0 0-3-3.87\" /> <circle cx=\"9\" cy=\"7\" r=\"4\" />",
            Self::Pin => "<path d=\"M12 17v5\" /> <path d=\"M9 10.76a2 2 0 0 1-1.11 1.79l-1.78.9A2 2 0 0 0 5 15.24V16a1 1 0 0 0 1 1h12a1 1 0 0 0 1-1v-.76a2 2 0 0 0-1.11-1.79l-1.78-.9A2 2 0 0 1 15 10.76V7a1 1 0 0 1 1-1 2 2 0 0 0 0-4H8a2 2 0 0 0 0 4 1 1 0 0 1 1 1z\" />",
            Self::Inbox => "<polyline points=\"22 12 16 12 14 15 10 15 8 12 2 12\" /> <path d=\"M5.45 5.11 2 12v6a2 2 0 0 0 2 2h16a2 2 0 0 0 2-2v-6l-3.45-6.89A2 2 0 0 0 16.76 4H7.24a2 2 0 0 0-1.79 1.11z\" />",
            Self::Plus => "<path d=\"M5 12h14\" /> <path d=\"M12 5v14\" />",
            Self::Send => "<path d=\"M14.536 21.686a.5.5 0 0 0 .937-.024l6.5-19a.496.496 0 0 0-.635-.635l-19 6.5a.5.5 0 0 0-.024.937l7.93 3.18a2 2 0 0 1 1.112 1.11z\" /> <path d=\"m21.854 2.147-10.94 10.939\" />",
            Self::Emoji => "<path d=\"M15 10V9\" /> <path d=\"M16.472 15a6 6 0 01-8.943 0\" /> <path d=\"M9 10V9\" /> <circle cx=\"12\" cy=\"12\" r=\"10\" />",
            Self::Chevron => "<path d=\"m6 9 6 6 6-6\" />",
            Self::Close => "<path d=\"M18 6 6 18\" /> <path d=\"m6 6 12 12\" />",
            Self::Edit => "<path d=\"M21.174 6.812a1 1 0 0 0-3.986-3.987L3.842 16.174a2 2 0 0 0-.5.83l-1.321 4.352a.5.5 0 0 0 .623.622l4.353-1.32a2 2 0 0 0 .83-.497z\" /> <path d=\"m15 5 4 4\" />",
            Self::Dm => "<path d=\"M2.992 16.342a2 2 0 0 1 .094 1.167l-1.065 3.29a1 1 0 0 0 1.236 1.168l3.413-.998a2 2 0 0 1 1.099.092 10 10 0 1 0-4.777-4.719\" />",
            Self::Image => "<rect width=\"18\" height=\"18\" x=\"3\" y=\"3\" rx=\"2\" ry=\"2\" /> <circle cx=\"9\" cy=\"9\" r=\"2\" /> <path d=\"m21 15-3.086-3.086a2 2 0 0 0-2.828 0L6 21\" />",
            Self::Compass => "<circle cx=\"12\" cy=\"12\" r=\"10\" /> <path d=\"m16.24 7.76-1.804 5.411a2 2 0 0 1-1.265 1.265L7.76 16.24l1.804-5.411a2 2 0 0 1 1.265-1.265z\" />",
        }
    }
}

/// Renders `icon` at `size` px. `stroke` overrides the size-derived width.
#[component]
pub fn IconView(
    icon: Icon,
    #[prop(default = 18.0)] size: f64,
    #[prop(optional)] stroke: Option<f64>,
) -> impl IntoView {
    let stroke = stroke.unwrap_or(if size < 16.0 { 1.8 } else { 1.6 });
    view! {
        <svg
            xmlns="http://www.w3.org/2000/svg"
            width=size
            height=size
            viewBox="0 0 24 24"
            fill="none"
            stroke="currentColor"
            stroke-width=stroke
            stroke-linecap="round"
            stroke-linejoin="round"
            aria-hidden="true"
            focusable="false"
            inner_html=icon.paths()
        ></svg>
    }
}
