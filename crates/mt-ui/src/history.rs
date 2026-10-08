//! The price history as SET, LOG and the status bar describe it: the window's
//! choices, what the store holds, and what the backfill is doing.

use std::time::{Duration, Instant};

use chrono::NaiveDate;
use egui::Color32;
use mt_miso::history::{DOWNLOAD_BYTES_PER_REPORT, Phase, STORED_BYTES_PER_REPORT};
use mt_miso::{BackfillStatus, HistoryConfig};

use crate::skin::Skin;

/// SET's choices for how far back to keep, in days (0: everything).
pub const WINDOWS: &[(u32, &str)] = &[
    (92, "3 months"),
    (183, "6 months"),
    (366, "1 year"),
    (731, "2 years"),
    (0, "Everything since 2023-01-01"),
];

/// A window as SET names it; a hand-written one by its days.
pub fn window_label(keep_days: u32) -> String {
    WINDOWS
        .iter()
        .find(|(d, _)| *d == keep_days)
        .map_or_else(|| format!("{keep_days} days"), |(_, l)| (*l).to_owned())
}

/// Reports in the window on `today`: DA through today, RT through yesterday.
pub fn reports_in(config: &HistoryConfig, today: NaiveDate) -> u64 {
    let days = (today - config.first_day(today)).num_days() + 1;
    u64::try_from(days * 2 - 1).unwrap_or(0)
}

/// What a window takes on disk, and what filling it downloads, roughly.
pub fn estimate(config: &HistoryConfig, today: NaiveDate) -> String {
    let n = reports_in(config, today);
    format!(
        "about {} on disk; filling it downloads about {}",
        size(n * STORED_BYTES_PER_REPORT),
        size(n * DOWNLOAD_BYTES_PER_REPORT)
    )
}

/// `31 MB`, `1.4 GB`.
pub fn size(bytes: u64) -> String {
    let mb = bytes as f64 / 1_048_576.0;
    if mb >= 1024.0 {
        format!("{:.1} GB", mb / 1024.0)
    } else if mb >= 10.0 {
        format!("{mb:.0} MB")
    } else {
        format!("{mb:.1} MB")
    }
}

/// `about 4 min`, `about 1 h 20 min`.
fn duration(d: Duration) -> String {
    let mins = d.as_secs().div_ceil(60);
    match mins {
        0 | 1 => "under a minute".into(),
        2..=59 => format!("about {mins} min"),
        _ => format!("about {} h {} min", mins / 60, mins % 60),
    }
}

/// What the store holds of the window.
pub fn stored(s: &BackfillStatus) -> String {
    let c = &s.coverage;
    let mut text = format!(
        "{} of {} DA days and {} of {} RT days",
        c.da_stored, c.da_days, c.rt_stored, c.rt_days
    );
    if c.rt_prelim > 0 {
        text.push_str(&format!(" ({} preliminary)", c.rt_prelim));
    }
    if let Some(first) = s.first_day {
        text.push_str(&format!(" since {first}"));
    }
    text.push_str(&format!(", {}", size(c.bytes)));
    text
}

/// What the backfill is doing, and the colour to say it in.
pub fn activity(s: &BackfillStatus, skin: &Skin) -> (String, Color32) {
    let missing = s.coverage.missing();
    match &s.phase {
        Phase::Starting => ("Starting…".into(), skin.text_muted),
        Phase::Scanning => ("Looking at what is stored…".into(), skin.text_muted),
        Phase::Unavailable => (
            "Offline replay: nothing is downloaded or kept.".into(),
            skin.text_muted,
        ),
        Phase::Off if missing == 0 => {
            ("Every day of the window is stored.".into(), skin.text_muted)
        }
        Phase::Off => (
            format!("{missing} reports missing. Nothing downloads until you click Download now."),
            skin.text_muted,
        ),
        Phase::Held => (
            "Waiting: fetching is paused (LOG's Resume fetching).".into(),
            skin.warning,
        ),
        Phase::Downloading(job) => {
            let left = s.eta().map(duration).unwrap_or_default();
            (
                format!(
                    "Downloading {job} · {} of {} · {left} left",
                    s.done + 1,
                    s.planned
                ),
                skin.info,
            )
        }
        Phase::Retrying { job, error, at } => (
            format!(
                "{job} failed ({error}); trying again in {} s",
                at.saturating_duration_since(Instant::now()).as_secs()
            ),
            skin.warning,
        ),
        Phase::UpToDate => {
            let mut text = "Up to date; looks for new days every hour.".to_owned();
            if !s.not_published.is_empty() {
                text.push_str(&format!(
                    " MISO has not published {} of them yet.",
                    s.not_published.len()
                ));
            }
            (text, skin.text_muted)
        }
    }
}

/// The status bar's note while the backfill downloads.
pub fn status_bar_text(s: &BackfillStatus, skin: &Skin) -> Option<(String, Color32)> {
    match &s.phase {
        Phase::Downloading(_) => Some((
            format!("history {} of {}", s.done, s.planned),
            skin.text_muted,
        )),
        Phase::Retrying { .. } => Some(("history: retrying".into(), skin.warning)),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn windows_sizes_and_times_read_well() {
        assert_eq!(window_label(92), "3 months");
        assert_eq!(window_label(0), "Everything since 2023-01-01");
        assert_eq!(window_label(45), "45 days");
        let today = NaiveDate::from_ymd_opt(2026, 10, 8).unwrap();
        assert_eq!(reports_in(&HistoryConfig::default(), today), 92 * 2 - 1);
        let all = HistoryConfig {
            keep_days: 0,
            ..HistoryConfig::default()
        };
        assert_eq!(reports_in(&all, today), (1377 * 2) - 1);
        assert_eq!(
            estimate(&HistoryConfig::default(), today),
            "about 30 MB on disk; filling it downloads about 227 MB"
        );
        assert_eq!(size(5 * 1_048_576 / 2), "2.5 MB");
        assert_eq!(size(470 * 1_048_576), "470 MB");
        assert_eq!(size(3 * 1_073_741_824 + 400 * 1_048_576), "3.4 GB");
        assert_eq!(duration(Duration::from_secs(20)), "under a minute");
        assert_eq!(duration(Duration::from_secs(4 * 60)), "about 4 min");
        assert_eq!(duration(Duration::from_secs(80 * 60)), "about 1 h 20 min");
    }
}
