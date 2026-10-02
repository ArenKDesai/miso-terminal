use std::collections::VecDeque;
use std::sync::Arc;

use chrono::{DateTime, Utc};
use parking_lot::Mutex;

const CAPACITY: usize = 1000;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum EventLevel {
    Info,
    Warn,
    Error,
}

#[derive(Clone, Debug)]
pub struct Event {
    pub at: DateTime<Utc>,
    pub level: EventLevel,
    pub message: String,
}

/// In-memory ring buffer of data-layer activity, shown by the LOG function.
/// Everything logged here also goes to `tracing` (and so the log file).
#[derive(Clone, Default)]
pub struct EventLog {
    inner: Arc<Mutex<VecDeque<Event>>>,
}

impl EventLog {
    pub fn push(&self, level: EventLevel, message: impl Into<String>) {
        let message = message.into();
        match level {
            EventLevel::Info => tracing::debug!("{message}"),
            EventLevel::Warn => tracing::warn!("{message}"),
            EventLevel::Error => tracing::error!("{message}"),
        }
        let mut q = self.inner.lock();
        if q.len() == CAPACITY {
            q.pop_front();
        }
        q.push_back(Event {
            at: Utc::now(),
            level,
            message,
        });
    }

    pub fn info(&self, message: impl Into<String>) {
        self.push(EventLevel::Info, message);
    }

    pub fn warn(&self, message: impl Into<String>) {
        self.push(EventLevel::Warn, message);
    }

    pub fn error(&self, message: impl Into<String>) {
        self.push(EventLevel::Error, message);
    }

    /// Most recent events, newest last.
    pub fn recent(&self, n: usize) -> Vec<Event> {
        let q = self.inner.lock();
        q.iter().skip(q.len().saturating_sub(n)).cloned().collect()
    }
}
