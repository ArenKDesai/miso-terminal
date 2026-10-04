//! Request budgets: at most `max` requests per `per` to a group of hosts,
//! shared by every query (Alpaca allows 200 a minute per key across its APIs;
//! the terminal keeps to about 180). A request that would exceed the budget
//! waits for a slot instead of failing, and a `429 Too Many Requests` closes
//! the budget until the server's `Retry-After` has passed.

use std::collections::VecDeque;
use std::time::Duration;

use parking_lot::Mutex;
use tokio::time::Instant;

/// A limit on requests to one or more hosts.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Budget {
    /// Shown in LOG, e.g. `Alpaca`.
    pub name: String,
    /// Exact host names the budget covers.
    pub hosts: Vec<String>,
    pub max: u32,
    pub per: Duration,
}

impl Budget {
    pub fn new(
        name: impl Into<String>,
        hosts: impl IntoIterator<Item = impl Into<String>>,
        max: u32,
        per: Duration,
    ) -> Self {
        Self {
            name: name.into(),
            hosts: hosts.into_iter().map(Into::into).collect(),
            max: max.max(1),
            per,
        }
    }

    fn covers(&self, host: &str) -> bool {
        self.hosts.iter().any(|h| h.eq_ignore_ascii_case(host))
    }
}

/// One row of LOG's budget table.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct BudgetStatus {
    pub name: String,
    pub used: u32,
    pub max: u32,
    pub per: Duration,
    /// Requests that had to wait for a slot, since launch.
    pub waits: u64,
    /// Set while a `429` holds the budget closed.
    pub blocked_for: Option<Duration>,
}

struct Window {
    budget: Budget,
    sent: VecDeque<Instant>,
    blocked_until: Option<Instant>,
    waits: u64,
}

impl Window {
    fn forget_old(&mut self, now: Instant) {
        while self
            .sent
            .front()
            .is_some_and(|t| now.duration_since(*t) >= self.budget.per)
        {
            self.sent.pop_front();
        }
    }

    /// Take a slot now, or say how long until one frees up.
    fn try_take(&mut self, now: Instant) -> Result<(), Duration> {
        if let Some(until) = self.blocked_until {
            if now < until {
                return Err(until - now);
            }
            self.blocked_until = None;
        }
        self.forget_old(now);
        if (self.sent.len() as u32) < self.budget.max {
            self.sent.push_back(now);
            return Ok(());
        }
        let oldest = self.sent.front().copied().unwrap_or(now);
        Err((oldest + self.budget.per).saturating_duration_since(now))
    }
}

/// Every configured budget with its recent requests.
#[derive(Default)]
pub struct Budgets {
    windows: Mutex<Vec<Window>>,
}

impl Budgets {
    pub fn new(budgets: &[Budget]) -> Self {
        Self {
            windows: Mutex::new(
                budgets
                    .iter()
                    .map(|b| Window {
                        budget: b.clone(),
                        sent: VecDeque::new(),
                        blocked_until: None,
                        waits: 0,
                    })
                    .collect(),
            ),
        }
    }

    /// Take a slot for a request to `host`, or the time to wait before trying
    /// again. Hosts outside every budget always get `Ok`.
    pub fn try_acquire(&self, host: &str, now: Instant) -> Result<(), Duration> {
        let mut windows = self.windows.lock();
        let Some(w) = windows.iter_mut().find(|w| w.budget.covers(host)) else {
            return Ok(());
        };
        let result = w.try_take(now);
        if result.is_err() {
            w.waits += 1;
        }
        result
    }

    /// Wait (asynchronously) until a request to `host` fits its budget.
    /// Returns how long it waited.
    pub async fn acquire(&self, host: &str) -> Duration {
        let started = Instant::now();
        while let Err(wait) = self.try_acquire(host, Instant::now()) {
            tokio::time::sleep(wait.max(Duration::from_millis(10))).await;
        }
        started.elapsed()
    }

    /// The server said to slow down: hold the host's budget closed for `retry_after`.
    pub fn block(&self, host: &str, retry_after: Duration) {
        let mut windows = self.windows.lock();
        if let Some(w) = windows.iter_mut().find(|w| w.budget.covers(host)) {
            let until = Instant::now() + retry_after;
            w.blocked_until = Some(w.blocked_until.map_or(until, |u| u.max(until)));
        }
    }

    pub fn status(&self) -> Vec<BudgetStatus> {
        let now = Instant::now();
        self.windows
            .lock()
            .iter_mut()
            .map(|w| {
                w.forget_old(now);
                BudgetStatus {
                    name: w.budget.name.clone(),
                    used: w.sent.len() as u32,
                    max: w.budget.max,
                    per: w.budget.per,
                    waits: w.waits,
                    blocked_for: w
                        .blocked_until
                        .and_then(|u| u.checked_duration_since(now))
                        .filter(|d| !d.is_zero()),
                }
            })
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn alpaca(max: u32) -> Budgets {
        Budgets::new(&[Budget::new(
            "Alpaca",
            ["data.alpaca.markets", "paper-api.alpaca.markets"],
            max,
            Duration::from_secs(60),
        )])
    }

    #[test]
    fn a_full_window_waits_for_the_oldest_request_to_age_out() {
        let b = alpaca(3);
        let t0 = Instant::now();
        for i in 0..3 {
            let host = if i % 2 == 0 {
                "data.alpaca.markets"
            } else {
                "PAPER-API.alpaca.markets"
            };
            assert_eq!(b.try_acquire(host, t0 + Duration::from_secs(i)), Ok(()));
        }
        // Hosts share the budget: the fourth request in the minute must wait
        // until the first one is a minute old.
        let t = t0 + Duration::from_secs(10);
        assert_eq!(
            b.try_acquire("data.alpaca.markets", t),
            Err(Duration::from_secs(50))
        );
        assert_eq!(
            b.try_acquire("data.alpaca.markets", t0 + Duration::from_secs(60)),
            Ok(())
        );
        // Other hosts are not budgeted.
        for _ in 0..10 {
            assert_eq!(b.try_acquire("public-api.misoenergy.org", t), Ok(()));
        }
        let s = &b.status()[0];
        assert_eq!((s.max, s.waits), (3, 1));
    }

    #[test]
    fn too_many_requests_closes_the_budget() {
        let b = alpaca(100);
        b.block("data.alpaca.markets", Duration::from_secs(30));
        let wait = b
            .try_acquire("paper-api.alpaca.markets", Instant::now())
            .unwrap_err();
        assert!(wait > Duration::from_secs(29) && wait <= Duration::from_secs(30));
        assert!(b.status()[0].blocked_for.is_some());
        assert!(b.try_acquire("example.com", Instant::now()).is_ok());
    }

    #[tokio::test(start_paused = true)]
    async fn acquire_sleeps_until_a_slot_frees() {
        let b = Budgets::new(&[Budget::new("t", ["h"], 2, Duration::from_secs(1))]);
        assert_eq!(b.acquire("h").await, Duration::ZERO);
        b.acquire("h").await;
        let started = tokio::time::Instant::now();
        b.acquire("h").await;
        assert!(started.elapsed() >= Duration::from_secs(1));
    }
}
