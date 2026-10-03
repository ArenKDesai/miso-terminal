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

pub mod endpoints;
pub mod parse;
pub mod queries;

pub use endpoints::MisoEndpoints;
pub use queries::{
    ApiQuery, DayReportQuery, INTERVALS_PER_DAY, Miso, RtArchiveQuery, RtBestDayQuery,
    RtIntradayQuery, RtPreviousDayQuery, intraday_archive_key,
};
