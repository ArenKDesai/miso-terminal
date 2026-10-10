//! What SET and LOG say about the forecasts kept as issued
//! ([`mt_data::issued`]).

use egui::{Color32, RichText, Ui};
use mt_data::issued::CollectorStatus;

use crate::skin::Skin;

/// The windows SET offers, in days.
pub const WINDOWS: [(u32, &str); 3] = [(365, "1 year"), (730, "2 years"), (1095, "3 years")];

pub fn window_label(days: u32) -> String {
    WINDOWS
        .iter()
        .find(|w| w.0 == days)
        .map_or_else(|| format!("{days} days"), |w| w.1.to_owned())
}

/// "412 files, 3.1 MB".
pub fn stored(s: &CollectorStatus) -> String {
    let mb = s.bytes as f64 / 1_048_576.0;
    format!("{} files, {mb:.1} MB", s.files)
}

/// One line on what the collector is doing.
pub fn activity(s: &CollectorStatus, skin: &Skin) -> (String, Color32) {
    if !s.live {
        return (
            "Not collected in an offline replay.".into(),
            skin.text_muted,
        );
    }
    if !s.keeping {
        return ("Off: nothing new is kept.".into(), skin.text_muted);
    }
    if s.sources.iter().any(|x| x.last_error.is_some()) {
        return (
            "Collecting, with errors (LOG has them).".into(),
            skin.warning,
        );
    }
    let mut text = String::from("Collecting while the terminal runs.");
    if s.backfill_left > 0 {
        text += &format!(
            " Filling {} past days of MISO's load forecast, one every 2 seconds.",
            s.backfill_left
        );
    }
    (text, skin.text)
}

/// A line per source: when it last answered, what it stored, its error.
pub fn sources(ui: &mut Ui, s: &CollectorStatus, skin: &Skin) {
    for src in &s.sources {
        let when = src.last_ok.map_or_else(
            || "not yet asked".to_owned(),
            |t| format!("last answered {} EST", t.format("%b %d %H:%M")),
        );
        let stored = match src.stored {
            0 if src.last_ok.is_some() => ", nothing new".to_owned(),
            0 => String::new(),
            n => format!(", {n} new values"),
        };
        ui.label(RichText::new(format!("{}: {when}{stored}", src.name)).color(skin.text_muted));
        if let Some(e) = &src.last_error {
            ui.label(RichText::new(format!("⚠ {}: {e}", src.name)).color(skin.warning));
        }
    }
}
