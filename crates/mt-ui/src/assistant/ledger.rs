//! What ASK has spent this month, kept beside the layout (not in the cache,
//! which *Clear cache* empties) so the monthly cap holds across restarts.

use std::path::{Path, PathBuf};

use mt_ai::Ledger;
use parking_lot::Mutex;

/// One read-modify-write at a time, across panels.
static LOCK: Mutex<()> = Mutex::new(());

/// `ai-usage.json` beside the saved layout.
pub fn path(state_file: &Path) -> PathBuf {
    state_file.with_file_name("ai-usage.json")
}

/// The ledger, or an empty one when there is none (or it cannot be read:
/// a damaged file must not stop the cap, so the caller treats an unreadable
/// one as spent, see [`spent_this_month`]).
pub fn load(path: &Path) -> Option<Ledger> {
    let _guard = LOCK.lock();
    read(path)
}

fn read(path: &Path) -> Option<Ledger> {
    match std::fs::read(path) {
        Ok(bytes) => serde_json::from_slice(&bytes).ok(),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Some(Ledger::default()),
        Err(_) => None,
    }
}

/// What this month has spent, in dollars; `f64::INFINITY` when the ledger
/// cannot be read, so nothing is sent until it can.
pub fn spent_this_month(path: &Path) -> f64 {
    let month = Ledger::month_of(mt_core::time::now_utc());
    load(path).map_or(f64::INFINITY, |l| l.spent_in(&month))
}

/// Add a request's cost and save. Returns the month's total.
pub fn record(path: &Path, usd: f64) -> std::io::Result<f64> {
    let _guard = LOCK.lock();
    let month = Ledger::month_of(mt_core::time::now_utc());
    let mut ledger =
        read(path).ok_or_else(|| std::io::Error::other("the usage ledger cannot be read"))?;
    ledger.record(&month, usd);
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)?;
    }
    let tmp = path.with_extension("json.tmp");
    std::fs::write(&tmp, serde_json::to_vec_pretty(&ledger)?)?;
    std::fs::rename(&tmp, path)?;
    Ok(ledger.spent_in(&month))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn records_and_reads_back() {
        let dir = std::env::temp_dir().join(format!("mt-ledger-{}", std::process::id()));
        let p = path(&dir.join("state.ron"));
        let _ = std::fs::remove_file(&p);
        assert_eq!(spent_this_month(&p), 0.0);
        record(&p, 0.25).expect("saved");
        assert_eq!(record(&p, 0.5).expect("saved"), 0.75);
        assert_eq!(spent_this_month(&p), 0.75);
        std::fs::write(&p, b"not json").expect("written");
        assert_eq!(
            spent_this_month(&p),
            f64::INFINITY,
            "a damaged ledger stops spending"
        );
        assert!(record(&p, 0.1).is_err(), "and is not overwritten");
        let _ = std::fs::remove_dir_all(&dir);
    }
}
