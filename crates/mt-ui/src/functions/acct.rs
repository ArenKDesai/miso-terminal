//! ACCT: the paper account itself. Its standing (status and any blocks),
//! balances, buying power, margin, the pattern-day-trader flag with the
//! day trades used, and the options level.

use egui::{Grid, RichText, Ui};
use mt_core::account::{Account, PDT_DAY_TRADES, PDT_MIN_EQUITY, mask_number, options_level_label};
use mt_core::money::Decimal;

use crate::context::PanelCx;
use crate::function::{Category, FunctionSpec, Panel, Route};
use crate::market;
use crate::portfolio;
use crate::skin::Skin;
use crate::widgets;

pub const SPEC: FunctionSpec = FunctionSpec {
    code: "ACCT",
    aliases: &["ACCOUNT", "BAL", "MARGIN"],
    name: "Account",
    category: Category::Account,
    usage: "ACCT",
    description: "The Alpaca paper account: status and any restrictions, balances, buying power, margin, the pattern-day-trader flag and day trades used, and the options level.",
    takes_node: false,
    takes_security: false,
    takes_option: false,
    open,
};

fn open(_: &[String]) -> Result<Box<dyn Panel>, String> {
    Ok(Box::new(AccountPanel))
}

struct AccountPanel;

impl Panel for AccountPanel {
    fn title(&self) -> String {
        "ACCT".into()
    }

    fn route(&self) -> Route {
        Route::code("ACCT")
    }

    fn absorb(&mut self, _args: &[String]) -> bool {
        true
    }

    fn ui(&mut self, ui: &mut Ui, cx: &mut PanelCx<'_>) {
        let skin = cx.skin;
        let mode = cx.alpaca.mode();
        let account = if cx.alpaca.is_ready() {
            Some(cx.hub.watch(&cx.alpaca.account()))
        } else {
            None
        };
        widgets::title_bar(
            ui,
            skin,
            &format!("Account · Alpaca {}", mode.name()),
            |ui| {
                if let Some(a) = &account {
                    widgets::freshness(ui, skin, a);
                }
            },
        );
        if market::needs_keys(ui, cx) {
            return;
        }
        let Some(snap) = account else { return };
        let mut open = None;
        egui::ScrollArea::vertical()
            .auto_shrink(false)
            .show(ui, |ui| {
                widgets::with_data(ui, skin, &snap, |ui, a| {
                    standing(ui, skin, a, mode);
                    ui.horizontal_wrapped(|ui| {
                        ui.vertical(|ui| {
                            balances(ui, skin, a);
                            buying_power(ui, skin, a);
                        });
                        ui.add_space(24.0);
                        ui.vertical(|ui| {
                            margin(ui, skin, a);
                            day_trading(ui, skin, a);
                            options(ui, skin, a);
                        });
                    });
                    ui.add_space(8.0);
                    ui.horizontal(|ui| {
                        for (label, code) in [
                            ("Positions → PORT", "PORT"),
                            ("Equity curve → PNL", "PNL"),
                            ("Activity → ACT", "ACT"),
                        ] {
                            if widgets::link(ui, skin, label).clicked() {
                                open = Some(Route::code(code));
                            }
                            ui.add_space(12.0);
                        }
                    });
                });
            });
        if let Some(r) = open {
            cx.open(r);
        }
    }
}

fn row(ui: &mut Ui, skin: &Skin, label: &str, value: impl Into<RichText>, note: &str) {
    ui.label(RichText::new(label).color(skin.text_muted));
    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
        ui.label(value.into().monospace());
    });
    ui.label(RichText::new(note).small().color(skin.text_muted));
    ui.end_row();
}

fn money_row(ui: &mut Ui, skin: &Skin, label: &str, v: Option<Decimal>, note: &str) {
    row(
        ui,
        skin,
        label,
        RichText::new(portfolio::usd_opt(v)).color(skin.text_strong),
        note,
    );
}

fn grid(ui: &mut Ui, id: &str, body: impl FnOnce(&mut Ui)) {
    Grid::new(id)
        .num_columns(3)
        .spacing([16.0, 4.0])
        .min_col_width(60.0)
        .show(ui, body);
}

fn standing(ui: &mut Ui, skin: &Skin, a: &Account, mode: mt_alpaca::AccountMode) {
    ui.horizontal_wrapped(|ui| {
        let (status_color, status) = if a.is_active() {
            (skin.live, a.status.to_ascii_lowercase())
        } else {
            (
                skin.warning,
                a.status.to_ascii_lowercase().replace('_', " "),
            )
        };
        widgets::lamp(ui, status_color);
        ui.label(
            RichText::new(format!("{} · {}", mode.band(), status))
                .strong()
                .color(status_color),
        );
        ui.label(
            RichText::new(format!(
                "account {} · {}{}",
                mask_number(&a.number),
                a.currency,
                a.created
                    .map(|t| format!(" · opened {}", t.format("%b %d %Y")))
                    .unwrap_or_default()
            ))
            .color(skin.text_muted),
        )
        .on_hover_text("The account number is masked so screenshots never carry it.");
    });
    let blocks = a.restrictions();
    if !blocks.is_empty() {
        ui.label(
            RichText::new(format!("⚠ {}", blocks.join(" · ")))
                .strong()
                .color(skin.negative),
        );
    }
}

fn balances(ui: &mut Ui, skin: &Skin, a: &Account) {
    widgets::section(ui, skin, "Balances");
    grid(ui, "acct-balances", |ui| {
        money_row(ui, skin, "Equity", Some(a.equity), "cash + long + short");
        money_row(
            ui,
            skin,
            "Previous close",
            Some(a.last_equity),
            "equity at the last close",
        );
        row(
            ui,
            skin,
            "Today",
            portfolio::pl_text(skin, a.day_pl(), a.day_pl_pct()),
            "deposits included",
        );
        money_row(ui, skin, "Cash", Some(a.cash), "");
        money_row(ui, skin, "Long market value", Some(a.long_market_value), "");
        money_row(
            ui,
            skin,
            "Short market value",
            Some(a.short_market_value),
            "",
        );
        if a.accrued_fees.is_some_and(|f| !f.is_zero()) {
            money_row(ui, skin, "Accrued fees", a.accrued_fees, "");
        }
        for (label, v) in [
            ("Transfers in", a.pending_transfer_in),
            ("Transfers out", a.pending_transfer_out),
        ] {
            if v.is_some_and(|v| !v.is_zero()) {
                money_row(ui, skin, label, v, "pending");
            }
        }
    });
}

fn buying_power(ui: &mut Ui, skin: &Skin, a: &Account) {
    widgets::section(ui, skin, "Buying power");
    grid(ui, "acct-bp", |ui| {
        money_row(
            ui,
            skin,
            "Buying power",
            Some(a.buying_power),
            "what an order can use now",
        );
        money_row(ui, skin, "Reg T", a.regt_buying_power, "overnight");
        money_row(
            ui,
            skin,
            "Day trading",
            a.daytrading_buying_power,
            "pattern day traders only",
        );
        money_row(
            ui,
            skin,
            "Non-marginable",
            a.non_marginable_buying_power,
            "cash for securities that cannot be margined",
        );
        money_row(ui, skin, "Options", a.options_buying_power, "");
    });
}

fn margin(ui: &mut Ui, skin: &Skin, a: &Account) {
    widgets::section(ui, skin, "Margin");
    grid(ui, "acct-margin", |ui| {
        row(
            ui,
            skin,
            "Type",
            RichText::new(a.margin_label()).color(skin.text_strong),
            "",
        );
        money_row(
            ui,
            skin,
            "Initial margin",
            a.initial_margin,
            "to open the positions",
        );
        money_row(
            ui,
            skin,
            "Maintenance",
            a.maintenance_margin,
            "to keep them",
        );
        money_row(
            ui,
            skin,
            "Maintenance at close",
            a.last_maintenance_margin,
            "",
        );
        if let Some(excess) = a.excess_equity() {
            let used = a.margin_used();
            let color = match used {
                Some(u) if u >= 0.8 => skin.negative,
                Some(u) if u >= 0.5 => skin.warning,
                _ => skin.text_strong,
            };
            row(
                ui,
                skin,
                "Excess equity",
                RichText::new(portfolio::usd(excess)).color(color),
                &used.map_or_else(String::new, |u| {
                    format!("maintenance is {:.0}% of equity", u * 100.0)
                }),
            );
        }
        money_row(ui, skin, "SMA", a.sma, "special memorandum account");
        row(
            ui,
            skin,
            "Short selling",
            RichText::new(if a.shorting_enabled { "allowed" } else { "off" })
                .color(skin.text_strong),
            "",
        );
    });
}

fn day_trading(ui: &mut Ui, skin: &Skin, a: &Account) {
    widgets::section(ui, skin, "Day trading");
    grid(ui, "acct-pdt", |ui| {
        row(
            ui,
            skin,
            "Pattern day trader",
            RichText::new(if a.pattern_day_trader { "yes" } else { "no" }).color(
                if a.pattern_day_trader {
                    skin.warning
                } else {
                    skin.text_strong
                },
            ),
            "four or more day trades in five business days",
        );
        let (text, color, note) = match a.day_trades_left() {
            Some(0) => (
                format!("{} of {PDT_DAY_TRADES}", a.daytrade_count),
                skin.negative,
                "another day trade would flag the account".to_owned(),
            ),
            Some(left) => (
                format!("{} of {PDT_DAY_TRADES}", a.daytrade_count),
                if left == 1 {
                    skin.warning
                } else {
                    skin.text_strong
                },
                format!("{left} left in the five-day window"),
            ),
            None => (
                a.daytrade_count.to_string(),
                skin.text_strong,
                format!(
                    "no limit with equity of {} or more",
                    portfolio::usd(PDT_MIN_EQUITY)
                ),
            ),
        };
        row(
            ui,
            skin,
            "Day trades (5 days)",
            RichText::new(text).color(color),
            &note,
        );
    });
}

fn options(ui: &mut Ui, skin: &Skin, a: &Account) {
    widgets::section(ui, skin, "Options");
    grid(ui, "acct-options", |ui| {
        for (label, level) in [
            ("Approved level", a.options_approved_level),
            ("Trading level", a.options_trading_level),
        ] {
            row(
                ui,
                skin,
                label,
                RichText::new(
                    level.map_or_else(|| market::fmt::DASH.to_owned(), |l| l.to_string()),
                )
                .color(skin.text_strong),
                level.map_or("", options_level_label),
            );
        }
    });
}
