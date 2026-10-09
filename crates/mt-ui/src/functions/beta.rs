//! BETA: how a security moves with the S&P 500 and with its industry. Beta,
//! adjusted beta, alpha, R², correlation and beta's standard error side by
//! side, a scatter of returns with the fitted line, and a rolling beta, from
//! total returns (bars adjusted for dividends) over one, two or five years.
//! The maths is in `mt_core::beta`; the benchmarks in `[markets]`.

use std::collections::HashMap;

use chrono::NaiveDate;
use egui::{Grid, RichText, Ui};
use egui_plot::{Corner, HLine, Legend, Line, LineStyle, Plot, PlotPoints, Points, VLine};
use mt_alpaca::{Adjustment, BarSet, Benchmark, MARKET_BENCHMARK, Timeframe};
use mt_core::beta::{self, Close, Fit, Frequency, Pair};
use mt_core::exchange;
use mt_core::instrument::Security;

use crate::context::PanelCx;
use crate::function::{Category, FunctionSpec, Panel, Route};
use crate::market::{self, SecurityPicker, fmt};
use crate::widgets::chart::{exchange_plot, utc_x};
use crate::widgets::{self, csv};

pub const SPEC: FunctionSpec = FunctionSpec {
    code: "BETA",
    aliases: &["BETAS"],
    name: "Beta",
    category: Category::Markets,
    usage: "BETA <security> [benchmark|list] [1Y|2Y|5Y] [DAILY|WEEKLY]",
    description: "How a security moves with the S&P 500 and its industry: beta, adjusted beta, alpha, R², correlation and beta's standard error side by side, a scatter of returns with the fitted line, and a rolling beta, from total returns over one, two or five years. A second security, or a list's name for an equal-weighted basket of it, names the industry benchmark.",
    takes_node: false,
    takes_security: true,
    takes_option: false,
    open,
};

/// The windows offered, in years.
const WINDOWS: [u32; 3] = [1, 2, 5];

/// Calendar days of bars fetched: five years and a rolling window before
/// them, so every window and frequency comes from one download.
const FETCH_DAYS: i64 = 5 * 366 + 200;

fn open(args: &[String]) -> Result<Box<dyn Panel>, String> {
    let mut security = None;
    let mut benchmark = None;
    let mut years = None;
    let mut frequency = None;
    for arg in args {
        let word = arg.trim().to_ascii_uppercase();
        if let Some(y) = word
            .strip_suffix('Y')
            .and_then(|y| y.parse::<u32>().ok())
            .filter(|y| WINDOWS.contains(y))
        {
            years = Some(y);
        } else if word == "DAILY" || word == "D" {
            frequency = Some(Frequency::Daily);
        } else if word == "WEEKLY" || word == "W" {
            frequency = Some(Frequency::Weekly);
        } else if security.is_none()
            && let Some(s) = market::security_of(arg)
        {
            security = Some(s);
        } else if benchmark.is_none() && !word.is_empty() {
            // A security, or a list's name: resolved against the config.
            benchmark = Some(market::security_of(arg).map_or(word, |s| s.to_string()));
        } else {
            return Err(format!(
                "{arg:?} is not a window (1Y, 2Y, 5Y) or DAILY or WEEKLY, and the benchmark is already named"
            ));
        }
    }
    let years = years.unwrap_or(1);
    Ok(Box::new(BetaPanel {
        security,
        benchmark,
        years,
        frequency: frequency.unwrap_or(Frequency::default_for(years)),
        scatter: Side::Market,
        picker: SecurityPicker::default(),
        cache: None,
    }))
}

/// Which comparison the scatter shows.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Side {
    Market,
    Industry,
}

struct BetaPanel {
    security: Option<Security>,
    /// Named in the route: a security or a list's name. Otherwise the
    /// industry benchmark comes from `[markets]`.
    benchmark: Option<String>,
    years: u32,
    frequency: Frequency,
    scatter: Side,
    picker: SecurityPicker,
    /// The last analysis and what it was computed from.
    cache: Option<(CacheKey, Analysis)>,
}

#[derive(Clone, PartialEq)]
struct CacheKey {
    query: String,
    bars: usize,
    last: Option<chrono::DateTime<chrono::Utc>>,
    years: u32,
    frequency: Frequency,
    industry: Option<Benchmark>,
}

/// One comparison: the paired returns in the window, their fit and the
/// rolling beta.
struct Comparison {
    label: String,
    pairs: Vec<Pair>,
    fit: Option<Fit>,
    rolling: Vec<(NaiveDate, f64)>,
    /// A basket's members without bars.
    missing: Vec<String>,
}

struct Analysis {
    market: Option<Comparison>,
    industry: Option<Comparison>,
    /// The first and last closes of the security in the window.
    span: Option<(NaiveDate, NaiveDate)>,
}

/// Compare `security`'s closes with `benchmark`'s over the window from
/// `first`, at `frequency`; the rolling beta starts a window earlier so it
/// covers the whole span.
fn compare(
    label: String,
    security: &[Close],
    benchmark: &[Close],
    first: NaiveDate,
    frequency: Frequency,
) -> Comparison {
    let resample = |c: &[Close]| match frequency {
        Frequency::Daily => c.to_vec(),
        Frequency::Weekly => beta::weekly(c),
    };
    let all = beta::paired_returns(&resample(security), &resample(benchmark));
    let pairs: Vec<Pair> = all.iter().copied().filter(|p| p.date > first).collect();
    let rolling = beta::rolling(&all, frequency.rolling_window())
        .into_iter()
        .filter(|(d, _)| *d > first)
        .collect();
    Comparison {
        label,
        fit: beta::fit(&pairs),
        pairs,
        rolling,
        missing: Vec::new(),
    }
}

fn analyse(
    set: &BarSet,
    security: &Security,
    industry: Option<&Benchmark>,
    first: NaiveDate,
    frequency: Frequency,
) -> Analysis {
    let closes: HashMap<&str, Vec<Close>> = set
        .bars
        .iter()
        .map(|(sym, bars)| (sym.as_str(), beta::daily_closes(bars)))
        .collect();
    let empty = Vec::new();
    let of = |sym: &str| closes.get(sym).unwrap_or(&empty);
    let own = of(&security.ticker);
    let market = (security.ticker != MARKET_BENCHMARK).then(|| {
        compare(
            format!("S&P 500 ({MARKET_BENCHMARK} US)"),
            own,
            of(MARKET_BENCHMARK),
            first,
            frequency,
        )
    });
    let industry = industry.map(|b| match b {
        Benchmark::Security(s) => compare(s.to_string(), own, of(&s.ticker), first, frequency),
        Benchmark::Basket { .. } => {
            let tickers = b.tickers();
            let (have, missing): (Vec<&String>, Vec<&String>) =
                tickers.iter().partition(|t| !of(t).is_empty());
            let members: Vec<&[Close]> = have.iter().map(|t| of(t).as_slice()).collect();
            let mut c = compare(b.label(), own, &beta::basket(&members), first, frequency);
            c.missing = missing.into_iter().cloned().collect();
            c
        }
    });
    let shown = beta::since(own, first);
    Analysis {
        market,
        industry,
        span: shown.first().zip(shown.last()).map(|(a, b)| (a.0, b.0)),
    }
}

impl Panel for BetaPanel {
    fn title(&self) -> String {
        match &self.security {
            Some(s) => format!("BETA {s}"),
            None => "BETA".into(),
        }
    }

    fn route(&self) -> Route {
        let mut args: Vec<String> = self.security.iter().map(ToString::to_string).collect();
        args.extend(self.benchmark.iter().cloned());
        if self.years != 1 {
            args.push(format!("{}Y", self.years));
        }
        if self.frequency != Frequency::default_for(self.years) {
            args.push(self.frequency.label().to_ascii_uppercase());
        }
        Route::new("BETA", args)
    }

    fn ui(&mut self, ui: &mut Ui, cx: &mut PanelCx<'_>) {
        let skin = cx.skin;
        let assets = market::assets(cx);
        ui.horizontal_wrapped(|ui| {
            if let Some(sec) = &self.security {
                ui.label(
                    RichText::new(sec.to_string())
                        .heading()
                        .color(skin.text_strong),
                );
                if let Some(name) = market::name_of(&assets, &sec.ticker) {
                    ui.label(RichText::new(name).color(skin.text_muted));
                }
                for code in ["DES", "GP"] {
                    if widgets::link(ui, skin, code).clicked() {
                        cx.open(Route::new(code, [sec.to_string()]));
                    }
                }
            }
            if let Some(s) = self.picker.show(ui, cx, "beta-sec", "another security…") {
                self.security = Some(s);
            }
        });
        let Some(security) = self.security.clone() else {
            ui.add_space(8.0);
            ui.label(
                RichText::new("Pick a security above, or type BETA XEL US on the command line.")
                    .color(skin.text_muted),
            );
            return;
        };
        if market::needs_keys(ui, cx) {
            return;
        }
        ui.horizontal_wrapped(|ui| {
            for years in WINDOWS {
                if ui
                    .selectable_label(self.years == years, format!("{years}Y"))
                    .clicked()
                {
                    self.years = years;
                    self.frequency = Frequency::default_for(years);
                }
            }
            ui.separator();
            ui.selectable_value(&mut self.frequency, Frequency::Daily, "Daily");
            ui.selectable_value(&mut self.frequency, Frequency::Weekly, "Weekly");
        });

        let markets = &cx.config.markets;
        let industry = match &self.benchmark {
            Some(spec) => markets.benchmark(spec, &security),
            None => markets.industry(&security),
        };
        let mut tickers = vec![security.ticker.clone(), MARKET_BENCHMARK.to_owned()];
        tickers.extend(industry.iter().flat_map(Benchmark::tickers));
        tickers.sort();
        tickers.dedup();
        let today = exchange::now_exchange().date_naive();
        let query = cx
            .alpaca
            .bars(
                &tickers,
                Timeframe::Day1,
                today - chrono::Duration::days(FETCH_DAYS),
                None,
            )
            .adjusted(Adjustment::All);
        let snap = cx.hub.watch(&query);
        let Some(set) = snap.data() else {
            widgets::placeholder(ui, skin, snap.error.as_ref().map(ToString::to_string));
            return;
        };
        let first = today - chrono::Duration::days(i64::from(self.years) * 365);
        let key = CacheKey {
            query: mt_data::Query::key(&query),
            bars: set.bars.values().map(Vec::len).sum(),
            last: set
                .bars
                .values()
                .filter_map(|b| b.last())
                .map(|b| b.time)
                .max(),
            years: self.years,
            frequency: self.frequency,
            industry: industry.clone(),
        };
        if self.cache.as_ref().is_none_or(|(k, _)| *k != key) {
            let analysis = analyse(set, &security, industry.as_ref(), first, self.frequency);
            self.cache = Some((key, analysis));
        }
        let Some((_, analysis)) = &self.cache else {
            return;
        };

        notes(ui, cx, &security, self, industry.as_ref(), analysis, set);
        ui.add_space(4.0);
        table(ui, cx, self.frequency, analysis);
        ui.add_space(6.0);

        let comparisons: Vec<(Side, &Comparison)> = [
            (Side::Market, analysis.market.as_ref()),
            (Side::Industry, analysis.industry.as_ref()),
        ]
        .into_iter()
        .filter_map(|(s, c)| Some((s, c?)))
        .collect();
        if comparisons.is_empty() {
            return;
        }
        if !comparisons.iter().any(|(s, _)| *s == self.scatter) {
            self.scatter = comparisons[0].0;
        }
        let height = ui.available_height().max(160.0);
        let (scatter, frequency, span) = (&mut self.scatter, self.frequency, analysis.span);
        if ui.available_width() >= 760.0 {
            ui.columns(2, |cols| {
                scatter_plot(&mut cols[0], cx, &security, &comparisons, scatter, height);
                rolling_plot(
                    &mut cols[1],
                    cx,
                    &security,
                    &comparisons,
                    frequency,
                    span,
                    height,
                );
            });
        } else {
            let half = (height / 2.0 - 4.0).max(140.0);
            scatter_plot(ui, cx, &security, &comparisons, scatter, half);
            rolling_plot(ui, cx, &security, &comparisons, frequency, span, half);
        }
    }
}

/// What the figures are made of, and anything missing.
fn notes(
    ui: &mut Ui,
    cx: &PanelCx<'_>,
    security: &Security,
    panel: &BetaPanel,
    industry: Option<&Benchmark>,
    analysis: &Analysis,
    set: &BarSet,
) {
    let skin = cx.skin;
    let muted = |text: String| RichText::new(text).small().color(skin.text_muted);
    ui.horizontal_wrapped(|ui| {
        let span = analysis.span.map_or_else(String::new, |(a, b)| {
            format!(" · {} to {}", a.format("%b %d %Y"), b.format("%b %d %Y"))
        });
        ui.label(muted(format!(
            "Total returns (dividends reinvested), {}{span} · alpha is over zero, not over a risk-free rate",
            panel.frequency.label()
        )));
        // The returns the scatter shows.
        let shown = match panel.scatter {
            Side::Market => analysis.market.as_ref(),
            Side::Industry => analysis.industry.as_ref(),
        };
        if let Some(c) = shown.or(analysis.market.as_ref()) {
            csv::copy_button(ui, skin, || {
                csv::to_csv(
                    &["date", "security_return", "benchmark_return", "benchmark"],
                    c.pairs.iter().map(|p| {
                        vec![
                            p.date.to_string(),
                            p.security.to_string(),
                            p.benchmark.to_string(),
                            c.label.clone(),
                        ]
                    }),
                )
            });
        }
    });
    if let Some((from, _)) = analysis.span {
        let wanted = exchange::now_exchange().date_naive()
            - chrono::Duration::days(i64::from(panel.years) * 365);
        if from > wanted + chrono::Duration::days(10) {
            ui.label(
                RichText::new(format!(
                    "{security}'s bars start on {}: less than {} year{}.",
                    from.format("%b %d %Y"),
                    panel.years,
                    if panel.years == 1 { "" } else { "s" }
                ))
                .small()
                .color(skin.warning),
            );
        }
    } else {
        ui.label(
            RichText::new(format!("No daily bars for {security} in this window."))
                .color(skin.warning),
        );
    }
    if set.truncated {
        ui.label(muted(
            "More bars than one request returns; the oldest are cut.".into(),
        ));
    }
    if let Some(c) = &analysis.industry
        && !c.missing.is_empty()
    {
        ui.label(muted(format!(
            "No bars for {} in the basket; it averages the others.",
            c.missing.join(", ")
        )));
    }
    match (&panel.benchmark, industry) {
        (Some(spec), None) => {
            ui.label(
                RichText::new(format!(
                    "{spec} is neither a security nor a list's name (other than {security} itself)."
                ))
                .small()
                .color(skin.warning),
            );
        }
        (None, None) => {
            ui.label(muted(format!(
                "No industry benchmark for {security}: name one (BETA {security} XLU US), a list \
                 for a basket of it (BETA {security} UTILITIES), or set one under \
                 [markets.benchmarks] in config.toml."
            )));
        }
        _ => {}
    }
    if analysis.market.is_none() {
        ui.label(muted(format!(
            "{security} is the S&P 500 benchmark itself."
        )));
    }
}

/// The figures, one column per benchmark.
fn table(ui: &mut Ui, cx: &PanelCx<'_>, frequency: Frequency, analysis: &Analysis) {
    let skin = cx.skin;
    let columns: Vec<&Comparison> = [analysis.market.as_ref(), analysis.industry.as_ref()]
        .into_iter()
        .flatten()
        .collect();
    if columns.is_empty() {
        return;
    }
    type Row = (&'static str, &'static str, fn(&Fit, Frequency) -> String);
    let rows: [Row; 7] = [
        (
            "Beta",
            "How far the security moves, on average, when the benchmark moves 1%",
            |f, _| format!("{:.2}", f.beta),
        ),
        (
            "Adjusted beta",
            "Two thirds of beta plus one third of 1: betas drift towards 1 over time",
            |f, _| format!("{:.2}", f.adjusted_beta()),
        ),
        (
            "Alpha, a year",
            "The return left over when the benchmark is flat, per period times the periods in a year; over zero, not over a risk-free rate",
            |f, q| format!("{:+.1}%", f.annual_alpha(q) * 100.0),
        ),
        (
            "R²",
            "The share of the security's variance the benchmark explains",
            |f, _| format!("{:.2}", f.r_squared),
        ),
        (
            "Correlation",
            "How closely the two move together, from −1 to 1",
            |f, _| format!("{:+.2}", f.correlation),
        ),
        (
            "Beta's standard error",
            "How uncertain the beta is: about two of these either side covers it 95% of the time",
            |f, _| format!("{:.3}", f.beta_error),
        ),
        ("Observations", "Returns in the fit", |f, q| {
            format!("{} {}", f.observations, q.label())
        }),
    ];
    Grid::new("beta-table")
        .num_columns(columns.len() + 1)
        .striped(true)
        .spacing([28.0, 4.0])
        .show(ui, |ui| {
            ui.label("");
            for c in &columns {
                ui.label(RichText::new(&c.label).strong().color(skin.text_strong));
            }
            ui.end_row();
            for (label, help, value) in rows {
                ui.label(RichText::new(label).color(skin.text_muted))
                    .on_hover_text(help);
                for c in &columns {
                    let text = c
                        .fit
                        .as_ref()
                        .map_or_else(|| fmt::DASH.to_owned(), |f| value(f, frequency));
                    ui.label(RichText::new(text).monospace().color(skin.text));
                }
                ui.end_row();
            }
        });
}

/// The security's returns against one benchmark's, in percent, with the
/// fitted line.
fn scatter_plot(
    ui: &mut Ui,
    cx: &PanelCx<'_>,
    security: &Security,
    comparisons: &[(Side, &Comparison)],
    shown: &mut Side,
    height: f32,
) {
    let skin = cx.skin;
    ui.horizontal_wrapped(|ui| {
        widgets::label(ui, skin, "Returns against");
        for (side, c) in comparisons {
            ui.selectable_value(shown, *side, c.label.as_str());
        }
    });
    let Some((side, c)) = comparisons.iter().find(|(s, _)| s == shown) else {
        return;
    };
    let color = skin.series(if *side == Side::Market { 0 } else { 1 });
    let points: Vec<[f64; 2]> = c
        .pairs
        .iter()
        .map(|p| [p.benchmark * 100.0, p.security * 100.0])
        .filter(|p| p[0].is_finite() && p[1].is_finite())
        .collect();
    let (lo, hi) = points
        .iter()
        .map(|p| p[0])
        .fold((f64::MAX, f64::MIN), |(lo, hi), x| (lo.min(x), hi.max(x)));
    Plot::new(("beta-scatter", security.ticker.as_str()))
        .height((height - 28.0).max(120.0))
        .grid_color(skin.grid)
        .x_axis_label(format!("{}, %", c.label))
        .y_axis_label(format!("{security}, %"))
        .allow_scroll(false)
        .legend(
            Legend::default()
                .position(Corner::LeftTop)
                .background_alpha(0.8)
                .follow_insertion_order(true),
        )
        .show(ui, |plot| {
            plot.hline(HLine::new("", 0.0).color(skin.border_strong).width(1.0));
            plot.vline(VLine::new("", 0.0).color(skin.border_strong).width(1.0));
            plot.points(
                Points::new("Returns", PlotPoints::from(points.clone()))
                    .radius(2.0)
                    .color(color.gamma_multiply(0.8)),
            );
            if let Some(f) = c.fit
                && lo < hi
            {
                // Both axes are in percent, so the intercept is too.
                let at = |x: f64| [x, f.alpha * 100.0 + f.beta * x];
                plot.line(
                    Line::new(
                        format!("Fit: beta {:.2}", f.beta),
                        PlotPoints::from(vec![at(lo), at(hi)]),
                    )
                    .color(skin.text_strong)
                    .width(1.6),
                );
            }
        });
}

/// Beta over a rolling window, against each benchmark.
fn rolling_plot(
    ui: &mut Ui,
    cx: &PanelCx<'_>,
    security: &Security,
    comparisons: &[(Side, &Comparison)],
    frequency: Frequency,
    span: Option<(NaiveDate, NaiveDate)>,
    height: f32,
) {
    let skin = cx.skin;
    let window = frequency.rolling_window();
    widgets::label(
        ui,
        skin,
        &match frequency {
            Frequency::Daily => format!("Rolling beta, {window} days"),
            Frequency::Weekly => format!("Rolling beta, {window} weeks"),
        },
    );
    let x = |d: NaiveDate| {
        d.and_hms_opt(0, 0, 0)
            .and_then(exchange::exchange_to_utc)
            .map(utc_x)
    };
    let mut plot = exchange_plot(&format!("beta-rolling-{}", security.ticker), skin)
        .height((height - 28.0).max(120.0))
        .legend(
            Legend::default()
                .position(Corner::LeftTop)
                .background_alpha(0.8)
                .follow_insertion_order(true),
        );
    // The whole window, even where a short history starts the rolling beta late.
    for d in span.iter().flat_map(|(a, b)| [*a, *b]) {
        if let Some(x) = x(d) {
            plot = plot.include_x(x);
        }
    }
    plot.show(ui, |plot| {
        plot.hline(
            HLine::new("", 1.0)
                .color(skin.border_strong)
                .style(LineStyle::dashed_dense())
                .width(1.0),
        );
        plot.hline(HLine::new("", 0.0).color(skin.border_strong).width(1.0));
        for (side, c) in comparisons {
            let points: Vec<[f64; 2]> = c
                .rolling
                .iter()
                .filter_map(|(d, b)| Some([x(*d)?, *b]))
                .filter(|p| p[1].is_finite())
                .collect();
            let color = skin.series(if *side == Side::Market { 0 } else { 1 });
            plot.line(
                Line::new(format!("vs {}", c.label), PlotPoints::from(points))
                    .color(color)
                    .width(1.6),
            );
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    fn route(args: &[&str]) -> Result<String, String> {
        let args: Vec<String> = args.iter().map(|a| (*a).to_owned()).collect();
        open(&args).map(|p| p.route().to_string())
    }

    #[test]
    fn routes_carry_the_benchmark_window_and_frequency() {
        assert_eq!(route(&["XLU US"]).unwrap(), "BETA XLU US");
        assert_eq!(
            route(&["VST US", "generators"]).unwrap(),
            "BETA VST US GENERATORS"
        );
        assert_eq!(
            route(&["XLU US", "XLE US", "2y"]).unwrap(),
            "BETA XLU US XLE US 2Y"
        );
        // Weekly is the default from two years, so only daily is written.
        assert_eq!(
            route(&["XLU US", "5Y", "weekly"]).unwrap(),
            "BETA XLU US 5Y"
        );
        assert_eq!(
            route(&["XLU US", "5Y", "daily"]).unwrap(),
            "BETA XLU US 5Y DAILY"
        );
        assert_eq!(route(&["XLU US", "W"]).unwrap(), "BETA XLU US WEEKLY");
        assert_eq!(route(&[]).unwrap(), "BETA");
        assert!(route(&["XLU US", "XLE US", "SPY US"]).is_err());
    }

    fn closes(start: &str, values: &[f64]) -> Vec<Close> {
        let d: NaiveDate = start.parse().unwrap();
        values
            .iter()
            .enumerate()
            .map(|(i, v)| (d + chrono::Duration::days(i as i64), *v))
            .collect()
    }

    #[test]
    fn comparisons_fit_inside_the_window_and_roll_from_before_it() {
        // The security moves twice as far as the benchmark, every day.
        let mut bench = vec![100.0];
        let mut sec = vec![50.0];
        for i in 1..200 {
            let r = 0.01 * ((i as f64) * 0.9).sin();
            bench.push(bench[i - 1] * (1.0 + r));
            sec.push(sec[i - 1] * (1.0 + 2.0 * r));
        }
        let (s, b) = (closes("2026-01-01", &sec), closes("2026-01-01", &bench));
        let first: NaiveDate = "2026-05-01".parse().unwrap();
        let c = compare("B".into(), &s, &b, first, Frequency::Daily);
        assert!(c.pairs.iter().all(|p| p.date > first));
        assert_eq!(c.pairs.len(), 79, "May 2 to July 19");
        let f = c.fit.unwrap();
        assert!((f.beta - 2.0).abs() < 1e-9 && f.r_squared > 0.999_999);
        // The rolling beta covers the whole window, from its first day.
        assert_eq!(
            c.rolling.first().map(|r| r.0),
            Some(first.succ_opt().unwrap())
        );
        assert!(c.rolling.iter().all(|(_, b)| (b - 2.0).abs() < 1e-9));
        let w = compare("B".into(), &s, &b, first, Frequency::Weekly);
        assert!(w.pairs.len() < 15 && w.fit.is_some());
    }
}
