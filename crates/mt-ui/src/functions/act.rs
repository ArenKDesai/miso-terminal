//! ACT: the paper account's activity. Fills, dividends, fees, transfers,
//! option exercises, assignments and expiries, newest first, filtered by
//! kind or symbol; and above them, order events as they stream in.

use egui::{RichText, Ui};
use egui_extras::{Column, TableBuilder};
use mt_core::account::{Activity, ActivityCategory};

use crate::context::PanelCx;
use crate::function::{Category, FunctionSpec, Panel, Route};
use crate::market::{self, fmt};
use crate::portfolio;
use crate::widgets::{self, csv};

pub const SPEC: FunctionSpec = FunctionSpec {
    code: "ACT",
    aliases: &["ACTIVITY", "FILLS", "TXN"],
    name: "Account activity",
    category: Category::Account,
    usage: "ACT [FILLS | DIV | FEES | TRANSFERS | OPTIONS]",
    description: "The Alpaca paper account's history: fills, dividends, fees, transfers, and option exercises, assignments and expiries, newest first, with order events as they stream (ACT FILLS, ACT DIV).",
    takes_node: false,
    takes_security: false,
    takes_option: false,
    open,
};

/// Order events shown above the history.
const EVENTS_SHOWN: usize = 5;

fn parse_filter(s: &str) -> Option<ActivityCategory> {
    Some(match s.trim().to_ascii_uppercase().as_str() {
        "FILL" | "FILLS" | "TRADES" => ActivityCategory::Fill,
        "DIV" | "DIVS" | "DIVIDEND" | "DIVIDENDS" => ActivityCategory::Dividend,
        "INT" | "INTEREST" => ActivityCategory::Interest,
        "FEE" | "FEES" => ActivityCategory::Fee,
        "TRANSFER" | "TRANSFERS" | "CASH" => ActivityCategory::Transfer,
        "OPT" | "OPTION" | "OPTIONS" => ActivityCategory::Option,
        "CA" | "CORP" | "ACTIONS" => ActivityCategory::CorporateAction,
        "OTHER" => ActivityCategory::Other,
        _ => return None,
    })
}

fn filter_arg(c: ActivityCategory) -> &'static str {
    match c {
        ActivityCategory::Fill => "FILLS",
        ActivityCategory::Dividend => "DIV",
        ActivityCategory::Interest => "INTEREST",
        ActivityCategory::Fee => "FEES",
        ActivityCategory::Transfer => "TRANSFERS",
        ActivityCategory::Option => "OPTIONS",
        ActivityCategory::CorporateAction => "CA",
        ActivityCategory::Other => "OTHER",
    }
}

fn open(args: &[String]) -> Result<Box<dyn Panel>, String> {
    let filter = match args.first() {
        Some(a) => Some(parse_filter(a).ok_or_else(|| {
            format!("unknown kind {a:?}: use FILLS, DIV, FEES, TRANSFERS or OPTIONS")
        })?),
        None => None,
    };
    Ok(Box::new(ActivityPanel {
        filter,
        search: String::new(),
    }))
}

struct ActivityPanel {
    filter: Option<ActivityCategory>,
    search: String,
}

impl Panel for ActivityPanel {
    fn title(&self) -> String {
        match self.filter {
            None => "ACT".into(),
            Some(c) => format!("ACT {}", filter_arg(c)),
        }
    }

    fn route(&self) -> Route {
        match self.filter {
            None => Route::code("ACT"),
            Some(c) => Route::new("ACT", [filter_arg(c)]),
        }
    }

    fn absorb(&mut self, args: &[String]) -> bool {
        self.filter = args.first().and_then(|a| parse_filter(a));
        true
    }

    fn ui(&mut self, ui: &mut Ui, cx: &mut PanelCx<'_>) {
        let skin = cx.skin;
        let ready = cx.alpaca.is_ready();
        let acts = ready.then(|| cx.hub.watch(&cx.alpaca.activities()));
        widgets::title_bar(
            ui,
            skin,
            &format!("Activity · Alpaca {} account", cx.alpaca.mode().name()),
            |ui| {
                if let Some(a) = &acts {
                    widgets::freshness(ui, skin, a);
                }
            },
        );
        if market::needs_keys(ui, cx) {
            return;
        }
        let Some(acts) = acts else { return };
        let trades = cx
            .hub
            .watch_stream(&cx.alpaca.trade_stream(), &[mt_alpaca::TRADE_UPDATES]);
        self.events(ui, cx, &trades);

        let all: &[Activity] = acts.data().map_or(&[], |a| a.items.as_slice());
        ui.horizontal_wrapped(|ui| {
            if ui.selectable_label(self.filter.is_none(), "All").clicked() {
                self.filter = None;
            }
            for c in ActivityCategory::ALL {
                let n = all.iter().filter(|a| a.category() == c).count();
                if n == 0 && self.filter != Some(c) {
                    continue;
                }
                if ui
                    .selectable_label(self.filter == Some(c), format!("{} {n}", c.label()))
                    .clicked()
                {
                    self.filter = Some(c);
                }
            }
            ui.add_space(8.0);
            ui.add(
                egui::TextEdit::singleline(&mut self.search)
                    .hint_text("symbol…")
                    .desired_width(110.0),
            );
        });
        let needle = self.search.trim().to_ascii_uppercase();
        let rows: Vec<&Activity> = all
            .iter()
            .filter(|a| self.filter.is_none_or(|c| a.category() == c))
            .filter(|a| {
                needle.is_empty()
                    || a.symbol.as_deref().is_some_and(|s| s.contains(&needle))
                    || a.contract().is_some_and(|c| c.underlying.contains(&needle))
            })
            .collect();
        widgets::with_data(ui, skin, &acts, |ui, list| {
            ui.horizontal_wrapped(|ui| {
                ui.label(
                    RichText::new(format!(
                        "{} of {} activities{}",
                        rows.len(),
                        list.items.len(),
                        if list.more {
                            " (older ones not loaded)"
                        } else {
                            ""
                        }
                    ))
                    .small()
                    .color(skin.text_muted),
                );
                csv::copy_button(ui, skin, || to_csv(&rows));
            });
            if rows.is_empty() {
                ui.add_space(8.0);
                ui.label(RichText::new("Nothing here.").color(skin.text_muted));
                return;
            }
            self.table(ui, cx, &rows);
        });
    }
}

impl ActivityPanel {
    /// The latest order events from the stream, if any arrived.
    fn events(
        &self,
        ui: &mut Ui,
        cx: &PanelCx<'_>,
        trades: &mt_data::Snapshot<mt_alpaca::LiveTrades>,
    ) {
        let skin = cx.skin;
        let Some(live) = trades.data() else { return };
        widgets::section(ui, skin, "Order events (live)");
        ui.horizontal_wrapped(|ui| {
            let (color, state) = match (&trades.error, live.listening) {
                (Some(e), _) => (skin.negative, format!("stream: {e}")),
                (None, true) => (skin.live, "listening".to_owned()),
                (None, false) => (skin.text_muted, "connecting…".to_owned()),
            };
            widgets::lamp(ui, color);
            ui.label(RichText::new(state).small().color(skin.text_muted));
        });
        if live.events.is_empty() {
            ui.label(
                RichText::new("No order events since the terminal connected.")
                    .small()
                    .color(skin.text_muted),
            );
            return;
        }
        egui::Grid::new("act-events")
            .num_columns(5)
            .spacing([14.0, 2.0])
            .show(ui, |ui| {
                for e in live.events.iter().take(EVENTS_SHOWN) {
                    ui.label(
                        RichText::new(e.time.map_or_else(|| fmt::DASH.to_owned(), fmt::time))
                            .monospace()
                            .color(skin.text_muted),
                    );
                    let color = if e.is_fill() {
                        skin.live
                    } else if matches!(e.event.as_str(), "rejected" | "canceled" | "expired") {
                        skin.warning
                    } else {
                        skin.text
                    };
                    ui.label(RichText::new(e.event_label()).color(color));
                    ui.label(RichText::new(&e.symbol).strong());
                    let side = e.side.map_or("", |s| s.label());
                    let what = match (e.fill_qty, e.price) {
                        (Some(q), Some(p)) if e.is_fill() => {
                            format!("{side} {} @ {}", portfolio::qty(q), portfolio::price(p))
                        }
                        _ => format!(
                            "{side} {} {}",
                            e.qty.map(portfolio::qty).unwrap_or_default(),
                            e.order_type.as_deref().unwrap_or_default()
                        ),
                    };
                    ui.label(what);
                    ui.label(
                        RichText::new(e.position_qty.map_or_else(String::new, |q| {
                            format!("position {}", portfolio::qty(q))
                        }))
                        .small()
                        .color(skin.text_muted),
                    );
                    ui.end_row();
                }
            });
        ui.add_space(4.0);
    }

    fn table(&self, ui: &mut Ui, cx: &mut PanelCx<'_>, rows: &[&Activity]) {
        let skin = cx.skin;
        let row_h = 22.0;
        let mut open = None;
        egui::ScrollArea::horizontal()
            .id_salt("act-h")
            .show(ui, |ui| {
                let height = (ui.available_height() - 4.0).max(row_h * 4.0);
                TableBuilder::new(ui)
                    .id_salt("act-table")
                    .striped(true)
                    .max_scroll_height(height)
                    .cell_layout(egui::Layout::left_to_right(egui::Align::Center))
                    .column(Column::initial(118.0).at_least(96.0))
                    .column(Column::initial(150.0).at_least(100.0))
                    .column(Column::initial(150.0).at_least(80.0))
                    .column(Column::initial(48.0))
                    .columns(Column::initial(76.0).at_least(56.0), 2)
                    .column(Column::initial(100.0).at_least(80.0))
                    .column(Column::remainder().at_least(120.0).clip(true))
                    .header(row_h, |mut h| {
                        for name in [
                            "When", "Kind", "Symbol", "Side", "Qty", "Price", "Amount", "Details",
                        ] {
                            h.col(|ui| {
                                widgets::label(ui, skin, name);
                            });
                        }
                    })
                    .body(|body| {
                        body.rows(row_h, rows.len(), |mut row| {
                            let a = rows[row.index()];
                            row.col(|ui| {
                                ui.label(RichText::new(when(a)).monospace().color(skin.text_muted));
                            });
                            row.col(|ui| {
                                let color = match a.category() {
                                    ActivityCategory::Fill => skin.text_strong,
                                    ActivityCategory::Dividend | ActivityCategory::Interest => {
                                        skin.positive
                                    }
                                    ActivityCategory::Fee => skin.negative,
                                    ActivityCategory::Option => skin.info,
                                    _ => skin.text,
                                };
                                ui.label(RichText::new(a.name()).color(color));
                            });
                            row.col(|ui| {
                                let Some(sym) = &a.symbol else { return };
                                match a.contract() {
                                    Some(c) => {
                                        ui.label(portfolio::contract_name(&c)).on_hover_text(sym);
                                    }
                                    None => {
                                        let name = format!("{sym} US");
                                        if widgets::link(ui, skin, &name).clicked() {
                                            open = Some(Route::new("GP", [name]));
                                        }
                                    }
                                }
                            });
                            row.col(|ui| {
                                if let Some(s) = a.side {
                                    ui.label(RichText::new(s.label()).color(match s {
                                        mt_core::account::OrderSide::Buy => skin.positive,
                                        mt_core::account::OrderSide::Sell => skin.negative,
                                    }));
                                }
                            });
                            row.col(|ui| {
                                crate::widgets::table::num_cell(
                                    ui,
                                    a.qty.map(portfolio::qty).unwrap_or_default(),
                                )
                            });
                            row.col(|ui| {
                                let p = a.price.or(a.per_share_amount).filter(|p| !p.is_zero());
                                crate::widgets::table::num_cell(
                                    ui,
                                    p.map(portfolio::price).unwrap_or_default(),
                                )
                            });
                            row.col(|ui| {
                                let amount = a.amount();
                                crate::widgets::table::num_cell(
                                    ui,
                                    RichText::new(
                                        amount.map_or_else(String::new, portfolio::usd_signed),
                                    )
                                    .color(portfolio::delta_color(skin, amount)),
                                )
                            });
                            row.col(|ui| {
                                ui.label(RichText::new(details(a)).small().color(skin.text_muted));
                            });
                        });
                    });
            });
        if let Some(r) = open {
            cx.open(r);
        }
    }
}

/// `Oct 02 14:31` (New York) for a fill, the date for the rest.
fn when(a: &Activity) -> String {
    match (a.time, a.date) {
        (Some(t), _) if a.category() == ActivityCategory::Fill => mt_core::exchange::to_exchange(t)
            .format("%b %d %H:%M")
            .to_string(),
        (_, Some(d)) => d.format("%b %d %Y").to_string(),
        (Some(t), None) => mt_core::exchange::to_exchange(t)
            .format("%b %d %Y")
            .to_string(),
        _ => fmt::DASH.to_owned(),
    }
}

fn details(a: &Activity) -> String {
    let mut parts = Vec::new();
    if a.category() == ActivityCategory::Fill {
        if let (Some(cum), Some(left)) = (a.cum_qty, a.leaves_qty)
            && !left.is_zero()
        {
            parts.push(format!(
                "{} filled, {} to go",
                portfolio::qty(cum),
                portfolio::qty(left)
            ));
        }
        if let Some(s) = &a.order_status {
            parts.push(format!("order {}", s.replace('_', " ")));
        }
    }
    if !a.description.is_empty() {
        parts.push(a.description.clone());
    }
    if let Some(s) = a.status.as_deref().filter(|s| *s != "executed") {
        parts.push(s.to_owned());
    }
    parts.join(" · ")
}

fn to_csv(rows: &[&Activity]) -> String {
    let n = |v: Option<mt_core::money::Decimal>| {
        v.map_or_else(String::new, |v| v.normalize().to_string())
    };
    csv::to_csv(
        &[
            "id",
            "type",
            "time_utc",
            "date",
            "symbol",
            "side",
            "qty",
            "price",
            "amount",
            "description",
        ],
        rows.iter().map(|a| {
            vec![
                a.id.clone(),
                a.code.clone(),
                a.time.map_or_else(String::new, |t| t.to_rfc3339()),
                a.date.map_or_else(String::new, |d| d.to_string()),
                a.symbol.clone().unwrap_or_default(),
                a.side.map_or("", |s| s.label()).to_owned(),
                n(a.qty),
                n(a.price),
                n(a.amount()),
                a.description.clone(),
            ]
        }),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn filters_round_trip() {
        for c in ActivityCategory::ALL {
            assert_eq!(parse_filter(filter_arg(c)), Some(c));
        }
        assert_eq!(parse_filter("dividends"), Some(ActivityCategory::Dividend));
        assert!(open(&["NOPE".into()]).is_err());
    }
}
