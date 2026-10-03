//! Time-series charts in market time.
//!
//! The x axis is seconds since the epoch (`mt_core::time::chart_x`), labelled
//! in market time (EST) with a grid that snaps to 5-minute, hourly or daily
//! boundaries instead of egui_plot's decimal steps.

use chrono::NaiveDateTime;
use egui::Color32;
use egui_plot::{
    GridInput, GridMark, HoverPosition, Legend, Line, LineStyle, Plot, PlotPoints, PlotUi,
};
use mt_core::time::{MARKET_TZ_LABEL, MARKET_UTC_OFFSET_SECS, chart_x, from_chart_x};

use crate::skin::Skin;

const HOUR: f64 = 3600.0;
const DAY: f64 = 86_400.0;
const STEPS: &[f64] = &[
    300.0,
    900.0,
    1800.0,
    HOUR,
    2.0 * HOUR,
    3.0 * HOUR,
    6.0 * HOUR,
    12.0 * HOUR,
    DAY,
    2.0 * DAY,
    7.0 * DAY,
    14.0 * DAY,
    28.0 * DAY,
    56.0 * DAY,
    91.0 * DAY,
    182.0 * DAY,
    364.0 * DAY,
];

/// Grid marks on round market-time boundaries.
fn time_grid(input: GridInput) -> Vec<GridMark> {
    let (lo, hi) = input.bounds;
    let span = (hi - lo).max(1.0);
    let step = STEPS
        .iter()
        .copied()
        .find(|s| span / s <= 8.0)
        .unwrap_or(364.0 * DAY);
    // Align to market midnight, not UTC midnight.
    let offset = f64::from(MARKET_UTC_OFFSET_SECS);
    let first = ((lo + offset) / step).ceil() * step - offset;
    let mut marks = Vec::new();
    let mut x = first;
    while x <= hi && marks.len() < 200 {
        marks.push(GridMark {
            value: x,
            step_size: step,
        });
        // Finer, fainter marks in between.
        let minor = step / 2.0;
        if x + minor <= hi {
            marks.push(GridMark {
                value: x + minor,
                step_size: minor,
            });
        }
        x += step;
    }
    marks
}

fn axis_label(mark: GridMark, range: &std::ops::RangeInclusive<f64>) -> String {
    let Some(t) = from_chart_x(mark.value) else {
        return String::new();
    };
    let span = range.end() - range.start();
    let midnight = t.time() == chrono::NaiveTime::MIN;
    if span > 3.0 * DAY {
        // Dates at midnight; times in between only while they still fit.
        if midnight && span > 300.0 * DAY {
            t.format("%b %d %Y").to_string()
        } else if midnight {
            t.format("%b %d").to_string()
        } else if span <= 7.0 * DAY {
            t.format("%H:%M").to_string()
        } else {
            String::new()
        }
    } else if mark.step_size >= DAY {
        t.format("%b %d").to_string()
    } else if span > DAY {
        t.format("%a %H:%M").to_string()
    } else {
        t.format("%H:%M").to_string()
    }
}

fn hover_label(pos: &HoverPosition<'_>) -> Option<String> {
    let (name, p) = match pos {
        HoverPosition::NearDataPoint {
            plot_name,
            position,
            ..
        } => (*plot_name, position),
        HoverPosition::Elsewhere { position } => ("", position),
    };
    let t = from_chart_x(p.x)?;
    let when = t.format("%b %d %H:%M").to_string();
    Some(if name.is_empty() {
        format!("{when} {MARKET_TZ_LABEL}\n{:.2}", p.y)
    } else {
        format!("{name}\n{when} {MARKET_TZ_LABEL}\n{:.2}", p.y)
    })
}

/// A plot with a market-time x axis, themed grid and legend.
pub fn time_plot<'a>(id: &str, skin: &Skin) -> Plot<'a> {
    time_plot_bare(id, skin).legend(
        Legend::default()
            .position(egui_plot::Corner::LeftTop)
            .background_alpha(0.8)
            .follow_insertion_order(true),
    )
}

/// [`time_plot`] without a legend, for charts whose key is drawn elsewhere.
pub fn time_plot_bare<'a>(id: &str, skin: &Skin) -> Plot<'a> {
    Plot::new(id)
        .x_grid_spacer(time_grid)
        .x_axis_formatter(axis_label)
        .label_formatter(hover_label)
        .grid_color(skin.grid)
        .y_axis_min_width(48.0)
        .allow_scroll(false)
        .allow_double_click_reset(true)
}

/// A plot for price duration curves: x is the share of hours, 0-100%.
pub fn duration_plot<'a>(id: &str, skin: &Skin) -> Plot<'a> {
    Plot::new(id)
        .legend(
            Legend::default()
                .position(egui_plot::Corner::RightTop)
                .background_alpha(0.8),
        )
        .grid_color(skin.grid)
        .include_x(0.0)
        .include_x(100.0)
        .x_axis_formatter(|m, _| format!("{:.0}%", m.value))
        .x_axis_label("share of hours at or above")
        .y_axis_min_width(48.0)
        .label_formatter(|pos: &HoverPosition<'_>| {
            let (name, p) = match pos {
                HoverPosition::NearDataPoint {
                    plot_name,
                    position,
                    ..
                } => (*plot_name, position),
                HoverPosition::Elsewhere { position } => ("", position),
            };
            Some(format!(
                "{name}
{:.0}% of hours at or above {:.2}",
                p.x.clamp(0.0, 100.0),
                p.y
            ))
        })
        .allow_scroll(false)
}

pub fn points(pts: &[(NaiveDateTime, f64)]) -> Vec<[f64; 2]> {
    pts.iter().map(|(t, v)| [chart_x(*t), *v]).collect()
}

/// A plain line through `(time, value)` points.
pub fn line<'a>(name: &str, pts: &[(NaiveDateTime, f64)], color: egui::Color32) -> Line<'a> {
    Line::new(name, PlotPoints::from(points(pts)))
        .color(color)
        .width(1.6)
}

/// Draw hourly values as a staircase: each point holds for one hour from its
/// start. A gap in the data breaks the line (as separate segments sharing one
/// legend entry) so missing hours are visible rather than bridged.
pub fn hourly_steps(
    plot: &mut PlotUi<'_>,
    name: &str,
    hours: &[(NaiveDateTime, f64)],
    color: Color32,
) {
    steps(plot, name, hours, color, LineStyle::Solid);
}

/// A dashed [`hourly_steps`], for forecasts.
pub fn forecast_steps(
    plot: &mut PlotUi<'_>,
    name: &str,
    hours: &[(NaiveDateTime, f64)],
    color: Color32,
) {
    steps(plot, name, hours, color, LineStyle::dashed_dense());
}

fn steps(
    plot: &mut PlotUi<'_>,
    name: &str,
    hours: &[(NaiveDateTime, f64)],
    color: Color32,
    style: LineStyle,
) {
    for run in step_runs(hours, HOUR) {
        plot.line(
            Line::new(name, PlotPoints::from(run))
                .color(color)
                .width(1.6)
                .style(style),
        );
    }
}

/// A line that breaks wherever consecutive points are more than `max_gap_secs`
/// apart (missing days in the five-minute archive), sharing one legend entry.
pub fn gapped_line(
    plot: &mut PlotUi<'_>,
    name: &str,
    pts: &[(NaiveDateTime, f64)],
    color: Color32,
    max_gap_secs: f64,
) {
    for run in gap_runs(pts, max_gap_secs) {
        plot.line(
            Line::new(name, PlotPoints::from(run))
                .color(color)
                .width(1.4),
        );
    }
}

/// Split points into runs at gaps longer than `max_gap_secs`.
pub fn gap_runs(pts: &[(NaiveDateTime, f64)], max_gap_secs: f64) -> Vec<Vec<[f64; 2]>> {
    let mut runs: Vec<Vec<[f64; 2]>> = Vec::new();
    let mut prev: Option<f64> = None;
    for (t, v) in pts.iter().filter(|(_, v)| v.is_finite()) {
        let x = chart_x(*t);
        if prev.is_none_or(|p| x - p > max_gap_secs) {
            runs.push(Vec::new());
        }
        if let Some(run) = runs.last_mut() {
            run.push([x, *v]);
        }
        prev = Some(x);
    }
    runs
}

/// Expand `(start, value)` points into staircase segments, starting a new
/// segment wherever consecutive points are more than one period apart.
/// (egui cannot draw NaN, so gaps are separate segments, not NaN markers.)
pub fn step_runs(pts: &[(NaiveDateTime, f64)], period_secs: f64) -> Vec<Vec<[f64; 2]>> {
    let mut runs: Vec<Vec<[f64; 2]>> = Vec::new();
    let mut prev_end: Option<f64> = None;
    for (t, v) in pts.iter().filter(|(_, v)| v.is_finite()) {
        let x = chart_x(*t);
        if prev_end.is_none_or(|end| (x - end).abs() > 1.0) {
            runs.push(Vec::new());
        }
        if let Some(run) = runs.last_mut() {
            run.push([x, *v]);
            run.push([x + period_secs, *v]);
        }
        prev_end = Some(x + period_secs);
    }
    runs
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn multi_day_axes_label_dates_at_midnight_only() {
        let day = chrono::NaiveDate::from_ymd_opt(2026, 9, 30).unwrap();
        let x = |h: u32| chart_x(day.and_hms_opt(h, 0, 0).unwrap());
        let mark = |h: u32, step: f64| GridMark {
            value: x(h),
            step_size: step,
        };
        let five_days = x(0)..=x(0) + 5.0 * DAY;
        assert_eq!(axis_label(mark(0, DAY), &five_days), "Sep 30");
        assert_eq!(axis_label(mark(12, DAY / 2.0), &five_days), "12:00");
        let month = x(0)..=x(0) + 30.0 * DAY;
        assert_eq!(axis_label(mark(12, DAY / 2.0), &month), "");
        let year = x(0)..=x(0) + 365.0 * DAY;
        assert_eq!(axis_label(mark(0, 28.0 * DAY), &year), "Sep 30 2026");
        // A year spans about eight marks, so labels stay readable.
        let marks = time_grid(GridInput {
            bounds: (*year.start(), *year.end()),
            base_step_size: 1.0,
        });
        let step = marks.iter().map(|m| m.step_size).fold(0.0, f64::max);
        let major = marks.iter().filter(|m| m.step_size == step).count();
        assert!((4..=9).contains(&major), "{major} major marks over a year");
    }

    #[test]
    fn lines_break_at_gaps() {
        let day = chrono::NaiveDate::from_ymd_opt(2026, 10, 1).unwrap();
        let t = |d: i64, m: u32| {
            (day + chrono::Duration::days(d))
                .and_hms_opt(0, m, 0)
                .unwrap()
        };
        let pts = [
            (t(0, 0), 1.0),
            (t(0, 5), 2.0),
            (t(2, 0), 3.0),
            (t(2, 5), f64::NAN),
        ];
        let runs = gap_runs(&pts, 600.0);
        assert_eq!(runs.len(), 2, "a missing day starts a new run");
        assert_eq!(runs[0].len(), 2);
        assert_eq!(runs[1].len(), 1, "NaN is dropped");
    }

    #[test]
    fn grid_aligns_to_market_hours() {
        let t = chrono::NaiveDate::from_ymd_opt(2026, 10, 2)
            .unwrap()
            .and_hms_opt(0, 0, 0)
            .unwrap();
        let lo = chart_x(t);
        let marks = time_grid(GridInput {
            bounds: (lo - 10.0, lo + 6.0 * HOUR),
            base_step_size: 1.0,
        });
        let major: Vec<_> = marks.iter().filter(|m| m.step_size == HOUR).collect();
        assert_eq!(
            major.first().map(|m| from_chart_x(m.value).unwrap()),
            Some(t)
        );
        assert!(marks.len() < 20);
    }

    #[test]
    fn steps_split_at_gaps_and_never_emit_nan() {
        let d = chrono::NaiveDate::from_ymd_opt(2026, 10, 2).unwrap();
        let h = |h| d.and_hms_opt(h, 0, 0).unwrap();
        let runs = step_runs(
            &[(h(0), 1.0), (h(1), 2.0), (h(3), f64::NAN), (h(5), 3.0)],
            HOUR,
        );
        assert_eq!(runs.len(), 2);
        assert_eq!(runs[0].len(), 4);
        assert_eq!(runs[1].len(), 2);
        assert!(
            runs.iter()
                .flatten()
                .all(|p| p[0].is_finite() && p[1].is_finite())
        );
        assert!(step_runs(&[], HOUR).is_empty());
    }
}
