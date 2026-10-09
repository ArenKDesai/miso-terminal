//! Studies over a security's prices: the moving averages, Bollinger bands,
//! momentum, rate of change, RSI and MACD that GP draws over and under a
//! security's chart.
//!
//! Every study takes closes oldest first and returns one value per close:
//! `None` until it has seen enough bars, then a value. Studies count bars, not
//! time: on Alpaca's feed a minute without a trade has no bar, so a 20-period
//! intraday average of a thinly traded name can span more than twenty
//! minutes. [`Study::lookback`] says how many bars before a window to fetch so
//! every value inside it is defined.
//!
//! The conventions are the studies' authors': Bollinger bands use the
//! population standard deviation, exponential averages start from the simple
//! average of their first period, and RSI uses Wilder's smoothing (an
//! exponential average weighted 1 / period).
//!
//! A study is written as one word on the command line, in a route and in
//! `config.toml` ([`Study::token`], [`Study::parse`]): `SMA50`, `BB20,2`,
//! `MACD12,26,9`.

/// The longest period a study may have, in bars.
pub const MAX_PERIOD: usize = 500;

/// The widest Bollinger band, in standard deviations.
pub const MAX_WIDTH: f64 = 10.0;

/// A study and its periods (in bars).
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Study {
    /// Simple moving average.
    Sma { period: usize },
    /// Exponential moving average, weighted 2 / (period + 1).
    Ema { period: usize },
    /// A simple moving average, with bands `width` standard deviations above
    /// and below it.
    Bollinger { period: usize, width: f64 },
    /// The close minus the close `period` bars earlier.
    Momentum { period: usize },
    /// The percentage change from the close `period` bars earlier.
    RateOfChange { period: usize },
    /// Wilder's relative strength index, from 0 to 100.
    Rsi { period: usize },
    /// The fast EMA minus the slow one, with a signal line: an EMA of that
    /// difference.
    Macd {
        fast: usize,
        slow: usize,
        signal: usize,
    },
}

impl Study {
    /// Each study with its usual periods: what GP's Studies menu offers.
    pub const PRESETS: [Study; 9] = [
        Study::Sma { period: 20 },
        Study::Sma { period: 50 },
        Study::Sma { period: 200 },
        Study::Ema { period: 20 },
        Study::Bollinger {
            period: 20,
            width: 2.0,
        },
        Study::Momentum { period: 10 },
        Study::RateOfChange { period: 10 },
        Study::Rsi { period: 14 },
        Study::Macd {
            fast: 12,
            slow: 26,
            signal: 9,
        },
    ];

    /// The study as one word, for a command, a route or `config.toml`:
    /// `SMA50`, `BB20,2`, `MACD12,26,9`. [`Study::parse`] reads it back.
    pub fn token(&self) -> String {
        match *self {
            Study::Sma { period } => format!("SMA{period}"),
            Study::Ema { period } => format!("EMA{period}"),
            Study::Bollinger { period, width } => format!("BB{period},{width}"),
            Study::Momentum { period } => format!("MOM{period}"),
            Study::RateOfChange { period } => format!("ROC{period}"),
            Study::Rsi { period } => format!("RSI{period}"),
            Study::Macd { fast, slow, signal } => format!("MACD{fast},{slow},{signal}"),
        }
    }

    /// Reads a study's name and periods, ignoring case: `SMA50`, `bb20,2.5`,
    /// `MACD 12 26 9` (commas, spaces or slashes between the numbers). Periods
    /// left out take their usual values, so `RSI` is `RSI14` and `BB20` is
    /// `BB20,2`. `None` for anything else, or a period outside 1 to
    /// [`MAX_PERIOD`], or a band width outside (0, [`MAX_WIDTH`]].
    pub fn parse(s: &str) -> Option<Study> {
        let s = s.trim().to_ascii_uppercase();
        let at = s
            .find(|c: char| !c.is_ascii_alphabetic())
            .unwrap_or(s.len());
        let (name, rest) = s.split_at(at);
        let numbers: Vec<&str> = rest
            .split([',', ' ', '/'])
            .filter(|n| !n.is_empty())
            .collect();
        let period = |i: usize, usual: usize| match numbers.get(i) {
            None => Some(usual),
            Some(n) => n
                .parse::<usize>()
                .ok()
                .filter(|p| (1..=MAX_PERIOD).contains(p)),
        };
        let study = match name {
            "SMA" => Study::Sma {
                period: period(0, 20)?,
            },
            "EMA" => Study::Ema {
                period: period(0, 20)?,
            },
            "BB" => Study::Bollinger {
                period: period(0, 20)?,
                width: match numbers.get(1) {
                    None => 2.0,
                    Some(w) => w
                        .parse::<f64>()
                        .ok()
                        .filter(|w| *w > 0.0 && *w <= MAX_WIDTH)?,
                },
            },
            "MOM" => Study::Momentum {
                period: period(0, 10)?,
            },
            "ROC" => Study::RateOfChange {
                period: period(0, 10)?,
            },
            "RSI" => Study::Rsi {
                period: period(0, 14)?,
            },
            "MACD" => Study::Macd {
                fast: period(0, 12)?,
                slow: period(1, 26)?,
                signal: period(2, 9)?,
            },
            _ => return None,
        };
        let takes = match study {
            Study::Bollinger { .. } => 2,
            Study::Macd { .. } => 3,
            _ => 1,
        };
        (numbers.len() <= takes).then_some(study)
    }

    /// The study over `closes`, one value per close (see the module notes).
    pub fn values(&self, closes: &[f64]) -> Values {
        match *self {
            Study::Sma { period } => Values::Line(sma(closes, period)),
            Study::Ema { period } => Values::Line(ema(closes, period)),
            Study::Bollinger { period, width } => Values::Bands(bollinger(closes, period, width)),
            Study::Momentum { period } => Values::Line(momentum(closes, period)),
            Study::RateOfChange { period } => Values::Line(rate_of_change(closes, period)),
            Study::Rsi { period } => Values::Line(rsi(closes, period)),
            Study::Macd { fast, slow, signal } => Values::Macd(macd(closes, fast, slow, signal)),
        }
    }

    /// Drawn over the price, rather than in a pane of its own below it.
    pub fn is_overlay(&self) -> bool {
        matches!(
            self,
            Study::Sma { .. } | Study::Ema { .. } | Study::Bollinger { .. }
        )
    }

    /// The study's name without its periods: `SMA`, `Bollinger`.
    pub fn name(&self) -> &'static str {
        match self {
            Study::Sma { .. } => "SMA",
            Study::Ema { .. } => "EMA",
            Study::Bollinger { .. } => "Bollinger",
            Study::Momentum { .. } => "Momentum",
            Study::RateOfChange { .. } => "Rate of change",
            Study::Rsi { .. } => "RSI",
            Study::Macd { .. } => "MACD",
        }
    }

    /// A short label for legends: `SMA 20`, `BB 20 2`, `MACD 12 26 9`.
    pub fn label(&self) -> String {
        match *self {
            Study::Sma { period } => format!("SMA {period}"),
            Study::Ema { period } => format!("EMA {period}"),
            Study::Bollinger { period, width } => format!("BB {period} {width}"),
            Study::Momentum { period } => format!("MOM {period}"),
            Study::RateOfChange { period } => format!("ROC {period}"),
            Study::Rsi { period } => format!("RSI {period}"),
            Study::Macd { fast, slow, signal } => format!("MACD {fast} {slow} {signal}"),
        }
    }

    /// How many bars before the first one shown to fetch, so the study has a
    /// value from that first bar on. An exponential study never quite forgets
    /// where it started, so its lookback also covers the bars it takes for
    /// that start to weigh less than 1% (`SETTLED`): with it, the values match
    /// those computed from a long history.
    pub fn lookback(&self) -> usize {
        let ema = |period: usize| period.saturating_sub(1) + settle(2.0 / (period as f64 + 1.0));
        match *self {
            Study::Sma { period } | Study::Bollinger { period, .. } => period.saturating_sub(1),
            Study::Momentum { period } | Study::RateOfChange { period } => period,
            Study::Ema { period } => ema(period),
            Study::Rsi { period } => period + settle(1.0 / period.max(1) as f64),
            Study::Macd { fast, slow, signal } => ema(fast.max(slow)) + ema(signal),
        }
    }
}

/// A study's values, one per close, shaped by the study.
#[derive(Clone, Debug, PartialEq)]
pub enum Values {
    /// Averages, momentum, rate of change and RSI.
    Line(Vec<Option<f64>>),
    Bands(Vec<Option<Band>>),
    Macd(Vec<Option<MacdValue>>),
}

impl Values {
    /// Whether the study has its whole value at bar `i`: for MACD, the
    /// signal line too.
    pub fn is_defined(&self, i: usize) -> bool {
        match self {
            Values::Line(v) => v.get(i).is_some_and(Option::is_some),
            Values::Bands(v) => v.get(i).is_some_and(Option::is_some),
            Values::Macd(v) => v
                .get(i)
                .is_some_and(|m| m.is_some_and(|m| m.signal.is_some())),
        }
    }
}

/// What weight the start of an exponential study may keep at the first bar
/// shown.
const SETTLED: f64 = 0.01;

/// Bars until an exponential average weighted `alpha` keeps less than
/// [`SETTLED`] of its starting value.
fn settle(alpha: f64) -> usize {
    if alpha >= 1.0 {
        return 0;
    }
    (SETTLED.ln() / (1.0 - alpha).ln()).ceil() as usize
}

/// Simple moving average over `period` bars.
pub fn sma(closes: &[f64], period: usize) -> Vec<Option<f64>> {
    rolling(closes, period, mean)
}

/// Exponential moving average, weighted 2 / (`period` + 1), starting from the
/// simple average of the first `period` closes.
pub fn ema(closes: &[f64], period: usize) -> Vec<Option<f64>> {
    exponential(closes, period, 2.0 / (period as f64 + 1.0))
}

/// Bollinger bands: the simple average of `period` closes, and `width`
/// standard deviations (of the population, as Bollinger defines them) above
/// and below it.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Band {
    pub lower: f64,
    pub middle: f64,
    pub upper: f64,
}

pub fn bollinger(closes: &[f64], period: usize, width: f64) -> Vec<Option<Band>> {
    rolling(closes, period, |window| {
        let middle = mean(window);
        let variance =
            window.iter().map(|x| (x - middle).powi(2)).sum::<f64>() / window.len() as f64;
        let spread = width * variance.sqrt();
        Band {
            lower: middle - spread,
            middle,
            upper: middle + spread,
        }
    })
}

/// The close minus the close `period` bars earlier.
pub fn momentum(closes: &[f64], period: usize) -> Vec<Option<f64>> {
    lagged(closes, period, |now, then| Some(now - then))
}

/// The percentage change from the close `period` bars earlier; `None` where
/// that close is zero.
pub fn rate_of_change(closes: &[f64], period: usize) -> Vec<Option<f64>> {
    lagged(closes, period, |now, then| {
        (then != 0.0).then(|| 100.0 * (now / then - 1.0))
    })
}

/// Wilder's RSI: the average gain over the average gain plus the average
/// loss, as a percentage. The averages start as simple averages of the first
/// `period` changes, then move by 1 / `period` of each new change. A stretch
/// with no change at all reads 50.
pub fn rsi(closes: &[f64], period: usize) -> Vec<Option<f64>> {
    let mut out = vec![None; closes.len()];
    if period == 0 || closes.len() <= period {
        return out;
    }
    let changes: Vec<f64> = closes.windows(2).map(|w| w[1] - w[0]).collect();
    let gains: Vec<f64> = changes.iter().map(|c| c.max(0.0)).collect();
    let losses: Vec<f64> = changes.iter().map(|c| (-c).max(0.0)).collect();
    let n = period as f64;
    let (mut gain, mut loss) = (mean(&gains[..period]), mean(&losses[..period]));
    out[period] = Some(strength(gain, loss));
    for i in period..changes.len() {
        gain = (gain * (n - 1.0) + gains[i]) / n;
        loss = (loss * (n - 1.0) + losses[i]) / n;
        out[i + 1] = Some(strength(gain, loss));
    }
    out
}

/// 100 − 100 / (1 + gain / loss), written so a stretch without losses needs
/// no division by zero.
fn strength(gain: f64, loss: f64) -> f64 {
    if gain + loss == 0.0 {
        50.0
    } else {
        100.0 * gain / (gain + loss)
    }
}

/// One bar of MACD: the fast EMA minus the slow one, and the signal line once
/// it has enough of those to average.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct MacdValue {
    pub macd: f64,
    pub signal: Option<f64>,
}

impl MacdValue {
    /// MACD minus its signal line, the bars under the lines.
    pub fn histogram(&self) -> Option<f64> {
        self.signal.map(|signal| self.macd - signal)
    }
}

/// MACD with `fast` and `slow` EMAs of the closes and a `signal` EMA of their
/// difference.
pub fn macd(closes: &[f64], fast: usize, slow: usize, signal: usize) -> Vec<Option<MacdValue>> {
    let line: Vec<Option<f64>> = ema(closes, fast)
        .into_iter()
        .zip(ema(closes, slow))
        .map(|(f, s)| Some(f? - s?))
        .collect();
    let Some(start) = line.iter().position(Option::is_some) else {
        return vec![None; closes.len()];
    };
    // Once both averages have started, every bar has a value.
    let defined: Vec<f64> = line[start..].iter().flatten().copied().collect();
    let signals = ema(&defined, signal);
    line.iter()
        .enumerate()
        .map(|(i, macd)| {
            macd.map(|macd| MacdValue {
                macd,
                signal: i.checked_sub(start).and_then(|j| signals[j]),
            })
        })
        .collect()
}

fn mean(xs: &[f64]) -> f64 {
    xs.iter().sum::<f64>() / xs.len() as f64
}

/// `f` over each run of `period` values ending at each bar.
fn rolling<T>(xs: &[f64], period: usize, f: impl Fn(&[f64]) -> T) -> Vec<Option<T>> {
    (0..xs.len())
        .map(|i| (period > 0 && i + 1 >= period).then(|| f(&xs[i + 1 - period..=i])))
        .collect()
}

/// `f` of each value and the one `period` bars before it.
fn lagged(xs: &[f64], period: usize, f: impl Fn(f64, f64) -> Option<f64>) -> Vec<Option<f64>> {
    (0..xs.len())
        .map(|i| {
            if period > 0 && i >= period {
                f(xs[i], xs[i - period])
            } else {
                None
            }
        })
        .collect()
}

/// An exponential average weighted `alpha`, starting from the simple average
/// of the first `period` values.
fn exponential(xs: &[f64], period: usize, alpha: f64) -> Vec<Option<f64>> {
    let mut out = vec![None; xs.len()];
    if period == 0 || xs.len() < period {
        return out;
    }
    let mut average = mean(&xs[..period]);
    out[period - 1] = Some(average);
    for (i, x) in xs.iter().enumerate().skip(period) {
        average += alpha * (x - average);
        out[i] = Some(average);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Values worked by hand, compared to within rounding.
    fn assert_series(got: &[Option<f64>], want: &[Option<f64>]) {
        assert_eq!(got.len(), want.len(), "{got:?}");
        for (i, (g, w)) in got.iter().zip(want).enumerate() {
            match (g, w) {
                (Some(g), Some(w)) => assert!((g - w).abs() < 1e-9, "bar {i}: {g} != {w}"),
                (None, None) => {}
                _ => panic!("bar {i}: {g:?} != {w:?}"),
            }
        }
    }

    const ZIGZAG: [f64; 6] = [1.0, 3.0, 2.0, 5.0, 4.0, 6.0];

    #[test]
    fn sma_averages_each_window() {
        let closes = [1.0, 2.0, 3.0, 4.0, 5.0, 6.0];
        assert_series(
            &sma(&closes, 3),
            &[None, None, Some(2.0), Some(3.0), Some(4.0), Some(5.0)],
        );
        assert_series(&sma(&closes, 1), &closes.map(Some));
        assert_series(&sma(&closes, 7), &[None; 6]);
        assert_series(&sma(&closes, 0), &[None; 6]);
        assert!(sma(&[], 3).is_empty());
    }

    #[test]
    fn ema_starts_from_the_simple_average() {
        // Period 3, weight 1/2: starts at (1 + 3 + 2) / 3 = 2, then
        // 2 + (5 - 2) / 2 = 3.5, 3.5 + (4 - 3.5) / 2 = 3.75, 3.75 + (6 - 3.75) / 2 = 4.875.
        assert_series(
            &ema(&ZIGZAG, 3),
            &[None, None, Some(2.0), Some(3.5), Some(3.75), Some(4.875)],
        );
        // Period 2, weight 2/3: starts at 2, then 2 + (2 - 2)·2/3 = 2,
        // 2 + (5 - 2)·2/3 = 4, 4 + 0 = 4, 4 + (6 - 4)·2/3 = 16/3.
        assert_series(
            &ema(&ZIGZAG, 2),
            &[
                None,
                Some(2.0),
                Some(2.0),
                Some(4.0),
                Some(4.0),
                Some(16.0 / 3.0),
            ],
        );
    }

    #[test]
    fn bollinger_bands_use_the_population_deviation() {
        // The textbook population: mean 5, standard deviation exactly 2.
        let closes = [2.0, 4.0, 4.0, 4.0, 5.0, 5.0, 7.0, 9.0];
        let bands = bollinger(&closes, 8, 2.0);
        assert!(bands[..7].iter().all(Option::is_none));
        assert_eq!(
            bands[7],
            Some(Band {
                lower: 1.0,
                middle: 5.0,
                upper: 9.0
            })
        );
        // A flat stretch has no spread: the bands close onto the average.
        let flat = bollinger(&[3.0; 4], 2, 2.0);
        assert_eq!(
            flat[3],
            Some(Band {
                lower: 3.0,
                middle: 3.0,
                upper: 3.0
            })
        );
    }

    #[test]
    fn momentum_and_rate_of_change_look_back_a_period() {
        // 2 − 1, 5 − 3, 4 − 2, 6 − 5.
        assert_series(
            &momentum(&ZIGZAG, 2),
            &[None, None, Some(1.0), Some(2.0), Some(2.0), Some(1.0)],
        );
        // 2/1, 5/3, 4/2 and 6/5, less one, as percentages.
        assert_series(
            &rate_of_change(&ZIGZAG, 2),
            &[
                None,
                None,
                Some(100.0),
                Some(200.0 / 3.0),
                Some(100.0),
                Some(20.0),
            ],
        );
        assert_series(
            &rate_of_change(&[0.0, 1.0, 2.0], 1),
            &[None, None, Some(100.0)],
        );
        assert_series(&momentum(&ZIGZAG, 0), &[None; 6]);
    }

    #[test]
    fn rsi_follows_wilder() {
        // Changes +1 −1 +2 +1 −1. Period 3 starts from the first three:
        // gain (1 + 0 + 2) / 3 = 1, loss (0 + 1 + 0) / 3 = 1/3, RSI 100·1/(4/3) = 75.
        // +1: gain (2·1 + 1) / 3 = 1, loss (2/3) / 3 = 2/9, RSI 100·1/(11/9) = 900/11.
        // −1: gain 2/3, loss (4/9 + 1) / 3 = 13/27, RSI 100·(2/3)/(31/27) = 1800/31.
        let closes = [10.0, 11.0, 10.0, 12.0, 13.0, 12.0];
        assert_series(
            &rsi(&closes, 3),
            &[
                None,
                None,
                None,
                Some(75.0),
                Some(900.0 / 11.0),
                Some(1800.0 / 31.0),
            ],
        );
        assert_series(
            &rsi(&[1.0, 2.0, 3.0, 4.0], 2),
            &[None, None, Some(100.0), Some(100.0)],
        );
        assert_series(&rsi(&[4.0, 3.0, 2.0], 2), &[None, None, Some(0.0)]);
        assert_series(&rsi(&[5.0; 3], 2), &[None, None, Some(50.0)]);
        assert_series(&rsi(&[1.0, 2.0], 2), &[None, None]);
    }

    #[test]
    fn macd_is_the_gap_between_two_emas_and_its_average() {
        // EMA 2 and EMA 3 of the zigzag are worked above: from the third bar
        // MACD is 2 − 2 = 0, 4 − 3.5 = 0.5, 4 − 3.75 = 0.25, 16/3 − 4.875 = 11/24.
        // Signal (EMA 2 of those): starts at (0 + 0.5) / 2 = 1/4, then
        // 1/4 + (1/4 − 1/4)·2/3 = 1/4, 1/4 + (11/24 − 1/4)·2/3 = 7/18.
        let values = macd(&ZIGZAG, 2, 3, 2);
        let lines: Vec<Option<f64>> = values.iter().map(|v| v.map(|v| v.macd)).collect();
        let signals: Vec<Option<f64>> = values.iter().map(|v| v.and_then(|v| v.signal)).collect();
        let bars: Vec<Option<f64>> = values
            .iter()
            .map(|v| v.and_then(|v| v.histogram()))
            .collect();
        assert_series(
            &lines,
            &[
                None,
                None,
                Some(0.0),
                Some(0.5),
                Some(0.25),
                Some(11.0 / 24.0),
            ],
        );
        assert_series(
            &signals,
            &[None, None, None, Some(0.25), Some(0.25), Some(7.0 / 18.0)],
        );
        assert_series(
            &bars,
            &[
                None,
                None,
                None,
                Some(0.25),
                Some(0.0),
                Some(11.0 / 24.0 - 7.0 / 18.0),
            ],
        );
        assert!(macd(&ZIGZAG, 12, 26, 9).iter().all(Option::is_none));
    }

    #[test]
    fn lookback_covers_the_warm_up() {
        assert_eq!(Study::Sma { period: 200 }.lookback(), 199);
        assert_eq!(
            Study::Bollinger {
                period: 20,
                width: 2.0
            }
            .lookback(),
            19
        );
        assert_eq!(Study::Momentum { period: 10 }.lookback(), 10);
        assert_eq!(Study::RateOfChange { period: 10 }.lookback(), 10);
        // Weight 2/21: ln 0.01 / ln(19/21) = 46.01, so 47 bars after the first 19.
        assert_eq!(Study::Ema { period: 20 }.lookback(), 19 + 47);
        // Weight 1/14: ln 0.01 / ln(13/14) = 62.14, so 63 bars after the first 14.
        assert_eq!(Study::Rsi { period: 14 }.lookback(), 14 + 63);
        // The slow EMA (25 + 60, from 59.84) then the signal's (8 + 21, from 20.64).
        assert_eq!(
            Study::Macd {
                fast: 12,
                slow: 26,
                signal: 9
            }
            .lookback(),
            25 + 60 + 8 + 21
        );
        assert_eq!(Study::Ema { period: 1 }.lookback(), 0);
    }

    #[test]
    fn values_are_defined_from_the_first_bar_after_the_lookback() {
        let studies = [
            Study::Sma { period: 20 },
            Study::Ema { period: 20 },
            Study::Bollinger {
                period: 20,
                width: 2.0,
            },
            Study::Momentum { period: 10 },
            Study::RateOfChange { period: 10 },
            Study::Rsi { period: 14 },
            Study::Macd {
                fast: 12,
                slow: 26,
                signal: 9,
            },
        ];
        for study in studies {
            let closes: Vec<f64> = (0..=study.lookback())
                .map(|i| 50.0 + (i as f64 * 0.7).sin())
                .collect();
            let defined = match study {
                Study::Sma { period } => sma(&closes, period)[study.lookback()].is_some(),
                Study::Ema { period } => ema(&closes, period)[study.lookback()].is_some(),
                Study::Bollinger { period, width } => {
                    bollinger(&closes, period, width)[study.lookback()].is_some()
                }
                Study::Momentum { period } => momentum(&closes, period)[study.lookback()].is_some(),
                Study::RateOfChange { period } => {
                    rate_of_change(&closes, period)[study.lookback()].is_some()
                }
                Study::Rsi { period } => rsi(&closes, period)[study.lookback()].is_some(),
                Study::Macd { fast, slow, signal } => macd(&closes, fast, slow, signal)
                    [study.lookback()]
                .is_some_and(|v| v.signal.is_some()),
            };
            assert!(defined, "{}", study.label());
        }
    }

    #[test]
    fn exponential_lookback_forgets_the_start() {
        // Long history at 0, then the bars a lookback fetches at 1: the EMA
        // computed from that lookback alone (exactly 1) and from the whole
        // history agree to within 1%.
        let study = Study::Ema { period: 20 };
        let mut closes = vec![0.0; 500];
        closes.extend(std::iter::repeat_n(1.0, study.lookback() + 1));
        let long = ema(&closes, 20)
            .last()
            .copied()
            .flatten()
            .unwrap_or_default();
        let short = ema(&closes[500..], 20)
            .last()
            .copied()
            .flatten()
            .unwrap_or_default();
        assert_eq!(short, 1.0);
        assert!((short - long).abs() < SETTLED, "{long}");
    }

    #[test]
    fn tokens_read_back() {
        for study in Study::PRESETS {
            assert_eq!(
                Study::parse(&study.token()),
                Some(study),
                "{}",
                study.token()
            );
            // A legend's label reads back too.
            assert_eq!(
                Study::parse(&study.label()),
                Some(study),
                "{}",
                study.label()
            );
        }
        let bb = Study::Bollinger {
            period: 20,
            width: 2.5,
        };
        assert_eq!(bb.token(), "BB20,2.5");
        assert_eq!(Study::parse("bb20/2.5"), Some(bb));
        assert_eq!(
            Study::Macd {
                fast: 12,
                slow: 26,
                signal: 9
            }
            .token(),
            "MACD12,26,9"
        );
    }

    #[test]
    fn parse_fills_in_usual_periods_and_refuses_the_rest() {
        assert_eq!(Study::parse(" rsi "), Some(Study::Rsi { period: 14 }));
        assert_eq!(Study::parse("SMA"), Some(Study::Sma { period: 20 }));
        assert_eq!(
            Study::parse("BB10"),
            Some(Study::Bollinger {
                period: 10,
                width: 2.0
            })
        );
        assert_eq!(
            Study::parse("MACD5,35"),
            Some(Study::Macd {
                fast: 5,
                slow: 35,
                signal: 9
            })
        );
        assert_eq!(
            Study::parse("SMA500"),
            Some(Study::Sma { period: MAX_PERIOD })
        );
        for bad in [
            "",
            "50",
            "SMA0",
            "SMA501",
            "SMA-5",
            "SMA5X",
            "SMA20,2",
            "RSI14,3",
            "BB20,0",
            "BB20,11",
            "BB20,NAN",
            "BB20,2,1",
            "MACD1,2,3,4",
            "VWAP",
            "XLU",
        ] {
            assert_eq!(Study::parse(bad), None, "{bad}");
        }
    }

    #[test]
    fn values_match_the_functions() {
        let closes: Vec<f64> = (0..60).map(|i| 50.0 + (i as f64 * 0.3).sin()).collect();
        for study in Study::PRESETS {
            let values = study.values(&closes);
            let first = (0..closes.len()).find(|i| values.is_defined(*i));
            match (study, &values) {
                (Study::Sma { period }, Values::Line(v)) => assert_eq!(*v, sma(&closes, period)),
                (Study::Ema { period }, Values::Line(v)) => assert_eq!(*v, ema(&closes, period)),
                (Study::Bollinger { period, width }, Values::Bands(v)) => {
                    assert_eq!(*v, bollinger(&closes, period, width));
                }
                (Study::Momentum { period }, Values::Line(v)) => {
                    assert_eq!(*v, momentum(&closes, period));
                }
                (Study::RateOfChange { period }, Values::Line(v)) => {
                    assert_eq!(*v, rate_of_change(&closes, period));
                }
                (Study::Rsi { period }, Values::Line(v)) => assert_eq!(*v, rsi(&closes, period)),
                (Study::Macd { fast, slow, signal }, Values::Macd(v)) => {
                    assert_eq!(*v, macd(&closes, fast, slow, signal));
                    // The MACD line starts before its signal does.
                    assert_eq!(first, Some(slow - 1 + signal - 1));
                }
                _ => panic!("{} has the wrong shape", study.label()),
            }
        }
        // Sixty closes are too few for a 200-bar average.
        assert!(!Study::Sma { period: 200 }.values(&closes).is_defined(59));
        assert!(Study::Sma { period: 20 }.values(&closes).is_defined(19));
        assert!(!Study::Sma { period: 20 }.values(&closes).is_defined(60));
    }

    #[test]
    fn labels_and_placement() {
        let bb = Study::Bollinger {
            period: 20,
            width: 2.0,
        };
        assert_eq!(bb.label(), "BB 20 2");
        assert_eq!(
            Study::Bollinger {
                period: 20,
                width: 2.5
            }
            .label(),
            "BB 20 2.5"
        );
        assert_eq!(
            Study::Macd {
                fast: 12,
                slow: 26,
                signal: 9
            }
            .label(),
            "MACD 12 26 9"
        );
        assert!(bb.is_overlay() && Study::Ema { period: 9 }.is_overlay());
        assert!(!Study::Rsi { period: 14 }.is_overlay());
    }
}
