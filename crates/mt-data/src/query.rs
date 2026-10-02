use std::future::Future;
use std::sync::Arc;
use std::time::Duration;

use crate::{FetchCtx, FetchError};

/// How long a query's result stays fresh before the hub refetches it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Freshness {
    /// Refetch once the value is older than this while something still watches it.
    Every(Duration),
    /// Never refetch on a timer (immutable data, e.g. a settled market report).
    Forever,
}

/// A typed, cacheable request for one dataset.
///
/// Implement this once per dataset. The hub identifies a query by [`Query::key`],
/// so two panels asking for the same thing share one fetch and one cached value.
///
/// ```ignore
/// #[derive(Clone)]
/// struct FuelMixQuery;
/// impl Query for FuelMixQuery {
///     type Output = FuelMix;
///     fn key(&self) -> String { "miso/fuel-mix".into() }
///     fn freshness(&self, _: &FuelMix) -> Freshness { Freshness::Every(Duration::from_secs(60)) }
///     async fn fetch(&self, ctx: FetchCtx, _prev: Option<Arc<FuelMix>>) -> Result<FuelMix, FetchError> {
///         parse_fuel_mix(&ctx.get_text(URL).await?)
///     }
/// }
/// ```
pub trait Query: Clone + Send + Sync + 'static {
    type Output: Send + Sync + 'static;

    /// Unique cache key including every parameter, by convention
    /// `<source>/<dataset>[/<params>]`.
    fn key(&self) -> String;

    /// Human-readable name for the data-feed log.
    fn label(&self) -> String {
        self.key()
    }

    /// How long `current` stays fresh. Can depend on the value: a preliminary
    /// report might refresh hourly while a final one never does.
    fn freshness(&self, current: &Self::Output) -> Freshness;

    /// Fetch a new value. `prev` is the value currently cached, for queries that
    /// build on what they already have (append-only feeds).
    fn fetch(
        &self,
        ctx: FetchCtx,
        prev: Option<Arc<Self::Output>>,
    ) -> impl Future<Output = Result<Self::Output, FetchError>> + Send;
}
