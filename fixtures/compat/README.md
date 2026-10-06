# Config files from earlier versions

Every `config.toml` here must still load in the current version
(`configs_from_earlier_versions_load` in `crates/mt-ui/src/config.rs`). Each
release adds the file a fresh install of it writes:

```powershell
cargo run -p mt-ui --example default_config -- fixtures/compat/0.2.0.toml
```

The `dev-*` files stand for the development builds before the first release:
`dev-2026-10-04.toml` with the empty lists those builds wrote (`alerts = []`)
and no `[trading]` table, and `dev-2026-10-05.toml` without them, plus one of
every table a user adds by hand. Never edit a file once it is here: a test
failing on one means a change would break that version's settings.
