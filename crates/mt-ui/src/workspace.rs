//! The tiled, tabbed workspace (egui_dock) and its persistence.
//!
//! Tabs persist as [`Route`]s only; panels are rebuilt through the registry
//! on load. A route whose function no longer exists (renamed or removed in a
//! later version) opens a placeholder instead of breaking the saved layout.
//!
//! A panel that panics is contained to its tab: the panic is caught, logged,
//! and the tab offers to reload the panel instead of the whole app exiting.

use std::panic::{AssertUnwindSafe, catch_unwind};

use egui::{RichText, Ui, WidgetText};
use egui_dock::{DockState, NodeIndex, TabViewer};
use serde::{Deserialize, Serialize};

use crate::context::PanelCx;
use crate::function::{Panel, Registry, Route};

/// Bump when the default layout or persisted format changes incompatibly;
/// saved layouts with another version are replaced by the default.
pub const LAYOUT_VERSION: u32 = 1;

#[derive(Serialize, Deserialize)]
pub struct Tab {
    pub id: u64,
    pub route: Route,
    #[serde(skip)]
    panel: Option<Box<dyn Panel>>,
    /// Set when the panel panicked; cleared by "Reload".
    #[serde(skip)]
    crashed: Option<String>,
}

impl Tab {
    fn new(id: u64, route: Route) -> Self {
        Self {
            id,
            route,
            panel: None,
            crashed: None,
        }
    }

    fn panel(&mut self, registry: &Registry) -> &mut dyn Panel {
        let route = &self.route;
        self.panel
            .get_or_insert_with(|| {
                registry.open(route).unwrap_or_else(|error| {
                    Box::new(Missing {
                        route: route.clone(),
                        error,
                    })
                })
            })
            .as_mut()
    }
}

/// Shown for a route that cannot be opened.
struct Missing {
    route: Route,
    error: String,
}

impl Panel for Missing {
    fn title(&self) -> String {
        format!("{} ?", self.route.code)
    }

    fn route(&self) -> Route {
        self.route.clone()
    }

    fn ui(&mut self, ui: &mut Ui, cx: &mut PanelCx<'_>) {
        ui.label(
            RichText::new(format!("Cannot open {}: {}", self.route, self.error))
                .color(cx.skin.negative),
        );
        ui.label("Type HELP for the list of functions, or close this tab.");
    }
}

#[derive(Serialize, Deserialize)]
pub struct Workspace {
    pub version: u32,
    pub dock: DockState<Tab>,
    next_id: u64,
}

impl Workspace {
    pub fn default_layout() -> Self {
        let mut ws = Self {
            version: LAYOUT_VERSION,
            dock: DockState::new(Vec::new()),
            next_id: 1,
        };
        let home = ws.tab(Route::code("HOME"));
        let lmp = ws.tab(Route::code("LMP"));
        let load = ws.tab(Route::code("LOAD"));
        let fuel = ws.tab(Route::code("FUEL"));
        let cons = ws.tab(Route::code("CONS"));
        let help = ws.tab(Route::code("HELP"));
        let wl = ws.tab(Route::code("WL"));
        ws.dock = DockState::new(vec![home, help]);
        let surface = ws.dock.main_surface_mut();
        let [_, right] = surface.split_right(NodeIndex::root(), 0.52, vec![lmp, wl]);
        surface.split_below(right, 0.55, vec![load, fuel, cons]);
        ws
    }

    fn tab(&mut self, route: Route) -> Tab {
        let id = self.next_id;
        self.next_id += 1;
        Tab::new(id, route)
    }

    /// Focus a tab showing `route` (or one whose panel absorbs it), else open
    /// it in the focused pane.
    pub fn open(&mut self, route: Route, registry: &Registry) {
        let code = registry
            .find(&route.code)
            .map_or(route.code.as_str(), |s| s.code)
            .to_owned();
        let mut target = self.dock.find_tab_from(|t| t.route == route);
        if target.is_none() {
            for (path, tab) in self.dock.iter_all_tabs_mut() {
                if tab.route.code == code
                    && tab.crashed.is_none()
                    && tab.panel(registry).absorb(&route.args)
                {
                    target = Some(path);
                    break;
                }
            }
        }
        if let Some(path) = target {
            let _ = self.dock.set_active_tab(path);
            self.dock.set_focused_node_and_surface(path.node_path());
            return;
        }
        let tab = self.tab(route);
        self.dock.push_to_focused_leaf(tab);
    }

    pub fn routes(&self) -> Vec<Route> {
        self.dock
            .iter_all_tabs()
            .map(|(_, t)| t.route.clone())
            .collect()
    }

    /// Restore a persisted workspace, discarding incompatible versions.
    pub fn restore(saved: Option<Self>) -> Self {
        match saved {
            Some(ws)
                if ws.version == LAYOUT_VERSION && ws.dock.iter_all_tabs().next().is_some() =>
            {
                ws
            }
            _ => Self::default_layout(),
        }
    }
}

/// Draw one tab's panel, containing any panic to the tab.
pub(crate) fn draw_tab(ui: &mut Ui, cx: &mut PanelCx<'_>, tab: &mut Tab) {
    if let Some(error) = &tab.crashed {
        ui.label(RichText::new(format!("{} stopped: {error}", tab.route)).color(cx.skin.negative));
        ui.label(
            RichText::new(
                "The rest of the terminal is unaffected. The details are in the log file.",
            )
            .color(cx.skin.text_muted),
        );
        if ui.button("Reload panel").clicked() {
            tab.crashed = None;
        }
        return;
    }
    let registry = cx.registry;
    let panel = tab.panel(registry);
    match catch_unwind(AssertUnwindSafe(|| panel.ui(ui, cx))) {
        // Keep the persisted route in step with the panel's state (node, days, ...).
        Ok(()) => tab.route = panel.route(),
        Err(payload) => {
            let message = payload
                .downcast_ref::<&str>()
                .map(|s| (*s).to_owned())
                .or_else(|| payload.downcast_ref::<String>().cloned())
                .unwrap_or_else(|| "panic".to_owned());
            tracing::error!("panel {} panicked: {message}", tab.route);
            // Drop the panel's state; "Reload" rebuilds it from the route.
            tab.panel = None;
            tab.crashed = Some(message);
        }
    }
}

/// Bridges egui_dock to panels.
pub struct Viewer<'a, 'b> {
    pub cx: &'a mut PanelCx<'b>,
}

impl TabViewer for Viewer<'_, '_> {
    type Tab = Tab;

    fn id(&mut self, tab: &mut Tab) -> egui::Id {
        egui::Id::new(("mt-tab", tab.id))
    }

    fn title(&mut self, tab: &mut Tab) -> WidgetText {
        if tab.crashed.is_some() {
            return format!("{} ⚠", tab.route.code).into();
        }
        let registry = self.cx.registry;
        tab.panel(registry).title().into()
    }

    fn ui(&mut self, ui: &mut Ui, tab: &mut Tab) {
        draw_tab(ui, self.cx, tab);
    }

    fn scroll_bars(&self, _tab: &Tab) -> [bool; 2] {
        // Panels manage their own scrolling (tables and charts fill the tab).
        [false, false]
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;

    /// A panel that panics on demand, for crash-isolation tests.
    pub(crate) struct Bomb;

    impl Panel for Bomb {
        fn title(&self) -> String {
            "BOMB".into()
        }

        fn route(&self) -> Route {
            Route::code("LMP")
        }

        fn ui(&mut self, _ui: &mut Ui, _cx: &mut PanelCx<'_>) {
            panic!("boom in a panel");
        }
    }

    pub(crate) fn bomb_tab() -> Tab {
        let mut tab = Tab::new(99, Route::code("LMP"));
        tab.panel = Some(Box::new(Bomb));
        tab
    }

    pub(crate) fn crashed(tab: &Tab) -> Option<&str> {
        tab.crashed.as_deref()
    }
}
