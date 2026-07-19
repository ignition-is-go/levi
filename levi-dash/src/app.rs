//! App shell: mullion splittable panes with an activity bar (Overview /
//! In Flight / Projects), pulse-leptos-ui design system, myko live data.

use leptos::prelude::*;
use mullion::{
    ActivityDef, ActivityIcon, ActivityId, Category, CategoryId, MullionContext, MullionPaneTree,
    MullionProvider, MullionTheme, PaneId, PaneNode,
};
use pulse_leptos_ui::{BaseStyle, Styleable, tokens, use_style};
use serde::{Deserialize, Serialize};

use crate::pages;

/// Per-pane state. Views hold their own component-local signals; panes are
/// keyed by id so that state survives splits and moves.
#[derive(Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct PaneState {}

const ICON_OVERVIEW: &str = r##"<svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2"><rect x="3" y="3" width="7" height="9"/><rect x="14" y="3" width="7" height="5"/><rect x="14" y="12" width="7" height="9"/><rect x="3" y="16" width="7" height="5"/></svg>"##;
const ICON_FLIGHT: &str = r##"<svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2"><circle cx="12" cy="12" r="10"/><polyline points="12 6 12 12 16 14"/></svg>"##;
const ICON_BROWSER: &str = r##"<svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2"><line x1="8" y1="6" x2="21" y2="6"/><line x1="8" y1="12" x2="21" y2="12"/><line x1="8" y1="18" x2="21" y2="18"/><line x1="3" y1="6" x2="3.01" y2="6"/><line x1="3" y1="12" x2="3.01" y2="12"/><line x1="3" y1="18" x2="3.01" y2="18"/></svg>"##;
const ICON_APP: &str = r##"<svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2"><path d="M12 2 2 7l10 5 10-5-10-5z"/><path d="m2 17 10 5 10-5"/><path d="m2 12 10 5 10-5"/></svg>"##;

fn categories() -> Vec<Category<PaneState>> {
    vec![Category {
        id: CategoryId::new("levi"),
        name: "levi".into(),
        order: 0,
        icon: ActivityIcon::Svg(ICON_APP.into()),
        color: tokens::PRIMARY.into(),
        activities: vec![
            ActivityDef {
                id: ActivityId::new("overview"),
                name: "Overview".into(),
                icon: ActivityIcon::Svg(ICON_OVERVIEW.into()),
                filter: |_| true,
                render: |_pid, _data| view! { <pages::overview::Overview /> }.into_any(),
            },
            ActivityDef {
                id: ActivityId::new("in-flight"),
                name: "In flight".into(),
                icon: ActivityIcon::Svg(ICON_FLIGHT.into()),
                filter: |_| true,
                render: |_pid, _data| view! { <pages::in_flight::InFlight /> }.into_any(),
            },
            ActivityDef {
                id: ActivityId::new("browser"),
                name: "Projects".into(),
                icon: ActivityIcon::Svg(ICON_BROWSER.into()),
                filter: |_| true,
                render: |_pid, _data| view! { <pages::browser::Browser /> }.into_any(),
            },
        ],
    }]
}

#[component]
pub fn App() -> impl IntoView {
    connect();

    // Align mullion's palette with pulse's design tokens so panes and
    // content read as one system.
    provide_context(MullionTheme {
        bg: tokens::BASE_100.into(),
        surface: tokens::BASE_100.into(),
        border: tokens::BORDER.into(),
        accent: tokens::BASE_200.into(),
        highlight: tokens::BASE_300.into(),
        text: tokens::TEXT_PRIMARY.into(),
        text_muted: tokens::TEXT_TERTIARY.into(),
        ..Default::default()
    });

    let initial_tree = PaneNode::leaf_with_activity(
        PaneId::new("main"),
        ActivityId::new("overview"),
        PaneState::default(),
    );

    let base_css = use_style::<BaseStyle>().css();
    let on_event = |_event: mullion::PaneEvent<PaneState>| {};

    view! {
        <style>{base_css}</style>
        <MullionProvider
            initial_tree=initial_tree
            categories=categories()
            on_event=on_event
            app_icon=ActivityIcon::Svg(ICON_APP.into())
        >
            <Shell />
        </MullionProvider>
    }
}

#[component]
fn Shell() -> impl IntoView {
    let ctx = use_context::<MullionContext<PaneState>>().expect("mullion context");
    let connected = myko_leptos::use_connection_status();
    view! {
        <div style="display:flex;flex-direction:column;height:100%;">
            <div style="flex:1;min-height:0;">
                <MullionPaneTree ctx=ctx />
            </div>
            <div style=format!(
                "display:flex;justify-content:flex-end;padding:2px 10px;font-size:11px;\
                 border-top:1px solid {};color:{};",
                tokens::BORDER,
                tokens::TEXT_TERTIARY
            )>
                {move || {
                    if connected.get() {
                        view! { <pulse_leptos_ui::Status variant=pulse_leptos_ui::StatusVariant::Success>"live"</pulse_leptos_ui::Status> }
                            .into_any()
                    } else {
                        view! { <pulse_leptos_ui::Status variant=pulse_leptos_ui::StatusVariant::Warning>"connecting"</pulse_leptos_ui::Status> }
                            .into_any()
                    }
                }}
            </div>
        </div>
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
