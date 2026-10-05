//! CMP: several nodes on one chart. Today at five minutes, or hourly RT or DA
//! over N days, with a summary row per node. With securities among the
//! arguments (`CMP XEL US MINN.HUB 30`) it opens `cross_chart` instead.

use egui::{RichText, Ui};

use mt_core::instrument::Security;

use super::cross_chart;
use super::gp::parse_days;
use crate::context::PanelCx;
use crate::function::{Category, FunctionSpec, Panel, Route};
use crate::market::{self, SecurityPicker};
use crate::series::{self, Component, Points};
use crate::widgets::node_picker::NodePicker;
use crate::widgets::{self, chart, csv, fmt};

pub const SPEC: FunctionSpec = FunctionSpec {
    code: "CMP",
    aliases: &["COMPARE", "OVERLAY"],
    name: "Compare",
    category: Category::Prices,
    usage: "CMP <node|security> … [days]",
    description: "Up to eight nodes on one chart: today's five-minute RT, or hourly RT or DA over N days, by component. With securities (CMP XEL US MINN.HUB 30), their prices above the nodes' on one time axis, and how they moved together day by day.",
    takes_node: true,
    takes_security: true,
    takes_option: false,
    open,
};

const MAX_NODES: usize = 8;

fn open(args: &[String]) -> Result<Box<dyn Panel>, String> {
    // Numbers are the day count; securities go above; everything else is a node.
    let securities: Vec<Security> = args.iter().filter_map(|a| market::security_of(a)).collect();
    let args: Vec<String> = args
        .iter()
        .filter(|a| market::security_of(a).is_none())
        .cloned()
        .collect();
    let (days, nodes): (Vec<&String>, Vec<&String>) =
        args.iter().partition(|a| a.parse::<u32>().is_ok());
    let mut list: Vec<String> = Vec::new();
    for n in nodes {
        let n = n.trim().to_ascii_uppercase();
        if !n.is_empty() && !list.contains(&n) && list.len() < MAX_NODES {
            list.push(n);
        }
    }
    let days = parse_days(days.first().copied())?;
    if !securities.is_empty() {
        let mut unique: Vec<Security> = Vec::new();
        for s in securities {
            if !unique.contains(&s) && unique.len() < cross_chart::MAX_SECURITIES {
                unique.push(s);
            }
        }
        list.truncate(cross_chart::MAX_NODES);
        return Ok(cross_chart::open(unique, list, days));
    }
    Ok(Box::new(Compare {
        nodes: list,
        days,
        history: days > 0,
        da: false,
        component: Component::Lmp,
        picker: NodePicker::default(),
        security_picker: SecurityPicker::default(),
    }))
}

struct Compare {
    nodes: Vec<String>,
    days: u32,
    /// Hourly history instead of today's five-minute prices.
    history: bool,
    /// In history, compare DA rather than RT.
    da: bool,
    component: Component,
    picker: NodePicker,
    security_picker: SecurityPicker,
}

impl Panel for Compare {
    fn title(&self) -> String {
        match self.nodes.len() {
            0 => "CMP".into(),
            n if n <= 3 => format!(
                "CMP {}",
                self.nodes
                    .iter()
                    .map(|n| mt_core::hub_short(n))
                    .collect::<Vec<_>>()
                    .join(" ")
            ),
            n => format!("CMP {n} nodes"),
        }
    }

    fn route(&self) -> Route {
        let mut args = self.nodes.clone();
        if self.history {
            args.push(self.days.max(1).to_string());
        }
        Route::new("CMP", args)
    }

    fn ui(&mut self, ui: &mut Ui, cx: &mut PanelCx<'_>) {
        let skin = cx.skin;
        if self.days == 0 {
            self.days = cx.config.data.history_days.clamp(1, 90);
        }
        let mut open_cross = None;
        ui.horizontal_wrapped(|ui| {
            let mut remove = None;
            for (i, n) in self.nodes.iter().enumerate() {
                widgets::lamp(ui, skin.series(i));
                ui.label(RichText::new(n).color(skin.text_strong));
                if ui.small_button("✕").clicked() {
                    remove = Some(i);
                }
                ui.add_space(6.0);
            }
            if let Some(i) = remove {
                self.nodes.remove(i);
            }
            if self.nodes.len() < MAX_NODES
                && let Some(n) = self.picker.show(ui, cx, "cmp", "add a node…")
                && !self.nodes.contains(&n)
            {
                self.nodes.push(n);
            }
            // A security turns this into a stock-against-nodes chart.
            if cx.alpaca.is_ready()
                && let Some(s) = self
                    .security_picker
                    .show(ui, cx, "cmp-add-sec", "add a security…")
            {
                let mut args = vec![s.to_string()];
                args.extend(self.nodes.iter().take(cross_chart::MAX_NODES).cloned());
                args.push(self.days.max(1).to_string());
                open_cross = Some(Route::new("CMP", args));
            }
        });
        if let Some(route) = open_cross {
            cx.open(route);
        }
        if self.nodes.is_empty() {
            ui.label(
                RichText::new("Add nodes above, or type CMP MINN.HUB MICHIGAN.HUB ILLINOIS.HUB.")
                    .color(skin.text_muted),
            );
            return;
        }
        ui.horizontal_wrapped(|ui| {
            ui.selectable_value(&mut self.history, false, "Today · 5-min");
            ui.selectable_value(&mut self.history, true, "History · hourly");
            ui.separator();
            for c in Component::ALL {
                ui.selectable_value(&mut self.component, c, c.label());
            }
            if self.history {
                ui.separator();
                ui.selectable_value(&mut self.da, false, "RT");
                ui.selectable_value(&mut self.da, true, "DA");
                ui.separator();
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
            }
        });

        let lines: Vec<(String, Points)> = self
            .nodes
            .iter()
            .map(|n| {
                let pts = if self.history {
                    let h = series::node_history(cx, n, self.component, self.days);
                    if self.da { h.da } else { h.rt }
                } else {
                    series::node_today(cx, n, self.component).rt_5min
                };
                (n.clone(), pts)
            })
            .collect();

        if lines.iter().all(|(_, p)| p.is_empty()) {
            let loading = if self.history {
                self.nodes
                    .iter()
                    .any(|n| series::node_history(cx, n, self.component, self.days).pending > 0)
            } else {
                cx.hub.peek(&cx.miso.rt_intraday()).data.is_none()
            };
            if loading {
                widgets::placeholder(ui, skin, None);
            } else {
                ui.label(RichText::new("No prices for these nodes yet.").color(skin.warning));
            }
            return;
        }

        // One summary line per node: latest, average, range.
        egui::Grid::new("cmp-summary")
            .striped(true)
            .num_columns(5)
            .spacing([16.0, 3.0])
            .show(ui, |ui| {
                for h in ["Node", "Latest", "Average", "Max", "Min"] {
                    widgets::label(ui, skin, h);
                }
                ui.end_row();
                for (i, (n, pts)) in lines.iter().enumerate() {
                    ui.horizontal(|ui| {
                        widgets::lamp(ui, skin.series(i));
                        ui.label(n);
                    });
                    let vals: Vec<f64> = pts.iter().map(|p| p.1).collect();
                    ui.label(fmt::price_opt(pts.last().map(|p| p.1)));
                    ui.label(fmt::price_opt(series::mean(vals.iter().copied())));
                    ui.label(fmt::price_opt(vals.iter().copied().max_by(f64::total_cmp)));
                    ui.label(fmt::price_opt(vals.iter().copied().min_by(f64::total_cmp)));
                    ui.end_row();
                }
            });
        csv::copy_button(ui, skin, || {
            let mut headers = vec!["time_est"];
            headers.extend(lines.iter().map(|(n, _)| n.as_str()));
            let mut times: Vec<_> = lines
                .iter()
                .flat_map(|(_, p)| p.iter().map(|x| x.0))
                .collect();
            times.sort();
            times.dedup();
            let maps: Vec<std::collections::HashMap<_, _>> = lines
                .iter()
                .map(|(_, p)| p.iter().copied().collect())
                .collect();
            csv::to_csv(
                &headers,
                times.iter().map(|t| {
                    std::iter::once(t.format("%Y-%m-%d %H:%M").to_string())
                        .chain(maps.iter().map(|m| {
                            m.get(t)
                                .map_or_else(String::new, |v: &f64| format!("{v:.2}"))
                        }))
                        .collect()
                }),
            )
        });

        chart::time_plot("cmp-chart", skin).show(ui, |plot| {
            for (i, (n, pts)) in lines.iter().enumerate() {
                if self.history {
                    chart::hourly_steps(plot, n, pts, skin.series(i));
                } else {
                    plot.line(chart::line(n, pts, skin.series(i)));
                }
            }
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn args_split_into_nodes_and_days() {
        let args: Vec<String> = ["minn.hub", "14", "MICHIGAN.HUB", "minn.hub"]
            .map(String::from)
            .to_vec();
        let p = open(&args).unwrap();
        assert_eq!(
            p.route(),
            Route::new("CMP", ["MINN.HUB", "MICHIGAN.HUB", "14"])
        );
        assert_eq!(open(&[]).unwrap().route(), Route::code("CMP"));
        // A security among them: the stock-against-nodes chart.
        let args: Vec<String> = ["XEL US", "minn.hub", "30", "XEL US"]
            .map(String::from)
            .to_vec();
        let p = open(&args).unwrap();
        assert_eq!(p.route(), Route::new("CMP", ["XEL US", "MINN.HUB", "30"]));
    }
}
