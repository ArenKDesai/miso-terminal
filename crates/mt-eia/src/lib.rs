//! The U.S. Energy Information Administration as a data source: the Henry Hub
//! natural gas spot price, which sets the marginal cost of gas-fired power and
//! so much of MISO's energy price.
//!
//! EIA publishes the daily series as a spreadsheet that needs no API key. It
//! is updated weekly and trails by a few days. Like `mt-nws`, this crate
//! depends only on `mt-core` and `mt-data`.

pub mod parse;

use std::sync::Arc;
use std::time::Duration;

use mt_core::SpotPrices;
use mt_data::{FetchCtx, FetchError, Freshness, Query};

/// EIA updates the series once a week; looking a few times a day is plenty.
pub const SPOT_REFRESH: Duration = Duration::from_secs(6 * 60 * 60);

/// Entry point for building EIA queries.
#[derive(Clone, Debug)]
pub struct Eia {
    base: Arc<str>,
}

impl Default for Eia {
    fn default() -> Self {
        Self {
            base: "https://www.eia.gov".into(),
        }
    }
}

impl Eia {
    /// Henry Hub daily spot, $/MMBtu, since 1997.
    pub fn henry_hub_url(&self) -> String {
        format!(
            "{}/dnav/ng/hist_xls/RNGWHHDd.xls",
            self.base.trim_end_matches('/')
        )
    }

    pub fn henry_hub(&self) -> HenryHubQuery {
        HenryHubQuery { eia: self.clone() }
    }
}

/// The Henry Hub daily spot price history.
#[derive(Clone, Debug)]
pub struct HenryHubQuery {
    eia: Eia,
}

impl Query for HenryHubQuery {
    type Output = SpotPrices;

    fn key(&self) -> String {
        "eia/henry-hub".into()
    }

    fn label(&self) -> String {
        "Henry Hub gas spot (EIA)".into()
    }

    fn freshness(&self, _: &SpotPrices) -> Freshness {
        Freshness::Every(SPOT_REFRESH)
    }

    async fn fetch(
        &self,
        ctx: FetchCtx,
        _prev: Option<Arc<SpotPrices>>,
    ) -> Result<SpotPrices, FetchError> {
        let body = ctx.get(&self.eia.henry_hub_url()).await?;
        parse::parse_spot_history("Henry Hub", "$/MMBtu", &body)
    }
}
