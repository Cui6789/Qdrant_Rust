//! Inline SVG icon set. Icons inherit `currentColor` and never load assets.

use yew::prelude::*;

fn icon(paths: Html) -> Html {
    html! {
        <svg class="icon" viewBox="0 0 24 24" fill="none" stroke="currentColor"
            stroke-width="1.7" stroke-linecap="round" stroke-linejoin="round"
            aria-hidden="true">
            { paths }
        </svg>
    }
}

pub fn logo() -> Html {
    html! {
        <svg class="brand-mark" viewBox="0 0 40 40" fill="none" aria-hidden="true">
            <defs>
                <linearGradient id="brandGradient" x1="4" y1="4" x2="36" y2="36"
                    gradientUnits="userSpaceOnUse">
                    <stop stop-color="#2997ff"/>
                    <stop offset="0.55" stop-color="#0071e3"/>
                    <stop offset="1" stop-color="#5e5ce6"/>
                </linearGradient>
            </defs>
            <rect x="1" y="1" width="38" height="38" rx="11" fill="url(#brandGradient)"/>
            <circle cx="13" cy="14" r="3.1" fill="white" fill-opacity="0.95"/>
            <circle cx="27.5" cy="12.5" r="2.4" fill="white" fill-opacity="0.6"/>
            <circle cx="15.5" cy="27.5" r="2.4" fill="white" fill-opacity="0.6"/>
            <circle cx="28" cy="26" r="3.5" fill="white" fill-opacity="0.95"/>
            <path d="M15.8 15.4 L25.4 13.4 M14.4 17 L15 24.8 M18 27.2 L24.8 26.4"
                stroke="white" stroke-opacity="0.7" stroke-width="1.4"
                stroke-linecap="round"/>
        </svg>
    }
}

pub fn database() -> Html {
    icon(html! {
        <>
            <ellipse cx="12" cy="5.5" rx="7.5" ry="3"/>
            <path d="M4.5 5.5v6c0 1.66 3.36 3 7.5 3s7.5-1.34 7.5-3v-6"/>
            <path d="M4.5 11.5v6c0 1.66 3.36 3 7.5 3s7.5-1.34 7.5-3v-6"/>
        </>
    })
}

pub fn layers() -> Html {
    icon(html! {
        <>
            <path d="M12 3 3 7.5 12 12l9-4.5L12 3Z"/>
            <path d="m3 12 9 4.5L21 12"/>
            <path d="m3 16.5 9 4.5 9-4.5"/>
        </>
    })
}

pub fn spark() -> Html {
    icon(html! {
        <path d="M12 3.5 13.8 9l5.7 1.8-5.7 1.8L12 18.3l-1.8-5.7L4.5 10.8 10.2 9 12 3.5ZM18.5 16.5l.8 2.2 2.2.8-2.2.8-.8 2.2-.8-2.2-2.2-.8 2.2-.8.8-2.2Z"/>
    })
}

pub fn refresh() -> Html {
    icon(html! {
        <>
            <path d="M20 11a8 8 0 1 0-2.3 6.4"/>
            <path d="M20 5v6h-6"/>
        </>
    })
}

pub fn search() -> Html {
    icon(html! {
        <>
            <circle cx="11" cy="11" r="6.5"/>
            <path d="m16 16 4.5 4.5"/>
        </>
    })
}

pub fn trash() -> Html {
    icon(html! {
        <>
            <path d="M4 7h16M9 7V4.8h6V7M6.5 7l.8 12.2h9.4L17.5 7"/>
            <path d="M10 11v4.5M14 11v4.5"/>
        </>
    })
}

pub fn close() -> Html {
    icon(html! { <path d="m6 6 12 12M18 6 6 18"/> })
}

pub fn check() -> Html {
    icon(html! { <path d="m5 12.5 4.5 4.5L19 7.5"/> })
}

pub fn chevron_right() -> Html {
    icon(html! { <path d="m9 5 7 7-7 7"/> })
}

pub fn upload() -> Html {
    icon(html! {
        <>
            <path d="M12 16V4M7 9l5-5 5 5"/>
            <path d="M5 16v3h14v-3"/>
        </>
    })
}

pub fn file() -> Html {
    icon(html! {
        <>
            <path d="M6 3h7.5L19 8.5V21H6V3Z"/>
            <path d="M13.5 3v5.5H19"/>
        </>
    })
}

pub fn cpu() -> Html {
    icon(html! {
        <>
            <rect x="6" y="6" width="12" height="12" rx="2.5"/>
            <rect x="9.5" y="9.5" width="5" height="5" rx="1"/>
            <path d="M9 3v3M15 3v3M9 18v3M15 18v3M3 9h3M3 15h3M18 9h3M18 15h3"/>
        </>
    })
}

pub fn activity() -> Html {
    icon(html! { <path d="M3 12h4l2.5-7 4.5 14 2.5-7h4.5"/> })
}

pub fn clock() -> Html {
    icon(html! {
        <>
            <circle cx="12" cy="12" r="8.5"/>
            <path d="M12 7v5.2l3.4 2"/>
        </>
    })
}

pub fn play() -> Html {
    icon(html! { <path d="M8 5.5v13l10-6.5-10-6.5Z"/> })
}
