//! HUBS: the eight trading hubs side by side over N days. DA and RT averages,
//! the DART spread, on-peak and off-peak blocks, volatility and extremes.

use egui::{RichText, Ui};
use egui_extras::{Column, TableBuilder};
use mt_core::{TRADING_HUBS, hub_short};

use crate::context::PanelCx;
use crate::function::{Category, FunctionSpec, Panel, Route};
use crate::series::{self, Component, Points};
use crate::widgets::table::num_cell;
use crate::widgets::{self, csv, fmt};

pub const SPEC: FunctionSpec = FunctionSpec {
    code: "HUBS",
    aliases: &["HUB", "HUBSTATS"],
    name: "Hub statistics",
    category: Category::Prices,
    usage: "HUBS [days]",
    description: "All eight trading hubs over N days: DA, RT and DART averages, on-peak and off-peak, RT volatility and extremes.",
    takes_node: false,
    takes_security: false,
    open,
};

fn open(args: &[String]) -> Result<Box<dyn Panel>, String> {
    Ok(Box::new(Hubs {
        days: super::gp::parse_days(args.first())?,
    }))
}

struct Hubs {
    days: u32,
}

/// One hub's figures for the window.
struct HubRow {
    hub: &'static str,
    da: Option<f64>,
    rt: Option<f64>,
    dart: Option<f64>,
    da_on: Option<f64>,
    rt_on: Option<f64>,
    da_off: Option<f64>,
    rt_off: Option<f64>,
    rt_sd: Option<f64>,
    rt_max: Option<f64>,
    rt_min: Option<f64>,
    rt_above_da: Option<f64>,
}

fn block_mean(pts: &Points, on_peak: bool) -> Option<f64> {
    series::mean(
        pts.iter()
            .filter(|(t, _)| series::is_on_peak(*t) == on_peak)
            .map(|p| p.1),
    )
}

impl HubRow {
    fn of(hub: &'static str, da: &Points, rt: &Points) -> Self {
        let stats = series::Stats::of(da, rt);
        let rt_vals: Vec<f64> = rt.iter().map(|p| p.1).collect();
        let dart = series::subtract(rt, da);
        Self {
            hub,
            da: stats.da_avg,
            rt: stats.rt_avg,
            dart: stats.dart_avg,
            da_on: block_mean(da, true),
            rt_on: block_mean(rt, true),
            da_off: block_mean(da, false),
            rt_off: block_mean(rt, false),
            rt_sd: series::std_dev(&rt_vals),
            rt_max: stats.rt_max,
            rt_min: stats.rt_min,
            rt_above_da: (!dart.is_empty()).then(|| {
                dart.iter().filter(|p| p.1 > 0.0).count() as f64 / dart.len() as f64 * 100.0
            }),
        }
    }

    fn cells(&self) -> [Option<f64>; 11] {
        [
            self.da,
            self.rt,
            self.dart,
            self.da_on,
            self.rt_on,
            self.da_off,
            self.rt_off,
            self.rt_sd,
            self.rt_max,
            self.rt_min,
            self.rt_above_da,
        ]
    }
}

const HEADERS: [&str; 11] = [
    "DA avg",
    "RT avg",
    "RT − DA",
    "DA on-peak",
    "RT on-peak",
    "DA off-peak",
    "RT off-peak",
    "RT std dev",
    "RT max",
    "RT min",
    "RT > DA",
];

impl Panel for Hubs {
    fn title(&self) -> String {
        format!("HUBS {}d", self.days.max(1))
    }

    fn route(&self) -> Route {
        Route::new("HUBS", [self.days.max(1).to_string()])
    }

    fn ui(&mut self, ui: &mut Ui, cx: &mut PanelCx<'_>) {
        let skin = cx.skin;
        if self.days == 0 {
            self.days = cx.config.data.history_days.clamp(1, 90);
        }
        widgets::title_bar(ui, skin, "Trading hubs · $/MWh", |_| {});
        ui.horizontal(|ui| {
            for d in super::gp::DAY_CHOICES {
                ui.selectable_value(
                    &mut self.days,
                    d,
                    if d == 365 {
                        "1y".into()
                    } else {
                        format!("{d}d")
                    },
                );
            }
        });
        // Each daily report covers every node, so the eight hubs share downloads.
        let (mut pending, mut prelim, mut archived, mut capped) = (0, 0, 0, false);
        let rows: Vec<HubRow> = TRADING_HUBS
            .iter()
            .map(|hub| {
                let h = series::node_history(cx, hub, Component::Lmp, self.days);
                pending = pending.max(h.pending);
                prelim = prelim.max(h.prelim_days);
                archived = archived.max(h.archived_days);
                capped |= h.capped;
                HubRow::of(hub, &h.da, &h.rt)
            })
            .collect();
        let history = series::History {
            da: Vec::new(),
            rt: Vec::new(),
            pending,
            prelim_days: prelim,
            archived_days: archived,
            capped,
        };
        super::gp::history_notes(ui, cx, &history, || {
            let mut headers = vec!["hub"];
            headers.extend(HEADERS);
            csv::to_csv(
                &headers,
                rows.iter().map(|r| {
                    std::iter::once(r.hub.to_owned())
                        .chain(
                            r.cells()
                                .iter()
                                .map(|v| v.map_or_else(String::new, |v| format!("{v:.2}"))),
                        )
                        .collect()
                }),
            )
        });

        let row_h = ui.text_style_height(&egui::TextStyle::Body) + 4.0;
        let mut open = None;
        // Eleven columns overflow a narrow pane; scroll sideways rather than widen it.
        egui::ScrollArea::horizontal()
            .id_salt("hubs-scroll")
            .show(ui, |ui| {
                TableBuilder::new(ui)
                    .id_salt("hubs-table")
                    .striped(true)
                    .cell_layout(egui::Layout::left_to_right(egui::Align::Center))
                    .column(Column::initial(90.0))
                    .columns(Column::initial(76.0).at_least(56.0), HEADERS.len())
                    .header(row_h, |mut h| {
                        h.col(|ui| {
                            widgets::label(ui, skin, "Hub");
                        });
                        for title in HEADERS {
                            h.col(|ui| {
                                widgets::label(ui, skin, title);
                            });
                        }
                    })
                    .body(|mut body| {
                        for r in &rows {
                            body.row(row_h, |mut row| {
                                row.col(|ui| {
                                    if widgets::link(ui, skin, hub_short(r.hub)).clicked() {
                                        open = Some(r.hub);
                                    }
                                });
                                for (i, v) in r.cells().into_iter().enumerate() {
                                    let text = match (i, v) {
                                        (_, None) => {
                                            RichText::new(fmt::DASH).color(skin.text_muted)
                                        }
                                        (2, Some(v)) => {
                                            RichText::new(fmt::signed(v)).color(skin.delta(v))
                                        }
                                        (10, Some(v)) => RichText::new(fmt::pct(v)),
                                        (7, Some(v)) => {
                                            RichText::new(fmt::price(v)).color(skin.text_muted)
                                        }
                                        (_, Some(v)) => {
                                            RichText::new(fmt::price(v)).color(cx.price_color(v))
                                        }
                                    };
                                    row.col(|ui| num_cell(ui, text));
                                }
                            });
                        }
                    });
            });
        if let Some(hub) = open {
            cx.open(Route::new("GP", [hub.to_owned(), self.days.to_string()]));
        }
        ui.label(
            RichText::new(
                "On-peak is HE 8 to HE 23 EST, Monday to Friday (NERC holidays not excluded). RT for today \
                 comes from the five-minute feed; earlier days from the RT final or preliminary reports. \
                 Click a hub for its chart.",
            )
            .small()
            .color(skin.text_muted),
        );
    }
}
