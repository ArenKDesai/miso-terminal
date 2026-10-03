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
    /// Where the panel was last drawn, for "copy as image".
    #[serde(skip)]
    body_rect: Option<egui::Rect>,
}

impl Tab {
    fn new(id: u64, route: Route) -> Self {
        Self {
            id,
            route,
            panel: None,
            crashed: None,
            body_rect: None,
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

    /// Where the panel is drawn, for "copy as image".
    pub(crate) fn set_body_rect(&mut self, rect: egui::Rect) {
        self.body_rect = Some(rect);
    }

    pub(crate) fn title(&mut self, registry: &Registry) -> String {
        if self.crashed.is_some() {
            return format!("{} ⚠", self.route.code);
        }
        self.panel(registry).title()
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

/// A tab in its own OS window, for a second monitor.
#[derive(Serialize, Deserialize)]
pub struct Popped {
    pub tab: Tab,
    /// Where the window was (outer top-left) and its inner size, in points,
    /// so it reopens in the same place.
    pub pos: Option<[f32; 2]>,
    pub size: Option<[f32; 2]>,
}

/// The OS window showing a popped-out tab.
pub fn popout_viewport(tab: u64) -> egui::ViewportId {
    egui::ViewportId::from_hash_of(("mt-popout", tab))
}

#[derive(Serialize, Deserialize)]
pub struct Workspace {
    pub version: u32,
    pub dock: DockState<Tab>,
    next_id: u64,
    /// The tab filling the window (Ctrl+M), if any. Layouts always reopen unzoomed.
    #[serde(skip)]
    zoomed: Option<u64>,
    /// Tabs popped out into their own windows. Saved with the layout.
    #[serde(default)]
    pub popped: Vec<Popped>,
    /// A popped-out window to bring forward (its route was opened again).
    #[serde(skip)]
    raise: Option<u64>,
}

impl Workspace {
    pub fn default_layout() -> Self {
        let mut ws = Self {
            version: LAYOUT_VERSION,
            dock: DockState::new(Vec::new()),
            next_id: 1,
            zoomed: None,
            popped: Vec::new(),
            raise: None,
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

    /// Move a tab out of the dock into its own OS window.
    pub fn pop_out(&mut self, tab: u64) {
        let Some(path) = self.dock.find_tab_from(|t| t.id == tab) else {
            return;
        };
        if self.zoomed == Some(tab) {
            self.zoomed = None;
        }
        if let Some(tab) = self.dock.remove_tab(path) {
            self.popped.push(Popped {
                tab,
                pos: None,
                size: None,
            });
        }
    }

    /// Put a popped-out tab back in the dock (its window was closed).
    pub fn dock_back(&mut self, tab: u64) {
        let Some(i) = self.popped.iter().position(|p| p.tab.id == tab) else {
            return;
        };
        let tab = self.popped.remove(i).tab;
        self.push_focused(tab);
    }

    /// The popped-out window to bring forward, if any (taken once).
    pub fn take_raise(&mut self) -> Option<u64> {
        self.raise.take()
    }

    /// Add a tab to the focused pane and focus it.
    fn push_focused(&mut self, tab: Tab) {
        let id = tab.id;
        self.dock.push_to_focused_leaf(tab);
        // Focus the new tab, so Ctrl+W / Ctrl+M act on what was just opened.
        if let Some(path) = self.dock.find_tab_from(|t| t.id == id) {
            let _ = self.dock.set_active_tab(path);
            self.dock.set_focused_node_and_surface(path.node_path());
        }
    }

    /// Fill the window with the focused tab, or go back to the layout.
    pub fn toggle_zoom(&mut self) {
        self.zoomed = match self.zoomed {
            Some(_) => None,
            None => self.focused_tab_id(),
        };
    }

    /// The active tab of the focused pane; before anything has been focused
    /// (no click since launch), the first pane's.
    fn focused_tab_id(&mut self) -> Option<u64> {
        if let Some((_, tab)) = self.dock.find_active_focused() {
            return Some(tab.id);
        }
        self.dock
            .iter_leaves()
            .find_map(|(_, leaf)| leaf.tabs.get(leaf.active.0).map(|t| t.id))
    }

    /// Zoom a particular tab (or none).
    pub fn zoom(&mut self, tab: Option<u64>) {
        self.zoomed = tab;
    }

    pub fn is_zoomed(&self) -> bool {
        self.zoomed.is_some()
    }

    /// The zoomed tab, if there is one and it is still open.
    pub fn zoomed_tab(&mut self) -> Option<&mut Tab> {
        let id = self.zoomed?;
        if !self.dock.iter_all_tabs().any(|(_, t)| t.id == id) {
            self.zoomed = None;
            return None;
        }
        self.dock
            .iter_all_tabs_mut()
            .map(|(_, t)| t)
            .find(|t| t.id == id)
    }

    /// Focus a tab showing `route` (or one whose panel absorbs it), else open
    /// it in the focused pane. Leaves a zoomed view, so the result is visible.
    pub fn open(&mut self, route: Route, registry: &Registry) {
        self.zoomed = None;
        // Already in its own window: bring that forward instead.
        if let Some(p) = self.popped.iter().find(|p| p.tab.route == route) {
            self.raise = Some(p.tab.id);
            return;
        }
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
        self.push_focused(tab);
    }

    /// Close the active tab in the focused pane (Ctrl+W).
    pub fn close_focused(&mut self) {
        let Some(node) = self.dock.focused_leaf() else {
            return;
        };
        let Ok(leaf) = self.dock.leaf(node) else {
            return;
        };
        if leaf.is_empty() {
            return;
        }
        let path = egui_dock::TabPath::new(node.surface, node.node, leaf.active);
        self.dock.remove_tab(path);
    }

    /// Activate the next (or previous) tab in the focused pane (Ctrl+Tab).
    pub fn cycle_focused(&mut self, forward: bool) {
        let Some(node) = self.dock.focused_leaf() else {
            return;
        };
        let Ok(leaf) = self.dock.leaf(node) else {
            return;
        };
        let n = leaf.len();
        if n < 2 {
            return;
        }
        let i = leaf.active.0;
        let next = if forward {
            (i + 1) % n
        } else {
            (i + n - 1) % n
        };
        let _ = self.dock.set_active_tab(egui_dock::TabPath::new(
            node.surface,
            node.node,
            next.into(),
        ));
        if self.zoomed.is_some() {
            self.zoomed = self.focused_tab_id();
        }
    }

    /// Every open route, docked or popped out.
    pub fn routes(&self) -> Vec<Route> {
        self.dock
            .iter_all_tabs()
            .map(|(_, t)| &t.route)
            .chain(self.popped.iter().map(|p| &p.tab.route))
            .cloned()
            .collect()
    }

    /// Restore a persisted workspace, discarding incompatible versions.
    pub fn restore(saved: Option<Self>) -> Self {
        match saved {
            Some(ws)
                if ws.version == LAYOUT_VERSION
                    && (ws.dock.iter_all_tabs().next().is_some() || !ws.popped.is_empty()) =>
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
        tab.title(self.cx.registry).into()
    }

    fn on_tab_button(&mut self, tab: &mut Tab, response: &egui::Response) {
        if response.double_clicked() {
            self.cx.send(crate::context::AppCommand::Zoom(Some(tab.id)));
        }
    }

    fn ui(&mut self, ui: &mut Ui, tab: &mut Tab) {
        // Include the tab body's padding so exported images are not cropped flush.
        tab.body_rect = Some(ui.max_rect().expand(self.cx.skin.theme.style.padding));
        draw_tab(ui, self.cx, tab);
    }

    fn context_menu(&mut self, ui: &mut Ui, tab: &mut Tab, _path: egui_dock::NodePath) {
        if ui.button("Zoom (Ctrl+M, double-click)").clicked() {
            self.cx.send(crate::context::AppCommand::Zoom(Some(tab.id)));
            ui.close();
        }
        if ui
            .button("Open in new window")
            .on_hover_text("For a second monitor. Close the window to dock it again.")
            .clicked()
        {
            self.cx.send(crate::context::AppCommand::PopOut(tab.id));
            ui.close();
        }
        let Some(rect) = tab.body_rect else { return };
        for (label, action) in [
            ("Copy panel as image", crate::capture::Action::Copy),
            ("Save panel as PNG", crate::capture::Action::SavePng),
        ] {
            if ui.button(label).clicked() {
                self.cx.send(crate::context::AppCommand::Capture(
                    crate::capture::Request {
                        rect,
                        action,
                        name: tab.route.to_string(),
                    },
                ));
                ui.close();
            }
        }
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

    #[test]
    fn zoom_follows_focus_and_gives_way_to_new_tabs() {
        let registry = Registry::builtin();
        let mut ws = Workspace::default_layout();
        ws.open(Route::code("MAP"), &registry);
        ws.toggle_zoom();
        let zoomed = ws.zoomed_tab().map(|t| t.route.clone());
        assert_eq!(zoomed, Some(Route::code("MAP")), "zooms the focused tab");

        ws.cycle_focused(true);
        let next = ws.zoomed_tab().map(|t| t.route.clone());
        assert!(
            next.is_some() && next != zoomed,
            "Ctrl+Tab moves the zoom along"
        );

        ws.open(Route::code("CAP"), &registry);
        assert!(!ws.is_zoomed(), "opening something shows the layout again");

        ws.toggle_zoom();
        assert!(ws.is_zoomed());
        ws.close_focused();
        assert!(
            ws.zoomed_tab().is_none(),
            "closing the zoomed tab ends the zoom"
        );
        assert!(!ws.is_zoomed());

        ws.toggle_zoom();
        ws.toggle_zoom();
        assert!(!ws.is_zoomed(), "toggles back");
    }

    #[test]
    fn tabs_pop_out_and_dock_back() {
        let registry = Registry::builtin();
        let mut ws = Workspace::default_layout();
        ws.open(Route::code("MAP"), &registry);
        let map = ws.dock.find_active_focused().map(|(_, t)| t.id).unwrap();
        let before = ws.routes().len();
        ws.pop_out(map);
        assert_eq!(ws.popped.len(), 1);
        assert!(
            ws.dock.find_tab_from(|t| t.id == map).is_none(),
            "left the dock"
        );
        assert_eq!(ws.routes().len(), before, "still open, in its own window");

        // Opening it again raises its window instead of a duplicate.
        ws.open(Route::code("MAP"), &registry);
        assert_eq!(ws.take_raise(), Some(map));
        assert_eq!(ws.routes().len(), before);

        // Survives a save and restore (window geometry too).
        ws.popped[0].pos = Some([40.0, 50.0]);
        // Through eframe's own persistence, as the app saves it.
        #[derive(Default)]
        struct Memory(std::collections::HashMap<String, String>);
        impl eframe::Storage for Memory {
            fn get_string(&self, key: &str) -> Option<String> {
                self.0.get(key).cloned()
            }
            fn set_string(&mut self, key: &str, value: String) {
                self.0.insert(key.to_owned(), value);
            }
            fn remove_string(&mut self, key: &str) {
                self.0.remove(key);
            }
            fn flush(&mut self) {}
        }
        let mut storage = Memory::default();
        eframe::set_value(&mut storage, "workspace", &ws);
        let saved = eframe::get_value::<Workspace>(&storage, "workspace");
        assert!(saved.is_some(), "the layout round-trips");
        let mut ws = Workspace::restore(saved);
        assert_eq!(ws.popped.len(), 1);
        assert_eq!(ws.popped[0].pos, Some([40.0, 50.0]));

        ws.dock_back(map);
        assert!(ws.popped.is_empty());
        assert!(
            ws.dock.find_tab_from(|t| t.id == map).is_some(),
            "back in the dock"
        );
    }
}
