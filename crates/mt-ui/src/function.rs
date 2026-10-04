//! Functions: the Bloomberg-style mnemonics (`LMP`, `GP MINN.HUB`, `FUEL`) and
//! the panels they open.
//!
//! A function is a [`FunctionSpec`] in `functions/mod.rs` plus a [`Panel`]
//! implementation. Open panels are identified by a [`Route`] (code + args),
//! which is what the layout persists, so a panel never needs to be serializable
//! itself. See `docs/ADDING_A_FUNCTION.md`.

use serde::{Deserialize, Serialize};

use crate::context::PanelCx;

/// A function code plus arguments, e.g. `GP MINN.HUB 14`.
#[derive(Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct Route {
    pub code: String,
    #[serde(default)]
    pub args: Vec<String>,
}

impl Route {
    pub fn new(code: &str, args: impl IntoIterator<Item = impl Into<String>>) -> Self {
        Self {
            code: code.to_ascii_uppercase(),
            args: args.into_iter().map(Into::into).collect(),
        }
    }

    pub fn code(code: &str) -> Self {
        Self::new(code, Vec::<String>::new())
    }

    pub fn arg(&self, i: usize) -> Option<&str> {
        self.args.get(i).map(String::as_str)
    }
}

impl std::fmt::Display for Route {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.code)?;
        for a in &self.args {
            write!(f, " {a}")?;
        }
        Ok(())
    }
}

/// An open panel. Implementations keep their own UI state (filters, sort
/// order, zoom) and fetch data through `cx.hub` every frame.
pub trait Panel {
    /// Tab title.
    fn title(&self) -> String;

    /// Draw the panel. Called every frame while visible; must not block.
    fn ui(&mut self, ui: &mut egui::Ui, cx: &mut PanelCx<'_>);

    /// The route that reopens this panel in its current state. Persisted with
    /// the layout, so a panel that changes node updates its route.
    fn route(&self) -> Route;

    /// Offered a route with this panel's code that is not an exact match
    /// (e.g. `WL ALTE.ALTE` while a watchlist is open). Return `true` to take
    /// the arguments and have the workspace focus this tab instead of opening
    /// a new one. Most panels want separate tabs per argument set: the default.
    fn absorb(&mut self, _args: &[String]) -> bool {
        false
    }
}

/// Grouping for menus and HELP.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum Category {
    Overview,
    Prices,
    Grid,
    System,
}

impl Category {
    pub const ALL: [Self; 4] = [Self::Overview, Self::Prices, Self::Grid, Self::System];

    pub fn label(self) -> &'static str {
        match self {
            Self::Overview => "Overview",
            Self::Prices => "Prices",
            Self::Grid => "Grid conditions",
            Self::System => "System",
        }
    }
}

/// Builds a panel from route arguments, or explains why it can't.
pub type OpenFn = fn(&[String]) -> Result<Box<dyn Panel>, String>;

pub struct FunctionSpec {
    /// The mnemonic, upper case.
    pub code: &'static str,
    /// Other accepted spellings.
    pub aliases: &'static [&'static str],
    pub name: &'static str,
    pub category: Category,
    /// Argument synopsis, e.g. `GP <node> [days]`.
    pub usage: &'static str,
    pub description: &'static str,
    /// Whether the first argument is a pricing node (enables node completion).
    pub takes_node: bool,
    /// Whether arguments may be securities (`XLU US`) or options (OCC
    /// symbols). The command line refuses them for functions that cannot.
    pub takes_security: bool,
    pub open: OpenFn,
}

impl FunctionSpec {
    pub fn matches(&self, code: &str) -> bool {
        self.code.eq_ignore_ascii_case(code)
            || self.aliases.iter().any(|a| a.eq_ignore_ascii_case(code))
    }
}

pub struct Registry {
    specs: Vec<FunctionSpec>,
}

impl Registry {
    pub fn new(specs: Vec<FunctionSpec>) -> Self {
        Self { specs }
    }

    pub fn builtin() -> Self {
        Self::new(crate::functions::all())
    }

    pub fn specs(&self) -> &[FunctionSpec] {
        &self.specs
    }

    pub fn find(&self, code: &str) -> Option<&FunctionSpec> {
        self.specs.iter().find(|s| s.matches(code))
    }

    /// Instantiate the panel for `route`, normalising aliases to the main code.
    pub fn open(&self, route: &Route) -> Result<Box<dyn Panel>, String> {
        let spec = self
            .find(&route.code)
            .ok_or_else(|| format!("unknown function {}", route.code))?;
        (spec.open)(&route.args)
    }
}
