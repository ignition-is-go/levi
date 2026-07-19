use leptos::prelude::*;
use wasm_bindgen::JsCast;

use crate::pages;

#[derive(Clone, Copy, PartialEq)]
pub enum Page {
    Overview,
    InFlight,
    Browser,
}

#[component]
pub fn App() -> impl IntoView {
    connect();
    let (page, set_page) = signal(Page::Overview);
    let connected = myko_leptos::use_connection_status();

    let tab = move |p: Page, label: &'static str| {
        view! {
            <button class:active=move || page.get() == p on:click=move |_| set_page.set(p)>
                {label}
            </button>
        }
    };

    view! {
        <header>
            <h1>"levi"</h1>
            <nav>
                {tab(Page::Overview, "overview")}
                {tab(Page::InFlight, "in flight")}
                {tab(Page::Browser, "projects")}
            </nav>
            <span class="conn" class:ok=connected class:bad=move || !connected.get()>
                {move || if connected.get() { "● live" } else { "○ connecting…" }}
            </span>
        </header>
        <main>
            {move || match page.get() {
                Page::Overview => view! { <pages::overview::Overview /> }.into_any(),
                Page::InFlight => view! { <pages::in_flight::InFlight /> }.into_any(),
                Page::Browser => view! { <pages::browser::Browser /> }.into_any(),
            }}
        </main>
    }
}

/// Stash `?token=` into the `levi_token` cookie (the front door checks it on
/// the same-origin WS upgrade), then connect to the hub we were served from.
fn connect() {
    let window = web_sys::window().expect("window");
    let location = window.location();
    if let Ok(search) = location.search()
        && let Some(token) = search
            .trim_start_matches('?')
            .split('&')
            .find_map(|p| p.strip_prefix("token="))
        && !token.is_empty()
        && let Some(doc) = window.document()
        && let Ok(html_doc) = doc.dyn_into::<web_sys::HtmlDocument>()
    {
        let _ = html_doc.set_cookie(&format!(
            "levi_token={token}; path=/; max-age=31536000; samesite=strict"
        ));
    }
    let host = location.host().unwrap_or_else(|_| "localhost:7377".into());
    let scheme = match location.protocol().as_deref() {
        Ok("https:") => "wss",
        _ => "ws",
    };
    myko_leptos::provide_myko(&format!("{scheme}://{host}"));
}
