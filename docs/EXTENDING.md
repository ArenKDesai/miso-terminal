# Extending MISO Terminal

Recipes for the common changes. Each is an addition, not a rewrite. Run
`cargo test --workspace` afterwards; the smoke test picks up new functions on
its own.

## Add a function (a new panel)

1. Create `crates/mt-ui/src/functions/<name>.rs`:

   ```rust
   use egui::Ui;
   use crate::context::PanelCx;
   use crate::function::{Category, FunctionSpec, Panel, Route};
   use crate::widgets;

   pub const SPEC: FunctionSpec = FunctionSpec {
       code: "RDT",
       aliases: &["TRANSFER"],
       name: "Regional transfer",
       category: Category::Grid,
       usage: "RDT",
       description: "North-South regional directional transfer against its limits.",
       takes_node: false,
       open,
   };

   fn open(_args: &[String]) -> Result<Box<dyn Panel>, String> {
       Ok(Box::new(Rdt))
   }

   struct Rdt;

   impl Panel for Rdt {
       fn title(&self) -> String { "RDT".into() }
       fn route(&self) -> Route { Route::code("RDT") }
       fn ui(&mut self, ui: &mut Ui, cx: &mut PanelCx<'_>) {
           let snap = cx.hub.watch(&cx.miso.regional_transfer());
           widgets::title_bar(ui, cx.skin, "Regional transfer", |ui| widgets::freshness(ui, cx.skin, &snap));
           widgets::with_data(ui, cx.skin, &snap, |ui, data| {
               // tables: egui::Grid or egui_extras::TableBuilder (+ widgets::table)
               // charts: widgets::chart::time_plot(..).show(ui, |plot| { .. })
           });
       }
   }
   ```

2. Add `mod <name>;` and `<name>::SPEC` to `functions/mod.rs`.
3. Add a row to the functions table in `README.md`. A test fails if you forget.

Rules of thumb:
- Colours come from `cx.skin` slots (`skin.positive`, `skin.series(i)`,
  `skin.fuel(cat)`), never literals.
- Prices go through `widgets::fmt` and `cx.price_color`.
- Open other functions with `cx.open(Route::new("GP", [node]))`. Change
  app-level state through `cx.send(AppCommand::…)`.
- If `open` gets arguments it cannot use, prompt inside the panel rather than
  failing (a test checks that every function opens with no arguments).
- Panel state that should survive a restart belongs in `route()`.
- For single-instance panels, implement `absorb()` so `CODE <args>` updates
  the open tab rather than opening another.
- Node history, spreads and component maths live in `crate::series`. Node
  search is `widgets::node_picker::NodePicker`, and CSV export is
  `widgets::csv::copy_button`. Reuse them.

## Add a MISO dataset

1. **Record it.** Add the path to `endpoints::paths` (and to `paths::ALL`),
   then run `cargo run -p mt-miso --example capture_fixtures`.
2. **Model it.** Add the types to `mt-core` (`grid.rs` or `prices.rs`): plain
   structs with `Option` where MISO can omit things.
3. **Parse it.** Add a pure `parse_xxx(&str) -> Result<T, FetchError>` in
   `mt-miso/src/parse.rs`. Mirror the JSON loosely (all fields `Option`, numbers
   through `parse_num` or `Num`) so a renamed field degrades one column instead
   of failing the dataset.
4. **Test it.** Add a test to `mt-miso/tests/parsers.rs` against the fixture.
5. **Query it.** For a simple JSON endpoint, add an `api_spec!` line and a
   method on `Miso` in `queries.rs`. For anything stateful (appending, falling
   back between reports), implement `mt_data::Query` directly. `RtIntradayQuery`
   and `RtBestDayQuery` are the examples.

When MISO changes a format: re-run `capture_fixtures`, see which parser test
fails, and fix that parser. Endpoint moves are a config change (`[endpoints]`
in `config.toml`) or a one-line path change.

## Add a non-MISO source

`mt-nws` (weather) is the worked example. Create a crate next to `mt-miso`
(for example `mt-eia` for gas prices) that depends on `mt-core` and `mt-data`. Implement `Query` for its datasets, add a
facade like `Miso`, and hand it to panels through `PanelCx`. The hub, cache,
transports, polite interval and LOG function all apply unchanged. If the source
needs an API key, put it in `AppConfig` (a new `[eia]` table with
`#[serde(default)]`).

## Add or change a theme

- **New built-in:** add `themes/<id>.toml` (see `themes/README.md`) and list it
  in `mt_theme::BUILTIN_SOURCES`. The tests validate contrast and round-trip it.
- **Everforge changed upstream:** run Everforge's `build.py`, then
  `cargo run -p mt-theme --example sync_everforge -- ..\everforge`.
- **New semantic slot:** add it to `Palette` (or a `#[serde(default)]` field so
  existing user themes keep loading), map it in `mt-ui/src/skin.rs`, and set it
  in every built-in and in `sync_everforge`.

## Upgrade egui

`egui`, `eframe`, `egui_extras`, `egui_plot` and `egui_dock` must move together
(see the workspace `Cargo.toml`). Check that egui_dock and egui_plot have
releases for the new egui first. API churn is concentrated in `mt-ui/src/app.rs`,
`skin.rs`, `fonts.rs` and `widgets/`, and the smoke tests exercise all of it.
