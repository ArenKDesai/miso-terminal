//! The data hub: everything between "a panel wants data" and "bytes on the wire".
//!
//! This crate knows nothing about MISO. It provides:
//!
//! - [`Query`]: a typed, cacheable request. Sources implement it once per dataset.
//! - [`Stream`]: a typed WebSocket feed, shared by every panel that watches it.
//! - [`DataHub`]: deduplicates, caches and refreshes queries, and owns stream
//!   connections, on a background runtime. Panels call [`DataHub::watch`] and
//!   [`DataHub::watch_stream`] every frame and never block.
//! - [`FetchCtx`]: what a query uses to fetch: [`Request`]s with any method,
//!   headers and body; a polite per-URL rate limit; conditional GETs; request
//!   [`Budget`]s per host; a concurrency cap; secrets; and an on-disk cache
//!   for immutable files.
//! - [`Transport`]: HTTP and WebSockets in production, fixture files offline
//!   and in tests.
//! - [`SecretStore`]: API keys in Windows Credential Manager, never in files or logs.
//! - [`EventLog`]: a ring buffer of fetch activity for the in-app log.

mod budget;
mod cache;
mod ctx;
mod error;
mod event;
mod hub;
pub mod issued;
mod query;
mod request;
mod secret;
mod stream;
mod transport;

pub use budget::{Budget, BudgetStatus};
pub use cache::DiskCache;
pub use ctx::{FetchCtx, FetchCtxOptions};
pub use error::FetchError;
pub use event::{Event, EventLevel, EventLog};
pub use hub::{DataHub, EntryState, EntryStatus, Snapshot};
pub use query::{Freshness, Query};
pub use request::{HeaderValue, Method, Request, Response, host_of};
#[cfg(windows)]
pub use secret::CredentialManager;
pub use secret::{MemorySecrets, Secret, SecretError, SecretStore, os_store};
pub use stream::{Applied, Frame, Stream, StreamConn, StreamPhase, StreamStatus};
pub use transport::{BoxFuture, FixtureTransport, HttpTransport, Transport};
