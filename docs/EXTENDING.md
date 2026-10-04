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

Daily reports that MISO only publishes as legacy `.xls` (the binding-constraint
histories) go through `calamine`: see `parse_constraint_history`, which finds
columns by normalised header name, and `ConstraintHistoryQuery`, which maps a
market day to the file's date. Fetch binary bodies with `ctx.get` /
`ctx.get_immutable` (bytes) rather than the `_text` variants.

When MISO changes a format: re-run `capture_fixtures`, see which parser test
fails, and fix that parser. Endpoint moves are a config change (`[endpoints]`
in `config.toml`) or a one-line path change.

## Add a non-MISO source

`mt-nws` (weather, JSON) and `mt-eia` (Henry Hub gas, a spreadsheet) are the
worked examples. Create a crate next to `mt-miso` that depends on `mt-core` and
`mt-data`; implement `Query` for its datasets, add a facade like `Miso`, and
hand it to panels through `PanelCx` (a field plus one line where the app and the
smoke harness build it). Give it a fixture, a parser test that honours
`MT_FIXTURES`, and a recorder example with a name unique in the workspace
(`capture_weather`, `capture_gas`); then add it to the Linux CI job and the
weekly drift workflow. The hub, cache, transports, polite interval and LOG
function all apply unchanged.

If the source needs an API key, never put it in `AppConfig`: give the key a
name (`<source>/<account>/<key>`, e.g. `alpaca/paper/key-id`), add it to
`CREDENTIALS` in `functions/settings.rs` so SET can store it in Windows
Credential Manager, and read it in `fetch()` with `ctx.secret(name)?` (a missing
key is a `FetchError::Auth` naming it). Send it with
`Request::get(url).secret_header("Header-Name", key)` and `ctx.get(&req)`: the
event log shows the request with the value as `***`. If the API limits
requests per minute, give it a `Budget` in `FetchCtxOptions` (built in
`main.rs`), shared by all of its hosts; LOG then shows how much is used.
Fixtures never contain keys or account numbers.

## Add a streaming source

For a WebSocket feed (quotes, trade updates, news), implement
`mt_data::Stream` once per endpoint: `key`, the handshake `request` (URL and
any `secret_header`s), `hello` (a login message, if the protocol has one; it is
never logged), `waits_for_ready` if subscriptions must wait for the login's
acknowledgement, `subscribe`/`unsubscribe` messages for a list of topics, and
`apply`, which folds one frame into the stream's `State` and says whether it
changed anything, accepted the login (`Applied::Ready`), or failed
(`FetchError::Parse` skips the frame, `FetchError::Auth` stops reconnecting).
`apply` is pure, so test it against recorded frames.

Panels call `cx.hub.watch_stream(&stream, &["quotes:XLU"])` every frame and
render the `Snapshot` like any other. The hub shares one connection among them,
subscribes the union of their topics, reconnects and resubscribes, and lists
the stream in LOG. For offline mode and tests, record a session as
`fixtures/<host>/<path>.jsonl` (one server frame per line, keys removed);
`FixtureTransport` replays it.

Data the app keeps itself (the five-minute archive, the long-history archive)
lives in the disk cache under `local://` keys, read and written with
`FetchCtx::local_get` / `local_put`; exempt its directory from the size cap in
`main.rs` and give it its own retention if it grows.

## Add or change a theme

- **New gallery theme** (downloadable, not compiled in): add
  `themes/gallery/<id>.toml` and a 960 × 576 preview at
  `docs/screenshots/themes/<id>.webp` (render it with `--theme-file`; see
  `themes/README.md`), then `uv run tools/build_docs.py site --update-fixture`
  to refresh the recorded gallery index. The tests validate contrast and
  round-trip it and check the index; the docs site lists it with a download
  button and THEME offers it to install.
- **New built-in:** add `themes/<id>.toml` (see `themes/README.md`) and list it
  in `mt_theme::BUILTIN_SOURCES`. The tests validate contrast and round-trip it,
  and the visual regression test adds its layout (accept the new snapshot).
- **Everforge changed upstream:** run Everforge's `build.py`, then
  `cargo run -p mt-theme --example sync_everforge -- ..\everforge`.
- **New semantic slot:** add it to `Palette` (or a `#[serde(default)]` field so
  existing user themes keep loading), map it in `mt-ui/src/skin.rs`, and set it
  in every built-in, every gallery theme and in `sync_everforge`.

## Upgrade egui

`egui`, `eframe`, `egui_extras`, `egui_plot` and `egui_dock` must move together
(see the workspace `Cargo.toml`). Check that egui_dock and egui_plot have
releases for the new egui first. API churn is concentrated in `mt-ui/src/app.rs`,
`skin.rs`, `fonts.rs` and `widgets/`, and the smoke tests exercise all of it.
