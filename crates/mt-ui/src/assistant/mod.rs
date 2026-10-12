//! The assistant (ASK): the lookups Claude may call and what it is told.
//! The conversation itself lives in `mt-ai`; this module joins it to the
//! terminal's data.

pub mod ledger;
pub mod prompt;
pub mod tools;

pub use tools::Lookups;
