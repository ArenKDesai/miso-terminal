use chrono::NaiveDate;
use serde::{Deserialize, Serialize};

/// Base URLs for MISO's public data. Overridable from `config.toml` so a moved
/// endpoint is a config change, not a release.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct MisoEndpoints {
    /// Real-time data API (JSON). Replaced the old `MISORTWDDataBroker` feeds
    /// on 2025-12-12.
    pub public_api: String,
    /// Daily market report CSVs.
    pub market_reports: String,
}

impl Default for MisoEndpoints {
    fn default() -> Self {
        Self {
            public_api: "https://public-api.misoenergy.org/api".into(),
            market_reports: "https://docs.misoenergy.org/marketreports".into(),
        }
    }
}

impl MisoEndpoints {
    pub fn api(&self, path: &str) -> String {
        format!(
            "{}/{}",
            self.public_api.trim_end_matches('/'),
            path.trim_start_matches('/')
        )
    }

    /// `<market_reports>/<yyyymmdd>_<suffix>.csv`
    pub fn report(&self, day: NaiveDate, suffix: &str) -> String {
        format!(
            "{}/{}_{suffix}.csv",
            self.market_reports.trim_end_matches('/'),
            day.format("%Y%m%d")
        )
    }
}

/// Paths under [`MisoEndpoints::public_api`]. Browse them at
/// <https://public-api.misoenergy.org/>.
pub mod paths {
    pub const LMP_CONSOLIDATED: &str = "MarketPricing/GetLmpConsolidatedTable";
    pub const EXANTE_HUBS: &str = "MarketPricing/GetExAnteLmp";
    pub const RT_FIVE_MIN_CURRENT: &str = "MarketPricing/GetRealTimeFiveMinExPost/Current";
    pub const RT_FIVE_MIN_ROLLING: &str = "MarketPricing/GetRealTimeFiveMinExPost/Rolling";
    /// The whole previous market day, every CP node (~11 MB gzipped).
    pub const RT_FIVE_MIN_PREVIOUS: &str = "MarketPricing/GetRealTimeFiveMinExPost/Previous";
    pub const ANCILLARY_MCP: &str = "MarketPricing/GetAncillaryServicesMcp";
    pub const FUEL_MIX: &str = "FuelMix";
    pub const FUEL_MIX_TODAY: &str = "FuelMix/Today";
    pub const LOAD: &str = "RealTimeTotalLoad";
    pub const NSI: &str = "Interchange/GetNsi";
    pub const NSI_FIVE_MIN: &str = "Interchange/GetNsi/FiveMinute";
    pub const NAI: &str = "Interchange/GetNai";
    pub const BINDING_CONSTRAINTS: &str = "BindingConstraints/RealTime";
    pub const RESERVE_CONSTRAINTS: &str = "BindingConstraints/Reserve";
    pub const SUBREGIONAL_CONSTRAINTS: &str = "BindingConstraints/SubRegional";
    pub const WIND_SOLAR: &str = "WindSolar/GetCombined";
    pub const OUTAGES: &str = "GenerationOutages/GetGenerationOutagesPlusMinusFiveDays";
    pub const CAPACITY: &str = "CsatSupplyDemand";
    pub const SNAPSHOT: &str = "Snapshot";
    pub const REGIONAL_TRANSFER: &str = "RegionalDirectionalTransfer";
    pub const ACE: &str = "Ace";
    pub const RSG_COMMITMENTS: &str = "RealTimeRSGCommitments";
    pub const STR_REQUIREMENT: &str = "CsatNextDayShortTermReserveRequirement";
    /// PJM's forecast LMP at the MISO interface (coordinated transaction scheduling).
    pub const CTS: &str = "CoordinatedTransactionScheduling";

    /// Every JSON path, for the fixture recorder.
    pub const ALL: &[&str] = &[
        LMP_CONSOLIDATED,
        EXANTE_HUBS,
        RT_FIVE_MIN_CURRENT,
        RT_FIVE_MIN_ROLLING,
        RT_FIVE_MIN_PREVIOUS,
        ANCILLARY_MCP,
        FUEL_MIX,
        FUEL_MIX_TODAY,
        LOAD,
        NSI,
        NSI_FIVE_MIN,
        NAI,
        BINDING_CONSTRAINTS,
        RESERVE_CONSTRAINTS,
        SUBREGIONAL_CONSTRAINTS,
        WIND_SOLAR,
        OUTAGES,
        CAPACITY,
        SNAPSHOT,
        REGIONAL_TRANSFER,
        ACE,
        RSG_COMMITMENTS,
        STR_REQUIREMENT,
        CTS,
    ];
}

/// Daily report file suffixes under [`MisoEndpoints::market_reports`].
pub mod reports {
    pub const DA_EXPOST: &str = "da_expost_lmp";
    pub const DA_EXANTE: &str = "da_exante_lmp";
    pub const RT_FINAL: &str = "rt_lmp_final";
    pub const RT_PRELIM: &str = "rt_lmp_prelim";
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn builds_urls() {
        let e = MisoEndpoints::default();
        assert_eq!(
            e.api(paths::FUEL_MIX),
            "https://public-api.misoenergy.org/api/FuelMix"
        );
        let day = NaiveDate::from_ymd_opt(2026, 10, 1).unwrap();
        assert_eq!(
            e.report(day, reports::DA_EXPOST),
            "https://docs.misoenergy.org/marketreports/20261001_da_expost_lmp.csv"
        );
    }
}
