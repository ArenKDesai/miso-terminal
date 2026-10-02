//! The data hub: everything between "a panel wants data" and "bytes on the wire".
//!
//! This crate knows nothing about MISO. It provides:
//!
//! - [`Query`]: a typed, cacheable request. Sources implement it once per dataset.
//! - [`DataHub`]: deduplicates, caches and refreshes queries on a background
//!   runtime. Panels call [`DataHub::watch`] every frame and never block.
//! - [`FetchCtx`]: what a query uses to fetch, with a polite per-URL rate limit,
//!   a concurrency cap and an on-disk cache for immutable files.
//! - [`Transport`]: HTTP in production, fixture files offline and in tests.
//! - [`EventLog`]: a ring buffer of fetch activity for the in-app log.

mod cache;
mod ctx;
mod error;
mod event;
mod hub;
mod query;
mod transport;

pub use cache::DiskCache;
pub use ctx::{FetchCtx, FetchCtxOptions};
pub use error::FetchError;
pub use event::{Event, EventLevel, EventLog};
pub use hub::{DataHub, EntryState, EntryStatus, Snapshot};
pub use query::{Freshness, Query};
pub use transport::{BoxFuture, FixtureTransport, HttpTransport, Transport};
