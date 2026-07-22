//! App shell: mullion splittable panes with an activity bar (Overview /
//! In Flight / Projects), pulse-leptos-ui design system, myko live data.

use leptos::prelude::*;
use mullion::{
    ActivityDef, ActivityIcon, ActivityId, Category, CategoryId, MullionContext, MullionPaneTree,
    MullionProvider, MullionTheme, PaneEvent, PaneId, PaneNode, SplitDirection, Workspace,
    WorkspaceId, WorkspaceManager, WorkspaceSwitcher,
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
const ICON_GRAPH: &str = r##"<svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2"><circle cx="5" cy="6" r="3"/><circle cx="19" cy="6" r="3"/><circle cx="12" cy="18" r="3"/><line x1="8" y1="6" x2="16" y2="6"/><line x1="6.5" y1="8.5" x2="10.5" y2="15.5"/><line x1="17.5" y1="8.5" x2="13.5" y2="15.5"/></svg>"##;

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
            ActivityDef {
                id: ActivityId::new("issues"),
                name: "Blocking graph".into(),
                icon: ActivityIcon::Svg(ICON_GRAPH.into()),
                filter: |_| true,
                render: |_pid, _data| view! { <pages::issues::Issues /> }.into_any(),
            },
        ],
    }]
}

// Bumped to v2 when the default layout was redefined (two columns: overview
// / projects / claims stacked left, graph right). The bump makes the new
// default reach anyone with a stored v1 layout — it resets saved workspaces,
// which is intended when the default itself changes.
const WORKSPACES_KEY: &str = "levi.workspaces.v2";
const LEGACY_LAYOUT_KEY: &str = "levi.layout";

fn leaf(id: &str, activity: &str) -> Box<PaneNode<PaneState>> {
    Box::new(PaneNode::leaf_with_activity(
        PaneId::new(id),
        ActivityId::new(activity),
        PaneState::default(),
    ))
}

/// "Main": two columns. Left column stacks overview / projects / claims
/// top-to-bottom; the blocking graph takes the wider right column.
fn main_tree() -> PaneNode<PaneState> {
    PaneNode::Split {
        direction: SplitDirection::Horizontal,
        ratio: 0.42,
        first: Box::new(PaneNode::Split {
            direction: SplitDirection::Vertical,
            ratio: 0.34,
            first: leaf("main-overview", "overview"),
            second: Box::new(PaneNode::Split {
                direction: SplitDirection::Vertical,
                ratio: 0.5,
                first: leaf("main-browser", "browser"),
                second: leaf("main-flight", "in-flight"),
            }),
        }),
        second: leaf("main-graph", "issues"),
    }
}

fn default_workspaces() -> Vec<Workspace<PaneState>> {
    vec![
        Workspace {
            id: WorkspaceId("main".into()),
            name: "Main".into(),
            tree: main_tree(),
        },
        Workspace {
            id: WorkspaceId("focus".into()),
            name: "Focus".into(),
            tree: *leaf("focus-browser", "browser"),
        },
        Workspace {
            id: WorkspaceId("ops".into()),
            name: "Ops".into(),
            tree: PaneNode::Split {
                direction: SplitDirection::Horizontal,
                ratio: 0.5,
                first: leaf("ops-overview", "overview"),
                second: leaf("ops-flight", "in-flight"),
            },
        },
    ]
}

#[derive(Serialize, Deserialize)]
struct StoredWorkspaces {
    active: String,
    workspaces: Vec<Workspace<PaneState>>,
}

fn local_storage() -> Option<web_sys::Storage> {
    web_sys::window()?.local_storage().ok()?
}

fn stored_workspaces() -> Option<StoredWorkspaces> {
    let storage = local_storage()?;
    if let Some(json) = storage.get_item(WORKSPACES_KEY).ok()? {
        return serde_json::from_str(&json).ok();
    }
    // Migrate the pre-workspaces single layout into "Main".
    let legacy: PaneNode<PaneState> =
        serde_json::from_str(&storage.get_item(LEGACY_LAYOUT_KEY).ok()??).ok()?;
    let mut workspaces = default_workspaces();
    workspaces[0].tree = legacy;
    Some(StoredWorkspaces {
        active: "main".into(),
        workspaces,
    })
}

fn persist_workspaces(manager: &WorkspaceManager<PaneState>) {
    let stored = StoredWorkspaces {
        active: manager.active_id().0,
        workspaces: manager.list(),
    };
    if let (Some(storage), Ok(json)) = (local_storage(), serde_json::to_string(&stored)) {
        let _ = storage.set_item(WORKSPACES_KEY, &json);
    }
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

    // Named workspaces: last-used set wins; sensible defaults on first
    // visit (with a migration for the pre-workspaces single layout).
    let stored = stored_workspaces().unwrap_or_else(|| StoredWorkspaces {
        active: "main".into(),
        workspaces: default_workspaces(),
    });
    let active = WorkspaceId(stored.active.clone());
    let initial_tree = stored
        .workspaces
        .iter()
        .find(|w| w.id == active)
        .or_else(|| stored.workspaces.first())
        .map(|w| w.tree.clone())
        .unwrap_or_else(main_tree);
    let manager = WorkspaceManager::new(stored.workspaces, active);

    let base_css = use_style::<BaseStyle>().css();
    let event_manager = manager.clone();
    let on_event = move |event: PaneEvent<PaneState>| {
        // Every mutation lands in the active workspace and persists the set.
        if let PaneEvent::TreeChanged { tree } = event {
            event_manager.update_tree(&event_manager.active_id(), tree);
            persist_workspaces(&event_manager);
        }
    };

    view! {
        <style>{base_css}</style>
        // Layout-only utilities (no color/type opinions — those stay pulse's):
        // one-line truncation and a subtle hover for clickable rows.
        <style>
            ".trunc{white-space:nowrap;overflow:hidden;text-overflow:ellipsis;min-width:0;}\
             .levi-row{display:flex;align-items:center;gap:0.5rem;min-width:0;}\
             .levi-rowlink{cursor:pointer;border-radius:0.25rem;}\
             .levi-rowlink:hover{background:oklch(1 0 0 / 0.04);}\
             .levi-fill{flex:1;min-height:0;display:flex;flex-direction:column;margin-bottom:0;}\
             .levi-fill .pane-content{flex:1;min-height:0;overflow-y:auto;}\
             .levi-select-wide>.dropdown__trigger{width:12rem;}\
             .levi-select-wide .dropdown__value{flex:1;text-align:left;}"
        </style>
        <MullionProvider
            initial_tree=initial_tree
            categories=categories()
            on_event=on_event
            app_icon=ActivityIcon::Svg(ICON_APP.into())
        >
            <Shell manager=manager />
        </MullionProvider>
    }
}

#[component]
fn Shell(manager: WorkspaceManager<PaneState>) -> impl IntoView {
    let ctx = use_context::<MullionContext<PaneState>>().expect("mullion context");
    let connected = myko_leptos::use_connection_status();
    // Persist on workspace switches too (the tree may not change).
    {
        let manager = manager.clone();
        let active = manager.active_signal();
        Effect::new(move || {
            let _ = active.get();
            persist_workspaces(&manager);
        });
    }
    view! {
        <div style="display:flex;flex-direction:column;height:100%;">
            <div style="flex:1;min-height:0;">
                <MullionPaneTree ctx=ctx.clone() />
            </div>
            <div style=format!(
                "display:flex;justify-content:space-between;align-items:center;\
                 padding:2px 10px;font-size:11px;border-top:1px solid {};color:{};",
                tokens::BORDER,
                tokens::TEXT_TERTIARY
            )>
                <WorkspaceSwitcher manager=manager ctx=ctx />
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
    // Secure pages must use wss:// — browsers block mixed-content ws://.
    let hub = if hub.contains("://") {
        hub
    } else {
        let scheme = match location.protocol().as_deref() {
            Ok("https:") => "wss",
            _ => "ws",
        };
        format!("{scheme}://{hub}")
    };
    myko_leptos::provide_myko(&hub);
}
