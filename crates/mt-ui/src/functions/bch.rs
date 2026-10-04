//! BCH: binding-constraint history. For one market day, every constraint that
//! bound in the day-ahead or real-time market (matched by MISO's constraint
//! ID), how long it bound and what it cost, and its shadow prices through the
//! day, DA against RT.

use chrono::{Duration, NaiveDate, Timelike};
use egui::{RichText, Ui};
use egui_extras::{Column, TableBuilder};
use egui_plot::{Line, Plot, PlotPoints};
use mt_core::time::market_today;
use mt_core::{ConstraintHistory, ConstraintSummary, Market};

use crate::context::PanelCx;
use crate::function::{Category, FunctionSpec, Panel, Route};
use crate::widgets::table::{Sort, cmp_opt, num_cell, sort_header};
use crate::widgets::{self, csv, fmt};

pub const SPEC: FunctionSpec = FunctionSpec {
    code: "BCH",
    aliases: &["CONHIST", "BCHIST"],
    name: "Constraint history",
    category: Category::Grid,
    usage: "BCH [YYYY-MM-DD | days ago]",
    description: "A day's binding constraints in DA and RT, matched by constraint: hours bound, cost ($/MW) and peak shadow price, with the selected constraint's shadow prices through the day.",
    takes_node: false,
    takes_security: false,
    open,
};

fn open(args: &[String]) -> Result<Box<dyn Panel>, String> {
    let day = match args.first() {
        None => None,
        Some(a) => Some(
            NaiveDate::parse_from_str(a, "%Y-%m-%d")
                .or_else(|_| {
                    a.parse::<i64>()
                        .map(|back| market_today() - Duration::days(back))
                })
                .map_err(|_| format!("expected a date (2026-10-01) or days ago, got {a:?}"))?,
        ),
    };
    Ok(Box::new(Bch {
        day,
        filter: String::new(),
        selected: None,
        // Most costly in real time first.
        sort: Sort::new(5, true),
    }))
}

struct Bch {
    /// `None` follows "yesterday", the latest day with both DA and RT.
    day: Option<NaiveDate>,
    filter: String,
    selected: Option<u64>,
    sort: Sort,
}

/// A constraint's day in each market.
struct Row {
    id: u64,
    name: String,
    contingency: String,
    branch: String,
    da: Option<ConstraintSummary>,
    rt: Option<ConstraintSummary>,
}

fn merge(da: Option<&ConstraintHistory>, rt: Option<&ConstraintHistory>) -> Vec<Row> {
    let mut rows: Vec<Row> = Vec::new();
    let mut index = std::collections::HashMap::new();
    for (summaries, is_rt) in [
        (rt.map(ConstraintHistory::summary), true),
        (da.map(ConstraintHistory::summary), false),
    ] {
        for s in summaries.into_iter().flatten() {
            let i = *index.entry(s.id).or_insert_with(|| {
                rows.push(Row {
                    id: s.id,
                    // RT names are the longer, more descriptive ones.
                    name: s.name.clone(),
                    contingency: s.contingency.clone(),
                    branch: s.branch.clone(),
                    da: None,
                    rt: None,
                });
                rows.len() - 1
            });
            if is_rt {
                rows[i].rt = Some(s);
            } else {
                rows[i].da = Some(s);
            }
        }
    }
    rows
}

const COLUMNS: [(&str, bool); 8] = [
    ("Constraint", false),
    ("Contingency", false),
    ("DA hours", true),
    ("DA $/MW", true),
    ("RT hours", true),
    ("RT $/MW", true),
    ("RT max $/MWh", true),
    ("RT − DA $/MW", true),
];

impl Row {
    fn key(&self, column: usize) -> Option<f64> {
        let da = self.da.as_ref();
        let rt = self.rt.as_ref();
        match column {
            2 => da.map(|s| s.hours),
            3 => da.map(|s| s.total),
            4 => rt.map(|s| s.hours),
            5 => rt.map(|s| s.total),
            6 => rt.map(|s| s.max),
            7 => Some(rt.map_or(0.0, |s| s.total) - da.map_or(0.0, |s| s.total)),
            _ => None,
        }
    }
}

/// Hours since midnight, so DA and RT share an axis.
fn hour_of_day(points: &[(chrono::NaiveDateTime, f64)]) -> Vec<[f64; 2]> {
    points
        .iter()
        .map(|(t, v)| [f64::from(t.num_seconds_from_midnight()) / 3600.0, *v])
        .collect()
}

impl Panel for Bch {
    fn title(&self) -> String {
        match self.day {
            Some(d) => format!("BCH {}", d.format("%b %-d")),
            None => "BCH".into(),
        }
    }

    fn route(&self) -> Route {
        match self.day {
            Some(d) => Route::new("BCH", [d.format("%Y-%m-%d").to_string()]),
            None => Route::code("BCH"),
        }
    }

    fn ui(&mut self, ui: &mut Ui, cx: &mut PanelCx<'_>) {
        let skin = cx.skin;
        let today = market_today();
        let day = self.day.unwrap_or(today - Duration::days(1));
        let da_snap = cx
            .hub
            .watch(&cx.miso.constraint_history(Market::DayAhead, day));
        let rt_snap = cx
            .hub
            .watch(&cx.miso.constraint_history(Market::RealTime, day));

        widgets::title_bar(
            ui,
            skin,
            &format!("Binding constraints · {}", day.format("%a %b %-d")),
            |ui| widgets::freshness(ui, skin, &rt_snap),
        );
        ui.horizontal_wrapped(|ui| {
            if ui.small_button("◀").on_hover_text("Day before").clicked() {
                self.day = Some(day - Duration::days(1));
            }
            if ui
                .add_enabled(day <= today, egui::Button::new("▶").small())
                .on_hover_text("Day after")
                .clicked()
            {
                self.day = Some(day + Duration::days(1));
            }
            if ui
                .selectable_label(self.day.is_none(), "Yesterday")
                .clicked()
            {
                self.day = None;
            }
            ui.separator();
            ui.add(
                egui::TextEdit::singleline(&mut self.filter)
                    .hint_text("filter constraints")
                    .desired_width(180.0),
            );
        });

        let state = |snap: &mt_data::Snapshot<Option<ConstraintHistory>>, market: Market| match snap
            .data()
        {
            None if snap.error.is_some() => "failed to load".to_owned(),
            None => "loading…".to_owned(),
            Some(None) => match market {
                Market::DayAhead => "not posted yet (about 13:30 EST the day before)".into(),
                Market::RealTime => "not posted yet (the day after)".into(),
            },
            Some(Some(h)) => {
                let s = h.summary();
                format!(
                    "{} constraints, ${} /MW",
                    s.len(),
                    fmt::price(s.iter().map(|s| s.total).sum())
                )
            }
        };
        ui.label(
            RichText::new(format!(
                "DA: {} · RT: {}",
                state(&da_snap, Market::DayAhead),
                state(&rt_snap, Market::RealTime)
            ))
            .small()
            .color(skin.text_muted),
        );

        let da = da_snap.data().and_then(Option::as_ref);
        let rt = rt_snap.data().and_then(Option::as_ref);
        let mut rows = merge(da, rt);
        let needle = self.filter.trim().to_ascii_uppercase();
        rows.retain(|r| {
            needle.is_empty()
                || [&r.name, &r.contingency, &r.branch]
                    .iter()
                    .any(|s| s.to_ascii_uppercase().contains(&needle))
        });
        let s = self.sort;
        rows.sort_by(|a, b| match s.column {
            0 => s.apply(a.name.cmp(&b.name)),
            1 => s.apply(a.contingency.cmp(&b.contingency)),
            c => cmp_opt(a.key(c), b.key(c), &s).then(a.id.cmp(&b.id)),
        });
        if rows.is_empty() {
            if da.is_some() || rt.is_some() {
                ui.label(RichText::new("No constraints match.").color(skin.text_muted));
            }
            return;
        }
        csv::copy_button(ui, skin, || {
            let mut headers = vec!["constraint_id"];
            headers.extend(COLUMNS.iter().map(|c| c.0));
            csv::to_csv(
                &headers,
                rows.iter().map(|r| {
                    let n = |v: Option<f64>| v.map_or_else(String::new, |v| format!("{v:.2}"));
                    vec![
                        r.id.to_string(),
                        r.name.clone(),
                        r.contingency.clone(),
                        n(r.key(2)),
                        n(r.key(3)),
                        n(r.key(4)),
                        n(r.key(5)),
                        n(r.key(6)),
                        n(r.key(7)),
                    ]
                }),
            )
        });

        let row_h = ui.text_style_height(&egui::TextStyle::Body) + 2.0;
        let table_h = (ui.available_height() * 0.55).max(120.0);
        let mut sort = self.sort;
        let mut clicked = None;
        egui::ScrollArea::horizontal()
            .id_salt("bch-scroll")
            .show(ui, |ui| {
                TableBuilder::new(ui)
                    .id_salt("bch-table")
                    .striped(true)
                    .resizable(true)
                    .max_scroll_height(table_h)
                    .sense(egui::Sense::click())
                    .cell_layout(egui::Layout::left_to_right(egui::Align::Center))
                    .column(Column::initial(300.0).at_least(120.0).clip(true))
                    .column(Column::initial(200.0).at_least(80.0).clip(true))
                    .columns(Column::initial(86.0).at_least(60.0), COLUMNS.len() - 2)
                    .header(row_h + 4.0, |mut h| {
                        for (i, (name, numeric)) in COLUMNS.iter().enumerate() {
                            h.col(|ui| sort_header(ui, skin, name, i, *numeric, &mut sort));
                        }
                    })
                    .body(|body| {
                        body.rows(row_h, rows.len(), |mut row| {
                            let r = &rows[row.index()];
                            row.set_selected(self.selected == Some(r.id));
                            row.col(|ui| {
                                ui.label(RichText::new(&r.name).color(skin.text_strong))
                                    .on_hover_text(format!("{}\nID {}", r.branch, r.id));
                            });
                            row.col(|ui| {
                                ui.label(RichText::new(&r.contingency).color(skin.text_muted));
                            });
                            let num = |v: Option<f64>, f: fn(f64) -> String| match v {
                                Some(v) if v != 0.0 => RichText::new(f(v)),
                                _ => RichText::new(fmt::DASH).color(skin.text_muted),
                            };
                            row.col(|ui| num_cell(ui, num(r.key(2), |v| format!("{v:.0}"))));
                            row.col(|ui| num_cell(ui, num(r.key(3), fmt::price)));
                            row.col(|ui| num_cell(ui, num(r.key(4), |v| format!("{v:.1}"))));
                            row.col(|ui| num_cell(ui, num(r.key(5), fmt::price)));
                            row.col(|ui| num_cell(ui, num(r.key(6), fmt::price)));
                            row.col(|ui| {
                                let v = r.key(7).unwrap_or(0.0);
                                num_cell(ui, RichText::new(fmt::signed(v)).color(skin.delta(-v)));
                            });
                            if row.response().clicked() {
                                clicked = Some(r.id);
                            }
                        });
                    });
            });
        self.sort = sort;
        if let Some(id) = clicked {
            self.selected = Some(id);
        }

        let Some(id) = self
            .selected
            .or_else(|| rows.first().map(|r| r.id))
            .filter(|id| rows.iter().any(|r| r.id == *id))
        else {
            return;
        };
        let name = rows
            .iter()
            .find(|r| r.id == id)
            .map_or("", |r| r.name.as_str());
        widgets::section(ui, skin, &format!("{name} · shadow price, $/MWh"));
        let da_pts = da.map(|h| h.series(id)).unwrap_or_default();
        let rt_pts = rt.map(|h| h.series(id)).unwrap_or_default();
        Plot::new(("bch-chart", id))
            .legend(egui_plot::Legend::default())
            .include_x(0.0)
            .include_x(24.0)
            .include_y(0.0)
            .x_grid_spacer(egui_plot::uniform_grid_spacer(|_| [1.0, 3.0, 6.0]))
            .x_axis_formatter(|m, _| format!("{:02}:00", m.value.round() as i64))
            .label_formatter(|pos| {
                let (name, p) = match pos {
                    egui_plot::HoverPosition::NearDataPoint {
                        plot_name,
                        position,
                        ..
                    } => (*plot_name, position),
                    egui_plot::HoverPosition::Elsewhere { position } => ("", position),
                };
                let mins = (p.x * 60.0).round() as i64;
                Some(format!(
                    "{name}
{:02}:{:02}  {:.2}",
                    mins / 60,
                    mins % 60,
                    p.y
                ))
            })
            .show(ui, |plot| {
                // DA holds for the hour; RT is a point per five minutes.
                for (start, v) in &da_pts {
                    let x = f64::from(start.num_seconds_from_midnight()) / 3600.0;
                    plot.line(
                        Line::new("DA", PlotPoints::from(vec![[x, *v], [x + 1.0, *v]]))
                            .color(skin.series(1))
                            .width(2.0),
                    );
                }
                if !rt_pts.is_empty() {
                    plot.points(
                        egui_plot::Points::new("RT", PlotPoints::from(hour_of_day(&rt_pts)))
                            .color(skin.series(0))
                            .radius(1.8),
                    );
                }
            });
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn routes_and_days() {
        assert_eq!(open(&[]).unwrap().route(), Route::code("BCH"));
        assert_eq!(
            open(&["2026-09-30".into()]).unwrap().route(),
            Route::new("BCH", ["2026-09-30"])
        );
        assert!(open(&["2".into()]).is_ok(), "days ago");
        assert!(open(&["last week".into()]).is_err());
    }
}
