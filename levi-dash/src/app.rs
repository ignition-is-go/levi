use leptos::prelude::*;

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

/// Connect to the hub: `?hub=host:port` overrides, otherwise the page's
/// hostname on the default hub port (the dashboard is a standalone CSR app —
/// `trunk serve` in dev — talking straight to levi-hub's /myko).
fn connect() {
    let window = web_sys::window().expect("window");
    let location = window.location();
    let hub = location
        .search()
        .ok()
        .and_then(|search| {
            search
                .trim_start_matches('?')
                .split('&')
                .find_map(|p| p.strip_prefix("hub=").map(str::to_string))
        })
        .filter(|h| !h.is_empty())
        .unwrap_or_else(|| {
            let hostname = location.hostname().unwrap_or_else(|_| "localhost".into());
            format!("{hostname}:7377")
        });
    myko_leptos::provide_myko(&hub);
}
