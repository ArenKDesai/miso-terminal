//! MISO as a data source.
//!
//! Three layers, each replaceable on its own:
//!
//! - [`endpoints`]: base URLs (configurable) and every path, in one place. When
//!   MISO moves an endpoint, this is the only file that changes.
//! - [`parse`]: pure functions from response text to `mt-core` types, tested
//!   against recorded fixtures in `fixtures/`. When MISO changes a format, the
//!   failing fixture test points at the parser to fix.
//! - [`queries`]: `mt_data::Query` implementations that tie the two together,
//!   reached through the [`Miso`] facade.
//!
//! [`history`] keeps the day store to a window of days and fills it in the
//! background ([`Backfill`]).

pub mod endpoints;
pub mod history;
pub mod issued;
pub mod parse;
pub mod queries;

pub use endpoints::MisoEndpoints;
pub use history::{Backfill, BackfillStatus, HistoryConfig, StoredPrices, StoredPricesQuery};
pub use queries::{
    ApiQuery, ConstraintHistoryQuery, DayReportQuery, INTERVALS_PER_DAY, Miso, RtArchiveQuery,
    RtBestDayQuery, RtIntradayQuery, RtPreviousDayQuery, archive_dir, day_store_dir, day_store_key,
    discard_exported_history, intraday_archive_key, prune_archive, read_day_store,
};
