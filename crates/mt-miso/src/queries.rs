//! `mt_data::Query` implementations for MISO, reached through [`Miso`].
//!
//! Most datasets are "GET one JSON path, parse it, refresh every minute", which
//! [`ApiQuery`] covers with a static [`ApiSpec`]. Adding such a dataset is a
//! path in `endpoints.rs`, a parser in `parse.rs`, a spec below and a one-line
//! method on [`Miso`]. Feeds with real logic (append-only intraday history,
//! report fallbacks) get their own types.

use std::fmt;
use std::sync::Arc;
use std::time::Duration;

use chrono::{NaiveDate, TimeDelta};
use mt_core::time::market_today;
use mt_core::*;
use mt_data::{FetchCtx, FetchError, Freshness, Query};

use crate::endpoints::{MisoEndpoints, paths, reports};
use crate::parse;

/// MISO refreshes its real-time displays every five minutes and asks clients to
/// poll each link at most once a minute.
pub const REALTIME_REFRESH: Duration = Duration::from_secs(60);

/// Entry point for building MISO queries. Cheap to clone; panels hold one.
#[derive(Clone, Debug, Default)]
pub struct Miso {
    endpoints: Arc<MisoEndpoints>,
}

impl Miso {
    pub fn new(endpoints: MisoEndpoints) -> Self {
        Self {
            endpoints: Arc::new(endpoints),
        }
    }

    pub fn endpoints(&self) -> &MisoEndpoints {
        &self.endpoints
    }

    fn api<T: Send + Sync + 'static>(&self, spec: &'static ApiSpec<T>) -> ApiQuery<T> {
        ApiQuery {
            endpoints: self.endpoints.clone(),
            spec,
        }
    }

    pub fn lmp_board(&self) -> ApiQuery<LmpBoard> {
        self.api(&LMP_BOARD)
    }
    pub fn exante_hubs(&self) -> ApiQuery<HubExAnte> {
        self.api(&EXANTE_HUBS)
    }
    pub fn ancillary(&self) -> ApiQuery<AncillaryMcps> {
        self.api(&ANCILLARY)
    }
    pub fn fuel_mix(&self) -> ApiQuery<FuelMix> {
        self.api(&FUEL_MIX)
    }
    pub fn fuel_mix_today(&self) -> ApiQuery<FuelMixHistory> {
        self.api(&FUEL_MIX_TODAY)
    }
    pub fn load(&self) -> ApiQuery<SystemLoad> {
        self.api(&LOAD)
    }
    pub fn interchange(&self) -> ApiQuery<Interchange> {
        self.api(&NSI)
    }
    pub fn interchange_history(&self) -> ApiQuery<InterchangeHistory> {
        self.api(&NSI_HISTORY)
    }
    pub fn binding_constraints(&self) -> ApiQuery<BindingConstraints> {
        self.api(&CONSTRAINTS)
    }
    pub fn renewables(&self) -> ApiQuery<Renewables> {
        self.api(&RENEWABLES)
    }
    pub fn outages(&self) -> ApiQuery<Outages> {
        self.api(&OUTAGES)
    }
    pub fn capacity(&self) -> ApiQuery<Capacity> {
        self.api(&CAPACITY)
    }
    pub fn snapshot(&self) -> ApiQuery<GridSnapshot> {
        self.api(&SNAPSHOT)
    }
    pub fn regional_transfer(&self) -> ApiQuery<RegionalTransfer> {
        self.api(&REGIONAL_TRANSFER)
    }
    pub fn ace(&self) -> ApiQuery<Ace> {
        self.api(&ACE)
    }

    /// Five-minute RT prices for every CP node, today so far.
    pub fn rt_intraday(&self) -> RtIntradayQuery {
        RtIntradayQuery {
            endpoints: self.endpoints.clone(),
        }
    }

    /// Yesterday's five-minute RT prices at every CP node.
    pub fn rt_previous_day(&self) -> RtPreviousDayQuery {
        RtPreviousDayQuery {
            endpoints: self.endpoints.clone(),
        }
    }

    /// One daily report. `None` inside the result means "not published yet".
    pub fn day_report(&self, kind: DayReportKind, day: NaiveDate) -> DayReportQuery {
        DayReportQuery {
            endpoints: self.endpoints.clone(),
            kind,
            day,
        }
    }

    /// The best available RT report for a day: final if settled, else prelim.
    pub fn rt_best_day(&self, day: NaiveDate) -> RtBestDayQuery {
        RtBestDayQuery {
            endpoints: self.endpoints.clone(),
            day,
        }
    }
}

// --- Generic JSON endpoint ----------------------------------------------------

/// Static description of a simple JSON dataset.
pub struct ApiSpec<T> {
    pub key: &'static str,
    pub label: &'static str,
    pub path: &'static str,
    pub refresh: Duration,
    pub parse: fn(&str) -> Result<T, FetchError>,
}

macro_rules! api_spec {
    ($name:ident: $t:ty = $key:literal, $label:literal, $path:expr, $parse:path) => {
        static $name: ApiSpec<$t> = ApiSpec {
            key: concat!("miso/", $key),
            label: $label,
            path: $path,
            refresh: REALTIME_REFRESH,
            parse: $parse,
        };
    };
}

api_spec!(LMP_BOARD: LmpBoard = "lmp-board", "LMP consolidated table", paths::LMP_CONSOLIDATED, parse::parse_lmp_board);
api_spec!(EXANTE_HUBS: HubExAnte = "exante-hubs", "Ex-ante hub LMPs", paths::EXANTE_HUBS, parse::parse_exante_hubs);
api_spec!(ANCILLARY: AncillaryMcps = "ancillary-mcp", "Ancillary-service MCPs", paths::ANCILLARY_MCP, parse::parse_ancillary);
api_spec!(FUEL_MIX: FuelMix = "fuel-mix", "Fuel mix", paths::FUEL_MIX, parse::parse_fuel_mix);
api_spec!(FUEL_MIX_TODAY: FuelMixHistory = "fuel-mix-today", "Fuel mix, today", paths::FUEL_MIX_TODAY, parse::parse_fuel_mix_history);
api_spec!(LOAD: SystemLoad = "load", "Real-time load", paths::LOAD, parse::parse_load);
api_spec!(NSI: Interchange = "nsi", "Net scheduled interchange", paths::NSI, parse::parse_nsi);
api_spec!(NSI_HISTORY: InterchangeHistory = "nsi-5min", "Interchange, five-minute", paths::NSI_FIVE_MIN, parse::parse_nsi_history);
api_spec!(CONSTRAINTS: BindingConstraints = "binding-constraints", "Binding constraints", paths::BINDING_CONSTRAINTS, parse::parse_binding_constraints);
api_spec!(RENEWABLES: Renewables = "wind-solar", "Wind and solar", paths::WIND_SOLAR, parse::parse_renewables);
api_spec!(OUTAGES: Outages = "outages", "Generation outages", paths::OUTAGES, parse::parse_outages);
api_spec!(CAPACITY: Capacity = "capacity", "Supply and demand", paths::CAPACITY, parse::parse_capacity);
api_spec!(SNAPSHOT: GridSnapshot = "snapshot", "Grid snapshot", paths::SNAPSHOT, parse::parse_snapshot);
api_spec!(
    REGIONAL_TRANSFER: RegionalTransfer = "regional-transfer",
    "Regional directional transfer",
    paths::REGIONAL_TRANSFER,
    parse::parse_regional_transfer
);
api_spec!(ACE: Ace = "ace", "Area control error", paths::ACE, parse::parse_ace);

/// A query for one [`ApiSpec`] dataset.
pub struct ApiQuery<T: 'static> {
    endpoints: Arc<MisoEndpoints>,
    spec: &'static ApiSpec<T>,
}

impl<T> Clone for ApiQuery<T> {
    fn clone(&self) -> Self {
        Self {
            endpoints: self.endpoints.clone(),
            spec: self.spec,
        }
    }
}

impl<T> fmt::Debug for ApiQuery<T> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("ApiQuery")
            .field("key", &self.spec.key)
            .finish()
    }
}

impl<T> ApiQuery<T> {
    pub fn url(&self) -> String {
        self.endpoints.api(self.spec.path)
    }
}

impl<T: Send + Sync + 'static> Query for ApiQuery<T> {
    type Output = T;

    fn key(&self) -> String {
        self.spec.key.to_owned()
    }

    fn label(&self) -> String {
        self.spec.label.to_owned()
    }

    fn freshness(&self, _: &T) -> Freshness {
        Freshness::Every(self.spec.refresh)
    }

    async fn fetch(&self, ctx: FetchCtx, _prev: Option<Arc<T>>) -> Result<T, FetchError> {
        let body = ctx.get_text(&self.url()).await?;
        (self.spec.parse)(&body)
    }
}

// --- Intraday five-minute history ---------------------------------------------

/// Today's five-minute RT prices at every node.
///
/// The first fetch downloads the rolling day (~7 MB gzipped); after that each
/// refresh downloads only the current interval (~25 KB) and appends it. A
/// missed interval (sleep, outage) or a new market day triggers a re-seed.
#[derive(Clone, Debug)]
pub struct RtIntradayQuery {
    endpoints: Arc<MisoEndpoints>,
}

impl RtIntradayQuery {
    async fn seed(&self, ctx: &FetchCtx) -> Result<RtIntraday, FetchError> {
        let body = ctx
            .get_text(&self.endpoints.api(paths::RT_FIVE_MIN_ROLLING))
            .await?;
        Ok(RtIntraday::from_rows(parse::parse_rt_five_min(&body)?))
    }
}

impl Query for RtIntradayQuery {
    type Output = RtIntraday;

    fn key(&self) -> String {
        "miso/rt-intraday".into()
    }

    fn label(&self) -> String {
        "RT five-minute LMPs, today".into()
    }

    fn freshness(&self, _: &RtIntraday) -> Freshness {
        Freshness::Every(REALTIME_REFRESH)
    }

    async fn fetch(
        &self,
        ctx: FetchCtx,
        prev: Option<Arc<RtIntraday>>,
    ) -> Result<RtIntraday, FetchError> {
        let today = market_today();
        let Some(prev) =
            prev.filter(|p| p.market_day == Some(today) && p.latest_interval().is_some())
        else {
            return self.seed(&ctx).await;
        };
        let body = ctx
            .get_text(&self.endpoints.api(paths::RT_FIVE_MIN_CURRENT))
            .await?;
        let rows = parse::parse_rt_five_min(&body)?;
        let gap = match (
            prev.latest_interval(),
            rows.iter().map(|r| r.interval).min(),
        ) {
            (Some(have), Some(got)) => got - have > TimeDelta::minutes(5),
            _ => false,
        };
        if gap {
            ctx.events()
                .info("intraday history has a gap; re-seeding from the rolling feed");
            return self.seed(&ctx).await;
        }
        let mut next = Arc::unwrap_or_clone(prev);
        next.merge_rows(rows);
        Ok(next)
    }
}

/// The previous market day's five-minute RT prices (MISO's `Previous` feed).
/// Keyed by today's date, so it is fetched once per day and only on demand.
#[derive(Clone, Debug)]
pub struct RtPreviousDayQuery {
    endpoints: Arc<MisoEndpoints>,
}

impl Query for RtPreviousDayQuery {
    type Output = RtIntraday;

    fn key(&self) -> String {
        format!("miso/rt-previous/{}", market_today())
    }

    fn label(&self) -> String {
        "RT five-minute LMPs, yesterday".into()
    }

    fn freshness(&self, _: &RtIntraday) -> Freshness {
        Freshness::Forever
    }

    async fn fetch(
        &self,
        ctx: FetchCtx,
        _prev: Option<Arc<RtIntraday>>,
    ) -> Result<RtIntraday, FetchError> {
        let body = ctx
            .get_text(&self.endpoints.api(paths::RT_FIVE_MIN_PREVIOUS))
            .await?;
        Ok(RtIntraday::from_rows(parse::parse_rt_five_min(&body)?))
    }
}

// --- Daily reports ------------------------------------------------------------

fn suffix(kind: DayReportKind) -> &'static str {
    match kind {
        DayReportKind::DaExPost => reports::DA_EXPOST,
        DayReportKind::DaExAnte => reports::DA_EXANTE,
        DayReportKind::RtFinal => reports::RT_FINAL,
        DayReportKind::RtPrelim => reports::RT_PRELIM,
    }
}

/// How often to look again for a report that is not out yet.
const UNPUBLISHED_RETRY: Duration = Duration::from_secs(10 * 60);

async fn fetch_report(
    ctx: &FetchCtx,
    endpoints: &MisoEndpoints,
    kind: DayReportKind,
    day: NaiveDate,
) -> Result<Option<DayLmpReport>, FetchError> {
    let url = endpoints.report(day, suffix(kind));
    // Preliminary RT reports are superseded by final ones, so never pin them on disk.
    let body = match kind {
        DayReportKind::RtPrelim => ctx.get_text(&url).await,
        _ => ctx.get_text_immutable(&url).await,
    };
    match body {
        Ok(body) => parse::parse_day_report(kind, day, &body).map(Some),
        Err(FetchError::NotFound(_)) => Ok(None),
        Err(e) => Err(e),
    }
}

/// One daily report for one day.
#[derive(Clone, Debug)]
pub struct DayReportQuery {
    endpoints: Arc<MisoEndpoints>,
    pub kind: DayReportKind,
    pub day: NaiveDate,
}

impl Query for DayReportQuery {
    type Output = Option<DayLmpReport>;

    fn key(&self) -> String {
        format!("miso/report/{}/{}", suffix(self.kind), self.day)
    }

    fn label(&self) -> String {
        format!("{} {}", self.kind.label(), self.day)
    }

    fn freshness(&self, current: &Self::Output) -> Freshness {
        match (current, self.kind) {
            (None, _) => Freshness::Every(UNPUBLISHED_RETRY),
            (Some(_), DayReportKind::RtPrelim) => Freshness::Every(Duration::from_secs(3600)),
            (Some(_), _) => Freshness::Forever,
        }
    }

    async fn fetch(
        &self,
        ctx: FetchCtx,
        _prev: Option<Arc<Self::Output>>,
    ) -> Result<Self::Output, FetchError> {
        fetch_report(&ctx, &self.endpoints, self.kind, self.day).await
    }
}

/// RT report for a day, preferring final over preliminary.
#[derive(Clone, Debug)]
pub struct RtBestDayQuery {
    endpoints: Arc<MisoEndpoints>,
    pub day: NaiveDate,
}

impl Query for RtBestDayQuery {
    type Output = Option<DayLmpReport>;

    fn key(&self) -> String {
        format!("miso/report/rt-best/{}", self.day)
    }

    fn label(&self) -> String {
        format!("RT (final or prelim) {}", self.day)
    }

    fn freshness(&self, current: &Self::Output) -> Freshness {
        match current.as_ref().map(|r| r.kind) {
            Some(DayReportKind::RtFinal) => Freshness::Forever,
            // Prelim: check hourly whether the final report has landed.
            Some(_) => Freshness::Every(Duration::from_secs(3600)),
            None => Freshness::Every(UNPUBLISHED_RETRY),
        }
    }

    async fn fetch(
        &self,
        ctx: FetchCtx,
        _prev: Option<Arc<Self::Output>>,
    ) -> Result<Self::Output, FetchError> {
        if let Some(r) =
            fetch_report(&ctx, &self.endpoints, DayReportKind::RtFinal, self.day).await?
        {
            return Ok(Some(r));
        }
        fetch_report(&ctx, &self.endpoints, DayReportKind::RtPrelim, self.day).await
    }
}
