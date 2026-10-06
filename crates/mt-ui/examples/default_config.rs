//! Write the `config.toml` a fresh install writes. Each release adds its own
//! to `fixtures/compat/`, and a test loads every file there, so a later
//! change cannot quietly break an older user's settings.
//!
//! ```text
//! cargo run -p mt-ui --example default_config -- fixtures/compat/0.2.0.toml
//! ```

use std::path::PathBuf;

fn main() -> std::io::Result<()> {
    let Some(path) = std::env::args_os().nth(1).map(PathBuf::from) else {
        eprintln!("usage: default_config <file>");
        std::process::exit(2);
    };
    mt_ui::AppConfig::default().save(&path)?;
    println!("wrote {}", path.display());
    Ok(())
}
