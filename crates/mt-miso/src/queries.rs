//! `mt_data::Query` implementations for MISO, reached through [`Miso`].
//!
//! Most datasets are "GET one JSON path, parse it, refresh every minute", which
//! [`ApiQuery`] covers with a static [`ApiSpec`]. Adding such a dataset is a
//! path in `endpoints.rs`, a parser in `parse.rs`, a spec below and a one-line
//! method on [`Miso`]. Feeds with real logic (append-only intraday history,
//! report fallbacks) get their own types.

use std::fmt;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use chrono::{NaiveDate, TimeDelta};
use mt_core::time::market_today;
use mt_core::*;
use mt_data::{DiskCache, FetchCtx, FetchError, Freshness, Query};

use crate::endpoints::{MisoEndpoints, paths, reports};
use crate::history::StoredPricesQuery;
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
    pub fn reserve_constraints(&self) -> ApiQuery<BindingConstraints> {
        self.api(&RESERVE_CONSTRAINTS)
    }
    pub fn subregional_constraints(&self) -> ApiQuery<BindingConstraints> {
        self.api(&SUBREGIONAL_CONSTRAINTS)
    }
    pub fn actual_interchange(&self) -> ApiQuery<ActualInterchange> {
        self.api(&NAI)
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
    pub fn rsg_commitments(&self) -> ApiQuery<RsgCommitments> {
        self.api(&RSG)
    }
    pub fn str_requirement(&self) -> ApiQuery<StrRequirements> {
        self.api(&STR_REQUIREMENT)
    }
    pub fn cts(&self) -> ApiQuery<Cts> {
        self.api(&CTS)
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

    /// One past day's five-minute RT prices from the local archive (`None`
    /// when that day was never saved). Nothing is downloaded.
    pub fn rt_archive_day(&self, day: NaiveDate) -> RtArchiveQuery {
        RtArchiveQuery { day }
    }

    /// Hourly prices at `nodes` for every day the day store holds from
    /// `first` on, read from disk (nothing is downloaded).
    pub fn stored_prices(&self, nodes: &[&str], first: NaiveDate) -> StoredPricesQuery {
        StoredPricesQuery::new(nodes, first)
    }

    /// A market day's binding constraints, DA or RT (`None` until published:
    /// DA about 13:30 EST the day before, RT the day after).
    pub fn constraint_history(&self, market: Market, day: NaiveDate) -> ConstraintHistoryQuery {
        ConstraintHistoryQuery {
            endpoints: self.endpoints.clone(),
            market,
            day,
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
api_spec!(NAI: ActualInterchange = "nai", "Net actual interchange", paths::NAI, parse::parse_nai);
api_spec!(
    RESERVE_CONSTRAINTS: BindingConstraints = "reserve-constraints",
    "Reserve constraints",
    paths::RESERVE_CONSTRAINTS,
    parse::parse_binding_constraints
);
api_spec!(
    SUBREGIONAL_CONSTRAINTS: BindingConstraints = "subregional-constraints",
    "Sub-regional constraints",
    paths::SUBREGIONAL_CONSTRAINTS,
    parse::parse_binding_constraints
);

api_spec!(RSG: RsgCommitments = "rsg", "RT RSG commitments", paths::RSG_COMMITMENTS, parse::parse_rsg);
/// Set once a day for the next day; no need to poll every minute.
static STR_REQUIREMENT: ApiSpec<StrRequirements> = ApiSpec {
    key: "miso/str-requirement",
    label: "Next-day STR requirement",
    path: paths::STR_REQUIREMENT,
    refresh: Duration::from_secs(15 * 60),
    parse: parse::parse_str_requirement,
};
/// PJM approves a new CTS case about every 15 minutes.
static CTS: ApiSpec<Cts> = ApiSpec {
    key: "miso/cts",
    label: "CTS: PJM interface forecast",
    path: paths::CTS,
    refresh: Duration::from_secs(5 * 60),
    parse: parse::parse_cts,
};

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
        let store = RtIntraday::from_rows(parse::parse_rt_five_min(&body)?);
        // A complete day fills in the archive, which may only have the part of
        // the day the app was running for.
        if let Some(day) = store.market_day {
            let key = intraday_archive_key(day);
            let have = store.intervals().len();
            let saved = ctx
                .local_get(&key)
                .await
                .and_then(|b| RtIntraday::from_bytes(&b))
                .map_or(0, |s| s.intervals().len());
            if have > saved && ctx.local_put(&key, store.to_bytes()).await {
                ctx.events()
                    .info(format!("archived {have} five-minute intervals for {day}"));
            }
        }
        Ok(store)
    }
}

/// Where a market day's five-minute store lives in the disk cache. Today's is
/// written every few minutes while the app runs; earlier days form the archive.
pub fn intraday_archive_key(day: NaiveDate) -> String {
    format!("local://intraday/{day}")
}

/// The directory holding the archive inside `cache` (exempt from the size cap).
pub fn archive_dir(cache: &DiskCache) -> PathBuf {
    cache
        .path_for(&intraday_archive_key(NaiveDate::default()))
        .parent()
        .map(Path::to_path_buf)
        .unwrap_or_default()
}

/// Delete archived days more than `keep_days` before `today` (0 keeps
/// everything). Returns how many days were removed.
pub fn prune_archive(cache: &DiskCache, keep_days: u32, today: NaiveDate) -> usize {
    if keep_days == 0 {
        return 0;
    }
    let cutoff = today - TimeDelta::days(i64::from(keep_days));
    let Ok(entries) = std::fs::read_dir(archive_dir(cache)) else {
        return 0;
    };
    entries
        .flatten()
        .filter(|e| {
            e.file_name()
                .to_str()
                .and_then(|n| n.strip_suffix(".gz"))
                .and_then(|d| NaiveDate::parse_from_str(d, "%Y-%m-%d").ok())
                .is_some_and(|d| d < cutoff)
        })
        .filter(|e| std::fs::remove_file(e.path()).is_ok())
        .count()
}

/// Where `tools/export_history.py` (retired in 0.3.0) wrote nodes' long
/// history in the disk cache. The price history replaces it, and
/// [`discard_exported_history`] removes what is left.
fn exported_history_dir(cache: &DiskCache) -> PathBuf {
    cache
        .path_for("local://archive/lmp/X")
        .parent()
        .map(Path::to_path_buf)
        .unwrap_or_default()
}

/// Remove the files the retired `tools/export_history.py` exported, at
/// launch. Returns how many there were. Blocking.
pub fn discard_exported_history(cache: &DiskCache) -> usize {
    let dir = exported_history_dir(cache);
    let files = std::fs::read_dir(&dir).map_or(0, Iterator::count);
    match files > 0 && std::fs::remove_dir_all(&dir).is_ok() {
        true => files,
        false => 0,
    }
}

/// Five-minute intervals in a complete market day.
pub const INTERVALS_PER_DAY: usize = 288;

/// A past day's five-minute store, read from the local archive. Days the app
/// saw only part of are re-read now and then, in case the previous-day feed
/// has since completed them.
#[derive(Clone, Debug)]
pub struct RtArchiveQuery {
    day: NaiveDate,
}

impl Query for RtArchiveQuery {
    type Output = Option<RtIntraday>;

    fn key(&self) -> String {
        format!("archive/rt/{}", self.day)
    }

    fn label(&self) -> String {
        format!("RT five-minute archive, {}", self.day)
    }

    fn freshness(&self, value: &Option<RtIntraday>) -> Freshness {
        match value {
            Some(s) if s.intervals().len() >= INTERVALS_PER_DAY => Freshness::Forever,
            _ => Freshness::Every(Duration::from_secs(5 * 60)),
        }
    }

    async fn fetch(
        &self,
        ctx: FetchCtx,
        _prev: Option<Arc<Option<RtIntraday>>>,
    ) -> Result<Option<RtIntraday>, FetchError> {
        Ok(ctx
            .local_get(&intraday_archive_key(self.day))
            .await
            .and_then(|b| RtIntraday::from_bytes(&b))
            .filter(|s| s.market_day == Some(self.day)))
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

/// Where the day store keeps a market day's report: every node's hourly LMP,
/// MCC and MLC, DA ex-post or RT (final, or preliminary until the final one
/// appears). Exempt from the cache's size cap, like the five-minute archive.
pub fn day_store_key(market: Market, day: NaiveDate) -> String {
    let market = match market {
        Market::DayAhead => "da",
        Market::RealTime => "rt",
    };
    format!("local://archive/report/{market}/{day}")
}

/// The day store's directory in `cache`.
pub fn day_store_dir(cache: &DiskCache) -> PathBuf {
    cache
        .path_for(&day_store_key(Market::DayAhead, NaiveDate::default()))
        .parent()
        .and_then(Path::parent)
        .map(Path::to_path_buf)
        .unwrap_or_default()
}

/// The reports the day store keeps (not DA ex-ante, which ex-post replaces).
fn stored(kind: DayReportKind) -> bool {
    kind != DayReportKind::DaExAnte
}

/// A market day's report from the day store, if it has one.
pub async fn read_day_store(
    ctx: &FetchCtx,
    market: Market,
    day: NaiveDate,
) -> Option<DayLmpReport> {
    ctx.local_get(&day_store_key(market, day))
        .await
        .and_then(|b| DayLmpReport::from_bytes(&b))
        .filter(|r| r.day == day && r.kind.market() == market)
}

/// One daily report: from the day store when it holds a settled one,
/// otherwise downloaded (and kept, for a live file naming `day`). `None` until
/// MISO publishes it.
pub(crate) async fn fetch_report(
    ctx: &FetchCtx,
    endpoints: &MisoEndpoints,
    kind: DayReportKind,
    day: NaiveDate,
) -> Result<Option<DayLmpReport>, FetchError> {
    let kept = match stored(kind) {
        true => read_day_store(ctx, kind.market(), day).await,
        false => None,
    };
    // Settled reports never change; a preliminary one is asked for again.
    if let Some(r) = &kept
        && r.kind == kind
        && kind != DayReportKind::RtPrelim
    {
        return Ok(kept);
    }
    let url = endpoints.report(day, suffix(kind));
    // The day store keeps what it parses, so a settled report is not kept a
    // second time, in memory or on disk; a preliminary one comes round again,
    // and is asked "changed since?" when it does.
    let body = match kind {
        DayReportKind::DaExAnte => ctx.get_text_immutable(&url).await,
        DayReportKind::RtPrelim => ctx.get_text(&url).await,
        DayReportKind::DaExPost | DayReportKind::RtFinal => ctx.get_text_uncached(&url).await,
    };
    let body = match body {
        Ok(body) => body,
        Err(FetchError::NotFound(_)) => return Ok(None),
        Err(e) => return Err(e),
    };
    let report = parse::parse_day_report(kind, day, &body)?;
    let named = parse::day_report_date(&body);
    // Replayed fixtures stand in for any date; live files must match.
    if let Some(named) = named
        && named != day
        && ctx.is_live()
    {
        return Err(FetchError::parse(
            format!("{} {day}", kind.label()),
            format!("the file is for {named}"),
        ));
    }
    // Only live files go in the store (replayed fixtures are trimmed to a few
    // nodes), only for the day they name, and a preliminary report never
    // replaces a final one.
    let downgrade = kind == DayReportKind::RtPrelim
        && kept
            .as_ref()
            .is_some_and(|k| k.kind == DayReportKind::RtFinal);
    if stored(kind) && ctx.is_live() && named == Some(day) && !downgrade {
        let bytes = report.to_bytes();
        let unchanged = kept.is_some_and(|k| k.to_bytes() == bytes);
        if !unchanged
            && ctx
                .local_put(&day_store_key(kind.market(), day), bytes)
                .await
        {
            ctx.events()
                .info(format!("kept {} for {day} in the day store", kind.label()));
        }
    }
    Ok(Some(report))
}

/// A market day's binding-constraints report.
#[derive(Clone, Debug)]
pub struct ConstraintHistoryQuery {
    endpoints: Arc<MisoEndpoints>,
    pub market: Market,
    pub day: NaiveDate,
}

impl ConstraintHistoryQuery {
    /// MISO dates the DA report the day before its market day and the RT one
    /// the day after.
    pub fn url(&self) -> String {
        let (file_day, suffix) = match self.market {
            Market::DayAhead => (self.day - TimeDelta::days(1), reports::DA_BC),
            Market::RealTime => (self.day + TimeDelta::days(1), reports::RT_BC),
        };
        self.endpoints.report_file(file_day, suffix, "xls")
    }
}

impl Query for ConstraintHistoryQuery {
    type Output = Option<ConstraintHistory>;

    fn key(&self) -> String {
        format!("miso/constraints/{}/{}", self.market.label(), self.day)
    }

    fn label(&self) -> String {
        format!("{} binding constraints {}", self.market.label(), self.day)
    }

    fn freshness(&self, current: &Self::Output) -> Freshness {
        match current {
            None => Freshness::Every(UNPUBLISHED_RETRY),
            Some(_) => Freshness::Forever,
        }
    }

    async fn fetch(
        &self,
        ctx: FetchCtx,
        _prev: Option<Arc<Self::Output>>,
    ) -> Result<Self::Output, FetchError> {
        let body = match ctx.get_immutable(&self.url()).await {
            Ok(body) => body,
            Err(FetchError::NotFound(_)) => return Ok(None),
            Err(e) => return Err(e),
        };
        let history = parse::parse_constraint_history(self.market, &body)?;
        // Replayed fixtures stand in for any date; live files must match.
        if history.day != self.day && ctx.is_live() {
            return Err(FetchError::parse(
                self.label(),
                format!("the file is for {}", history.day),
            ));
        }
        Ok(Some(history))
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
