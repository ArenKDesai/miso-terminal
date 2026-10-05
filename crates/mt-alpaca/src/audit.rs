//! The order audit log: every order request the desk sends and every answer
//! (or the lack of one), one JSON object per line, appended to
//! `orders-YYYY-MM.jsonl` in the terminal's local data folder. Request
//! bodies and responses only: keys travel in headers, which are never
//! written.

use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use chrono::{DateTime, Utc};
use parking_lot::Mutex;
use serde_json::Value;

/// Lines kept in memory for the blotter's view of the log.
const RECENT_KEEP: usize = 200;

#[derive(Clone, Default)]
pub struct AuditLog {
    dir: Option<PathBuf>,
    inner: Arc<Mutex<Recent>>,
}

#[derive(Default)]
struct Recent {
    lines: Vec<String>,
    /// The last failure to write the file, for the blotter to show.
    error: Option<String>,
}

impl AuditLog {
    /// Append to files in `dir` (created when first needed).
    pub fn to_dir(dir: impl Into<PathBuf>) -> Self {
        Self {
            dir: Some(dir.into()),
            inner: Arc::default(),
        }
    }

    /// Keep entries in memory only (tests, offline replay).
    pub fn in_memory() -> Self {
        Self::default()
    }

    pub fn dir(&self) -> Option<&Path> {
        self.dir.as_deref()
    }

    /// The month's file: `orders-2026-10.jsonl`.
    pub fn path_for(&self, at: DateTime<Utc>) -> Option<PathBuf> {
        self.dir
            .as_ref()
            .map(|d| d.join(format!("orders-{}.jsonl", at.format("%Y-%m"))))
    }

    /// Append one entry, stamped with the time it happened (the real clock,
    /// not the terminal's frozen one).
    pub fn record(&self, mut entry: Value) {
        let at = Utc::now();
        if let Value::Object(m) = &mut entry {
            m.insert("at".into(), Value::String(at.to_rfc3339()));
        }
        let line = entry.to_string();
        let written = self.path_for(at).map(|path| append(&path, &line));
        let mut inner = self.inner.lock();
        match written {
            Some(Err(e)) => inner.error = Some(format!("could not write the order audit log: {e}")),
            Some(Ok(())) => inner.error = None,
            None => {}
        }
        inner.lines.push(line);
        let excess = inner.lines.len().saturating_sub(RECENT_KEEP);
        inner.lines.drain(..excess);
    }

    /// The latest entries, oldest first.
    pub fn recent(&self) -> Vec<String> {
        self.inner.lock().lines.clone()
    }

    pub fn error(&self) -> Option<String> {
        self.inner.lock().error.clone()
    }
}

fn append(path: &Path, line: &str) -> std::io::Result<()> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let mut f = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(path)?;
    writeln!(f, "{line}")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn appends_lines_to_the_month_file() {
        let dir = std::env::temp_dir().join(format!("mt-audit-{}", std::process::id()));
        let log = AuditLog::to_dir(&dir);
        log.record(serde_json::json!({"action": "place", "client_order_id": "mt-1"}));
        log.record(serde_json::json!({"action": "place", "response": {"status": 200}}));
        let path = log.path_for(Utc::now()).unwrap();
        let text = std::fs::read_to_string(&path).unwrap();
        let lines: Vec<&str> = text.lines().collect();
        assert_eq!(lines.len(), 2);
        let first: Value = serde_json::from_str(lines[0]).unwrap();
        assert_eq!(first["client_order_id"], "mt-1");
        assert!(first["at"].is_string());
        assert_eq!(log.recent().len(), 2);
        assert!(log.error().is_none());
        std::fs::remove_dir_all(dir).unwrap();
        let mem = AuditLog::in_memory();
        mem.record(serde_json::json!({"action": "x"}));
        assert_eq!(mem.recent().len(), 1);
        assert!(mem.path_for(Utc::now()).is_none());
    }
}
