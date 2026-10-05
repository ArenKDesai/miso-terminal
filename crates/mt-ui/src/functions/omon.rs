//! OMON: the option monitor. One underlying's chain for one expiry, calls to
//! the left of the strikes and puts to the right, Bloomberg style: bid, ask,
//! last and its change, volume, open interest, implied volatility and delta
//! (gamma, theta and vega on request). In-the-money contracts are shaded and
//! a line marks where the stock trades. Expiries are tabs; the strikes shown
//! are those nearest the money. Clicking a bid opens a ticket to sell one
//! contract there, clicking an ask one to buy, and right-clicking a contract
//! offers both, adds it to a spread, or builds a spread from that strike
//! (verticals, a straddle, a strangle, an iron condor, butterflies) for the
//! MLEG ticket. Nothing is sent until a ticket is confirmed.
//!
//! Prices come from Alpaca's indicative feed on the free plan and are re-read
//! every minute; the contract list (expiries, open interest) every hour.

use chrono::NaiveDate;
use egui::{RichText, Ui};
use egui_extras::{Column, TableBuilder};
use mt_core::account::{OrderSide, from_f64};
use mt_core::equity::OptionSnapshot;
use mt_core::instrument::{OptionContract, OptionRight};
use mt_core::options::{self, ChainRow, ContractList, Leg};

use crate::context::PanelCx;
use crate::function::{Category, FunctionSpec, Panel, Route};
use crate::market::{self, SecurityPicker, fmt};
use crate::options::{self as opt, FEED_NOTE};
use crate::widgets::{self, csv};

pub const SPEC: FunctionSpec = FunctionSpec {
    code: "OMON",
    aliases: &["OPTIONS", "CHAIN"],
    name: "Option monitor",
    category: Category::Markets,
    usage: "OMON <ticker> US [expiry YYYY-MM-DD] [strikes each side | ALL], or OMON <OCC symbol>",
    description: "An option chain from Alpaca: calls and puts by strike for one expiry, with bid, ask, last, volume, open interest, implied volatility and greeks, in-the-money contracts shaded (OMON XLU US, OMON XLU US 2026-12-18 ALL, OMON XLU261218C00045000).",
    takes_node: false,
    takes_security: true,
    takes_option: true,
    open,
};

/// Strikes shown each side of the money by default.
const NEAR: usize = 10;

fn open(args: &[String]) -> Result<Box<dyn Panel>, String> {
    let mut m = Omon {
        underlying: None,
        expiry: None,
        focus: None,
        strikes: Some(NEAR),
        greeks: false,
        picker: SecurityPicker::default(),
        scroll_to_focus: false,
        spread: Vec::new(),
    };
    for a in args {
        if let Some(sec) = market::security_of(a) {
            m.underlying = Some(sec.ticker);
        } else if let Some(c) = OptionContract::parse_occ(a) {
            m.underlying = Some(c.underlying.clone());
            m.expiry = Some(c.expiry);
            m.focus = c.occ();
            m.scroll_to_focus = true;
        } else if let Ok(d) = NaiveDate::parse_from_str(a, "%Y-%m-%d") {
            m.expiry = Some(d);
        } else if a.eq_ignore_ascii_case("ALL") {
            m.strikes = None;
        } else if let Some(n) = a.parse::<usize>().ok().filter(|n| (1..=200).contains(n)) {
            m.strikes = Some(n);
        } else {
            return Err(format!(
                "{a:?} is not an expiry (YYYY-MM-DD), a number of strikes or ALL. Usage: {}",
                SPEC.usage
            ));
        }
    }
    Ok(Box::new(m))
}

struct Omon {
    /// The ticker: `XLU`.
    underlying: Option<String>,
    /// The expiry asked for; the nearest one trading when unset or gone.
    expiry: Option<NaiveDate>,
    /// A contract to point out (OMON with an OCC symbol).
    focus: Option<String>,
    /// Strikes each side of the money, or every strike.
    strikes: Option<usize>,
    greeks: bool,
    picker: SecurityPicker,
    scroll_to_focus: bool,
    /// A spread being built from the chain, for the MLEG ticket.
    spread: Vec<Leg>,
}

/// What the table draws from: every strike, the ones shown, the quotes, the
/// contract list and the stock's price.
struct ChainView<'a> {
    all: &'a [ChainRow],
    shown: std::ops::Range<usize>,
    quotes: Option<&'a mt_alpaca::OptionChain>,
    list: &'a ContractList,
    spot: Option<f64>,
}

/// What a right-click does to the spread being built.
enum SpreadAction {
    /// Add a leg (or turn an existing one round).
    Add(Leg),
    /// A whole strategy: open its ticket.
    Build(Vec<Leg>),
}

/// Strategies built from one strike.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Template {
    BullCall,
    BearCall,
    BullPut,
    BearPut,
    Straddle,
    Strangle,
    IronCondor,
    Butterfly,
}

impl Template {
    fn label(self) -> &'static str {
        match self {
            Self::BullCall => "Bull call spread: buy this call, sell the next strike up",
            Self::BearCall => "Bear call spread: sell this call, buy the next strike up",
            Self::BearPut => "Bear put spread: buy this put, sell the next strike down",
            Self::BullPut => "Bull put spread: sell this put, buy the next strike down",
            Self::Straddle => "Long straddle: buy this strike's call and put",
            Self::Strangle => "Long strangle: buy the put a strike down and the call a strike up",
            Self::IronCondor => {
                "Iron condor around this strike: sell one strike out each side, buy two out"
            }
            Self::Butterfly => "Butterfly: buy a strike down and up, sell two here",
        }
    }

    /// Those offered for a call or a put.
    fn for_right(right: OptionRight) -> [Self; 6] {
        match right {
            OptionRight::Call => [
                Self::BullCall,
                Self::BearCall,
                Self::Butterfly,
                Self::Straddle,
                Self::Strangle,
                Self::IronCondor,
            ],
            OptionRight::Put => [
                Self::BearPut,
                Self::BullPut,
                Self::Butterfly,
                Self::Straddle,
                Self::Strangle,
                Self::IronCondor,
            ],
        }
    }
}

/// The legs of `t` built at row `at` of `rows` (one expiry, lowest strike
/// first), from the clicked side `right`; `None` when a strike or contract
/// it needs is missing.
fn template(t: Template, rows: &[ChainRow], at: usize, right: OptionRight) -> Option<Vec<Leg>> {
    use OptionRight::{Call, Put};
    use OrderSide::{Buy, Sell};
    let leg = |i: isize, r: OptionRight, side: OrderSide, ratio: u32| -> Option<Leg> {
        let row = rows.get(usize::try_from(at as isize + i).ok()?)?;
        Some(Leg::new(row.symbol(r)?, side, ratio))
    };
    let legs = match t {
        Template::BullCall => vec![leg(0, Call, Buy, 1)?, leg(1, Call, Sell, 1)?],
        Template::BearCall => vec![leg(0, Call, Sell, 1)?, leg(1, Call, Buy, 1)?],
        Template::BearPut => vec![leg(0, Put, Buy, 1)?, leg(-1, Put, Sell, 1)?],
        Template::BullPut => vec![leg(0, Put, Sell, 1)?, leg(-1, Put, Buy, 1)?],
        Template::Straddle => vec![leg(0, Call, Buy, 1)?, leg(0, Put, Buy, 1)?],
        Template::Strangle => vec![leg(-1, Put, Buy, 1)?, leg(1, Call, Buy, 1)?],
        Template::IronCondor => vec![
            leg(-2, Put, Buy, 1)?,
            leg(-1, Put, Sell, 1)?,
            leg(1, Call, Sell, 1)?,
            leg(2, Call, Buy, 1)?,
        ],
        Template::Butterfly => vec![
            leg(-1, right, Buy, 1)?,
            leg(0, right, Sell, 2)?,
            leg(1, right, Buy, 1)?,
        ],
    };
    Some(legs)
}

/// Add a leg to a spread: a new contract joins (up to four legs); the same
/// contract on the same side gains a ratio, on the other side it turns round.
fn add_leg(spread: &mut Vec<Leg>, leg: Leg) {
    let full = spread.len() >= options::MAX_LEGS;
    match spread.iter_mut().find(|l| l.symbol == leg.symbol) {
        Some(l) if l.side == leg.side => l.ratio = (l.ratio + 1).min(10),
        Some(l) => l.side = leg.side,
        None if !full => spread.push(leg),
        None => {}
    }
}

/// One side's columns: what each shows for a contract.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Col {
    Bid,
    Ask,
    Last,
    Change,
    Volume,
    OpenInterest,
    Iv,
    Delta,
    Gamma,
    Theta,
    Vega,
}

impl Col {
    fn header(self) -> &'static str {
        match self {
            Self::Bid => "Bid",
            Self::Ask => "Ask",
            Self::Last => "Last",
            Self::Change => "Chg",
            Self::Volume => "Volume",
            Self::OpenInterest => "Open int",
            Self::Iv => "IV",
            Self::Delta => "Delta",
            Self::Gamma => "Gamma",
            Self::Theta => "Theta",
            Self::Vega => "Vega",
        }
    }

    fn width(self) -> f32 {
        match self {
            Self::Volume | Self::OpenInterest => 62.0,
            Self::Gamma | Self::Theta | Self::Vega => 54.0,
            _ => 50.0,
        }
    }
}

impl Omon {
    fn columns(&self) -> Vec<Col> {
        let mut cols = vec![
            Col::Bid,
            Col::Ask,
            Col::Last,
            Col::Change,
            Col::Volume,
            Col::OpenInterest,
            Col::Iv,
            Col::Delta,
        ];
        if self.greeks {
            cols.extend([Col::Gamma, Col::Theta, Col::Vega]);
        }
        cols
    }
}

impl Panel for Omon {
    fn title(&self) -> String {
        match &self.underlying {
            Some(u) => format!("OMON {u} US"),
            None => "OMON".into(),
        }
    }

    fn route(&self) -> Route {
        let mut args = Vec::new();
        if let Some(u) = &self.underlying {
            args.push(format!("{u} US"));
        }
        if let Some(d) = self.expiry {
            args.push(d.to_string());
        }
        match self.strikes {
            Some(NEAR) => {}
            Some(n) => args.push(n.to_string()),
            None => args.push("ALL".into()),
        }
        Route::new("OMON", args)
    }

    fn ui(&mut self, ui: &mut Ui, cx: &mut PanelCx<'_>) {
        let skin = cx.skin;
        ui.horizontal_wrapped(|ui| {
            if let Some(u) = &self.underlying {
                ui.label(
                    RichText::new(format!("{u} US"))
                        .heading()
                        .color(skin.text_strong),
                );
                if let Some(name) = market::name_of(&market::assets(cx), u) {
                    ui.label(RichText::new(name).color(skin.text_muted));
                }
            }
            if let Some(s) = self.picker.show(ui, cx, "omon", "another underlying…") {
                self.underlying = Some(s.ticker);
                self.expiry = None;
                self.focus = None;
            }
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                if cx.alpaca.is_ready() {
                    market::status_label(ui, cx);
                }
            });
        });
        if market::needs_keys(ui, cx) {
            return;
        }
        let Some(underlying) = self.underlying.clone() else {
            ui.add_space(8.0);
            ui.label(
                RichText::new("Pick an underlying above, or type OMON XLU US on the command line.")
                    .color(skin.text_muted),
            );
            return;
        };

        let board = market::board(cx, std::slice::from_ref(&underlying));
        let stock = board.row(&underlying);
        let contracts = cx.hub.watch(&cx.alpaca.option_contracts(&underlying));
        self.quote_line(ui, cx, &underlying, &stock);

        let Some(list) = contracts.data() else {
            widgets::placeholder(ui, skin, contracts.error.as_ref().map(ToString::to_string));
            return;
        };
        let expiries = list.expiries();
        let Some(default) = opt::default_expiry(&expiries, mt_core::time::now_utc()) else {
            ui.add_space(8.0);
            ui.label(
                RichText::new(format!(
                    "Alpaca lists no options on {underlying} expiring in the next three years."
                ))
                .color(skin.text_muted),
            );
            return;
        };
        let expiry = self
            .expiry
            .filter(|d| expiries.contains(d))
            .unwrap_or(default);
        let today = mt_core::exchange::now_exchange().date_naive();

        // Expiries.
        ui.horizontal_wrapped(|ui| {
            widgets::label(ui, skin, "Expiry");
            for d in &expiries {
                let mut text = RichText::new(opt::expiry_label(*d, today));
                if opt::monthly(*d) {
                    text = text.strong();
                }
                if *d == today {
                    text = text.color(skin.warning);
                }
                if ui
                    .selectable_label(*d == expiry, text)
                    .on_hover_text(if opt::monthly(*d) {
                        "Standard monthly expiry (the third Friday)"
                    } else {
                        "Weekly or other expiry"
                    })
                    .clicked()
                {
                    self.expiry = Some(*d);
                }
            }
        });

        let chain = cx.hub.watch(&cx.alpaca.option_chain(&underlying, expiry));
        let quotes = chain.data();
        let listed = list.on(expiry).map(|c| c.symbol.as_str());
        let quoted = quotes
            .into_iter()
            .flat_map(|c| c.by_symbol.keys().map(String::as_str));
        let rows = options::chain_rows(&underlying, expiry, listed.chain(quoted));
        let spot = stock.last;
        let range = match self.strikes {
            Some(n) => options::around(&rows, spot, n),
            None => 0..rows.len(),
        };
        // Keep a contract asked for in view, however far from the money.
        let range = match self.focus.as_deref().and_then(|f| {
            rows.iter()
                .position(|r| r.call.as_deref() == Some(f) || r.put.as_deref() == Some(f))
        }) {
            Some(i) if !range.contains(&i) => range.start.min(i)..range.end.max(i + 1),
            _ => range,
        };
        let shown = &rows[range.clone()];

        // Controls.
        ui.horizontal_wrapped(|ui| {
            widgets::label(ui, skin, "Strikes");
            for (label, value) in [("±10", Some(NEAR)), ("±20", Some(20)), ("All", None)] {
                if ui.selectable_label(self.strikes == value, label).clicked() {
                    self.strikes = value;
                }
            }
            ui.add_space(8.0);
            ui.checkbox(&mut self.greeks, "Gamma, theta, vega");
            ui.add_space(8.0);
            ui.label(
                RichText::new(format!(
                    "{} of {} strikes · {}",
                    shown.len(),
                    rows.len(),
                    mt_alpaca::OPTION_FEED_LABEL
                ))
                .small()
                .color(skin.text_muted),
            );
            let csv_rows = shown.to_vec();
            csv::copy_button(ui, skin, || to_csv(&csv_rows, quotes, list));
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                widgets::freshness(ui, skin, &chain);
            });
        });
        if let Some(e) = &chain.error {
            ui.label(RichText::new(format!("⚠ {e}")).small().color(skin.warning));
        }
        if rows.is_empty() {
            widgets::placeholder(ui, skin, chain.error.as_ref().map(ToString::to_string));
            return;
        }

        if !self.spread.is_empty() {
            self.spread_bar(ui, cx, quotes);
        }
        let view = ChainView {
            all: &rows,
            shown: range.clone(),
            quotes,
            list,
            spot,
        };
        egui::ScrollArea::vertical()
            .id_salt("omon-v")
            .auto_shrink(false)
            .show(ui, |ui| {
                self.table(ui, cx, &view);
                ui.add_space(6.0);
                let mut note =
                    format!("{FEED_NOTE} Refreshed every minute while the market trades.");
                if let Some(d) = list.on(expiry).find_map(|c| c.open_interest_date) {
                    note.push_str(&format!(" Open interest as of {}.", d.format("%b %d")));
                }
                match list.adjusted() {
                    0 => {}
                    1 => note.push_str(
                        " One adjusted contract (after a corporate action) is not shown.",
                    ),
                    n => note.push_str(&format!(
                        " {n} adjusted contracts (after corporate actions) are not shown."
                    )),
                }
                ui.label(RichText::new(note).small().color(skin.text_muted));
            });
    }
}

impl Omon {
    /// The underlying's price and links to its other functions.
    fn quote_line(
        &self,
        ui: &mut Ui,
        cx: &mut PanelCx<'_>,
        underlying: &str,
        stock: &mt_alpaca::board::Row,
    ) {
        let skin = cx.skin;
        let mut open = None;
        ui.horizontal_wrapped(|ui| {
            match stock.last {
                Some(p) => {
                    ui.label(
                        RichText::new(fmt::price(p))
                            .strong()
                            .size(18.0)
                            .color(skin.text_strong),
                    );
                }
                None => {
                    ui.label(RichText::new("No price yet").color(skin.text_muted));
                }
            }
            if let (Some(c), Some(p)) = (stock.change, stock.change_pct) {
                ui.label(
                    RichText::new(format!(
                        "{} ({})",
                        fmt::change_opt(Some(c)),
                        fmt::pct_opt(Some(p))
                    ))
                    .color(skin.delta(c)),
                );
            }
            if let Some(q) = stock.quote.as_ref().filter(|q| q.is_two_sided()) {
                ui.label(
                    RichText::new(format!(
                        "bid {} · ask {}",
                        fmt::price(q.bid),
                        fmt::price(q.ask)
                    ))
                    .color(skin.text),
                );
            }
            ui.label(
                RichText::new(cx.alpaca.feed().label())
                    .small()
                    .color(skin.text_muted),
            );
            ui.add_space(10.0);
            let name = format!("{underlying} US");
            for code in ["GP", "DES", "CN"] {
                if widgets::link(ui, skin, code)
                    .on_hover_text(format!("{code} {name}"))
                    .clicked()
                {
                    open = Some(Route::new(code, [name.clone()]));
                }
            }
        });
        if let Some(r) = open {
            cx.open(r);
        }
    }

    /// The spread being built: its legs, name and net prices, and buttons to
    /// open its ticket or start again.
    fn spread_bar(
        &mut self,
        ui: &mut Ui,
        cx: &mut PanelCx<'_>,
        quotes: Option<&mt_alpaca::OptionChain>,
    ) {
        let skin = cx.skin;
        let mut open = false;
        let mut clear = false;
        egui::Frame::new()
            .stroke(egui::Stroke::new(1.0, skin.accent))
            .inner_margin(egui::Margin::same(6))
            .show(ui, |ui| {
                ui.horizontal_wrapped(|ui| {
                    ui.label(
                        RichText::new(format!("Spread: {}", options::strategy_name(&self.spread)))
                            .strong()
                            .color(skin.text_strong),
                    );
                    let legs: Vec<String> = self.spread.iter().map(Leg::describe).collect();
                    ui.label(RichText::new(legs.join(" / ")).color(skin.text));
                    let exact = |v: Option<f64>| v.and_then(|v| from_f64(v, 2));
                    let net = options::net_price(
                        &self
                            .spread
                            .iter()
                            .map(|l| {
                                let s = quotes.and_then(|q| q.get(&l.symbol));
                                (
                                    l.signed(),
                                    exact(s.and_then(OptionSnapshot::bid)),
                                    exact(s.and_then(OptionSnapshot::ask)),
                                )
                            })
                            .collect::<Vec<_>>(),
                    );
                    if let Some(mid) = net.mid {
                        let mid = mid.round_dp(2);
                        ui.label(
                            RichText::new(if mid.is_sign_negative() {
                                format!("· mid {} credit", mt_core::order::price_text(-mid))
                            } else {
                                format!("· mid {} debit", mt_core::order::price_text(mid))
                            })
                            .color(skin.text_muted),
                        );
                    }
                    if ui
                        .add_enabled(self.spread.len() >= 2, egui::Button::new("Open ticket…"))
                        .on_hover_text(
                            "The MLEG ticket for this spread; nothing is sent until you confirm it",
                        )
                        .clicked()
                    {
                        open = true;
                    }
                    if ui.button("Clear").clicked() {
                        clear = true;
                    }
                });
            });
        if open {
            cx.open(Route::new(
                "MLEG",
                self.spread.iter().map(crate::functions::mleg::leg_arg),
            ));
        }
        if clear {
            self.spread.clear();
        }
    }

    fn table(&mut self, ui: &mut Ui, cx: &mut PanelCx<'_>, view: &ChainView<'_>) {
        let rows = &view.all[view.shown.clone()];
        let (quotes, list, spot) = (view.quotes, view.list, view.spot);
        let skin = cx.skin;
        let cols = self.columns();
        let row_h = 21.0;
        let itm_fill = skin.info.gamma_multiply(0.14);
        // Where the stock trades: after the last strike below it.
        let spot_after = spot.and_then(|s| {
            rows.iter()
                .rposition(|r| mt_core::account::to_f64(r.strike) <= s)
        });
        let focus = self.focus.clone();
        let mut scroll = std::mem::take(&mut self.scroll_to_focus);
        let mut open: Option<Route> = None;
        let mut action: Option<SpreadAction> = None;
        ui.horizontal(|ui| {
            let side_w: f32 = cols.iter().map(|c| c.width() + 6.0).sum();
            ui.add_sized(
                [side_w, 16.0],
                egui::Label::new(RichText::new("CALLS").strong().color(skin.text_strong)),
            );
            ui.add_space(74.0);
            ui.add_sized(
                [side_w, 16.0],
                egui::Label::new(RichText::new("PUTS").strong().color(skin.text_strong)),
            );
        });
        egui::ScrollArea::horizontal()
            .id_salt("omon-h")
            .show(ui, |ui| {
                let mut table = TableBuilder::new(ui)
                    .id_salt("omon-table")
                    .striped(true)
                    .vscroll(false)
                    .cell_layout(egui::Layout::right_to_left(egui::Align::Center));
                for c in &cols {
                    table = table.column(Column::exact(c.width()));
                }
                table = table.column(Column::exact(74.0));
                for c in &cols {
                    table = table.column(Column::exact(c.width()));
                }
                table
                    .header(row_h, |mut h| {
                        for c in cols
                            .iter()
                            .chain([&Col::Bid])
                            .chain(cols.iter())
                            .enumerate()
                        {
                            let (i, col) = c;
                            h.col(|ui| {
                                if i == cols.len() {
                                    ui.with_layout(
                                        egui::Layout::centered_and_justified(
                                            egui::Direction::LeftToRight,
                                        ),
                                        |ui| {
                                            widgets::label(ui, skin, "Strike");
                                        },
                                    );
                                } else {
                                    widgets::label(ui, skin, col.header());
                                }
                            });
                        }
                    })
                    .body(|mut body| {
                        for (i, r) in rows.iter().enumerate() {
                            let focused = focus.is_some() && (r.call == focus || r.put == focus);
                            body.row(row_h, |mut row| {
                                row.set_selected(focused);
                                let strike = mt_core::account::to_f64(r.strike);
                                for right in [OptionRight::Call, OptionRight::Put] {
                                    if right == OptionRight::Put {
                                        row.col(|ui| {
                                            ui.with_layout(
                                                egui::Layout::centered_and_justified(
                                                    egui::Direction::LeftToRight,
                                                ),
                                                |ui| {
                                                    ui.label(
                                                        RichText::new(
                                                            r.strike.normalize().to_string(),
                                                        )
                                                        .strong()
                                                        .color(skin.text_strong),
                                                    );
                                                },
                                            );
                                            if focused && scroll {
                                                ui.scroll_to_cursor(Some(egui::Align::Center));
                                                scroll = false;
                                            }
                                        });
                                    }
                                    let symbol = r.symbol(right);
                                    let snap = symbol.and_then(|s| quotes.and_then(|q| q.get(s)));
                                    let info = symbol.and_then(|s| list.get(s));
                                    let itm = spot.is_some_and(|s| {
                                        options::intrinsic(right, strike, s) > 0.0
                                    });
                                    for c in &cols {
                                        row.col(|ui| {
                                            if itm {
                                                ui.painter().rect_filled(
                                                    ui.max_rect(),
                                                    0.0,
                                                    itm_fill,
                                                );
                                            }
                                            let resp = cell(ui, skin, *c, snap, info);
                                            let Some(sym) = symbol else { return };
                                            if resp.clicked()
                                                && let Some(r) = ticket_at(*c, sym, snap)
                                            {
                                                open = Some(r);
                                            }
                                            resp.context_menu(|ui| {
                                                if let Some(r) = trade_menu(ui, sym) {
                                                    open = Some(r);
                                                }
                                                if let Some(a) = spread_menu(
                                                    ui,
                                                    sym,
                                                    right,
                                                    view.all,
                                                    view.shown.start + i,
                                                ) {
                                                    action = Some(a);
                                                }
                                            });
                                        });
                                    }
                                }
                            });
                            if spot_after == Some(i) && i + 1 < rows.len() {
                                body.row(6.0, |mut row| {
                                    for _ in 0..cols.len() * 2 + 1 {
                                        row.col(|ui| {
                                            let r = ui.max_rect();
                                            ui.painter().hline(
                                                r.x_range(),
                                                r.center().y,
                                                egui::Stroke::new(1.5, skin.accent),
                                            );
                                        });
                                    }
                                });
                            }
                        }
                    });
            });
        if let Some(s) = spot {
            ui.label(
                RichText::new(format!(
                    "The line marks the stock at {}; shaded contracts are in the money. Click a bid \
                     to sell at it or an ask to buy at it, or right-click a contract: a ticket opens, \
                     and nothing is sent until you confirm it.",
                    fmt::price(s)
                ))
                .small()
                .color(skin.text_muted),
            );
        }
        match action {
            Some(SpreadAction::Add(leg)) => add_leg(&mut self.spread, leg),
            Some(SpreadAction::Build(legs)) => {
                open = Some(Route::new(
                    "MLEG",
                    legs.iter().map(crate::functions::mleg::leg_arg),
                ));
                self.spread = legs;
            }
            None => {}
        }
        if let Some(r) = open {
            cx.open(r);
        }
    }
}

/// A contract's spread menu: add it to the spread being built, or build a
/// strategy from its strike.
fn spread_menu(
    ui: &mut Ui,
    symbol: &str,
    right: OptionRight,
    rows: &[ChainRow],
    at: usize,
) -> Option<SpreadAction> {
    let mut out = None;
    ui.separator();
    if ui.button("Add to the spread as a buy").clicked() {
        out = Some(SpreadAction::Add(Leg::new(symbol, OrderSide::Buy, 1)));
    }
    if ui.button("Add to the spread as a sell").clicked() {
        out = Some(SpreadAction::Add(Leg::new(symbol, OrderSide::Sell, 1)));
    }
    ui.menu_button("Spread from this strike", |ui| {
        for t in Template::for_right(right) {
            let legs = template(t, rows, at, right);
            if ui
                .add_enabled(legs.is_some(), egui::Button::new(t.label()))
                .clicked()
                && let Some(legs) = legs
            {
                out = Some(SpreadAction::Build(legs));
            }
        }
    });
    if out.is_some() {
        ui.close();
    }
    out
}

/// A ticket for one contract at the clicked bid (to sell) or ask (to buy).
fn ticket_at(col: Col, symbol: &str, snap: Option<&OptionSnapshot>) -> Option<Route> {
    let (code, price) = match col {
        Col::Bid => ("SELL", snap.and_then(OptionSnapshot::bid)),
        Col::Ask => ("BUY", snap.and_then(OptionSnapshot::ask)),
        _ => return None,
    };
    let mut args = vec![symbol.to_owned(), "1".to_owned()];
    if let Some(p) = price {
        args.extend(["LMT".to_owned(), opt::premium(p)]);
    }
    Some(Route::new(code, args))
}

/// A contract's right-click menu: tickets to buy or sell it, its symbol.
fn trade_menu(ui: &mut Ui, symbol: &str) -> Option<Route> {
    let name = OptionContract::parse_occ(symbol)
        .map_or_else(|| symbol.to_owned(), |c| options::contract_words(&c));
    let mut out = None;
    if ui.button(format!("Buy {name}…")).clicked() {
        out = Some(Route::new("BUY", [symbol]));
    }
    if ui.button(format!("Sell {name}…")).clicked() {
        out = Some(Route::new("SELL", [symbol]));
    }
    if ui.button("Copy the option symbol").clicked() {
        ui.ctx().copy_text(symbol.to_owned());
        ui.close();
    }
    if out.is_some() {
        ui.close();
    }
    out
}

/// One contract's figure for a column.
fn cell(
    ui: &mut Ui,
    skin: &crate::skin::Skin,
    col: Col,
    snap: Option<&OptionSnapshot>,
    info: Option<&options::ContractInfo>,
) -> egui::Response {
    let g = snap.map(|s| s.greeks).unwrap_or_default();
    let (text, color) = match col {
        Col::Bid => (
            opt::premium_opt(snap.and_then(OptionSnapshot::bid)),
            skin.text_strong,
        ),
        Col::Ask => (
            opt::premium_opt(snap.and_then(OptionSnapshot::ask)),
            skin.text_strong,
        ),
        Col::Last => (
            opt::premium_opt(snap.and_then(OptionSnapshot::last)),
            skin.text,
        ),
        Col::Change => {
            let c = snap.and_then(OptionSnapshot::change);
            (
                opt::change_opt(c),
                c.map_or(skin.text_muted, |c| skin.delta(c)),
            )
        }
        Col::Volume => (opt::count(snap.and_then(OptionSnapshot::volume)), skin.text),
        Col::OpenInterest => (
            opt::count(info.and_then(|i| i.open_interest).map(|n| n as f64)),
            skin.text_muted,
        ),
        Col::Iv => (opt::iv(snap.and_then(|s| s.implied_volatility)), skin.text),
        Col::Delta => (opt::greek(g.delta, 2), skin.text),
        Col::Gamma => (opt::greek(g.gamma, 3), skin.text_muted),
        Col::Theta => (opt::greek(g.theta, 3), skin.text_muted),
        Col::Vega => (opt::greek(g.vega, 3), skin.text_muted),
    };
    let text = RichText::new(text).monospace().color(color);
    match col {
        Col::Bid | Col::Ask => ui
            .add(egui::Label::new(text).sense(egui::Sense::click()))
            .on_hover_cursor(egui::CursorIcon::PointingHand)
            .on_hover_text(if col == Col::Bid {
                "Sell one contract at the bid: opens a ticket"
            } else {
                "Buy one contract at the ask: opens a ticket"
            }),
        _ => ui.add(egui::Label::new(text).sense(egui::Sense::click())),
    }
}

fn to_csv(
    rows: &[ChainRow],
    quotes: Option<&mt_alpaca::OptionChain>,
    list: &ContractList,
) -> String {
    let f = |v: Option<f64>| v.map_or_else(String::new, |v| format!("{v}"));
    let mut headers = vec!["strike"];
    for side in ["call", "put"] {
        headers.extend(match side {
            "call" => [
                "call_symbol",
                "call_bid",
                "call_ask",
                "call_last",
                "call_volume",
                "call_open_interest",
                "call_iv",
                "call_delta",
                "call_gamma",
                "call_theta",
                "call_vega",
            ],
            _ => [
                "put_symbol",
                "put_bid",
                "put_ask",
                "put_last",
                "put_volume",
                "put_open_interest",
                "put_iv",
                "put_delta",
                "put_gamma",
                "put_theta",
                "put_vega",
            ],
        });
    }
    csv::to_csv(
        &headers,
        rows.iter().map(|r| {
            let mut out = vec![r.strike.normalize().to_string()];
            for right in [OptionRight::Call, OptionRight::Put] {
                let symbol = r.symbol(right);
                let s = symbol.and_then(|s| quotes.and_then(|q| q.get(s)));
                let g = s.map(|s| s.greeks).unwrap_or_default();
                out.extend([
                    symbol.unwrap_or_default().to_owned(),
                    f(s.and_then(OptionSnapshot::bid)),
                    f(s.and_then(OptionSnapshot::ask)),
                    f(s.and_then(OptionSnapshot::last)),
                    f(s.and_then(OptionSnapshot::volume)),
                    symbol
                        .and_then(|s| list.get(s))
                        .and_then(|i| i.open_interest)
                        .map_or_else(String::new, |n| n.to_string()),
                    f(s.and_then(|s| s.implied_volatility)),
                    f(g.delta),
                    f(g.gamma),
                    f(g.theta),
                    f(g.vega),
                ]);
            }
            out
        }),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn arguments_and_routes() {
        let route = |args: &[&str]| {
            let args: Vec<String> = args.iter().map(|s| (*s).to_owned()).collect();
            open(&args).map(|p| p.route().to_string())
        };
        assert_eq!(route(&["XLU US"]).unwrap(), "OMON XLU US");
        assert_eq!(
            route(&["XLU US", "2026-12-18", "ALL"]).unwrap(),
            "OMON XLU US 2026-12-18 ALL"
        );
        assert_eq!(route(&["XLU US", "20"]).unwrap(), "OMON XLU US 20");
        assert_eq!(
            route(&["XLU261218C00045000"]).unwrap(),
            "OMON XLU US 2026-12-18"
        );
        assert!(route(&["XLU US", "soon"]).is_err());
        assert_eq!(route(&[]).unwrap(), "OMON");
    }

    #[test]
    fn templates_build_strategies_from_a_strike() {
        let day = NaiveDate::from_ymd_opt(2026, 12, 18).unwrap();
        let symbols: Vec<String> = [43, 44, 45, 46, 47]
            .iter()
            .flat_map(|k| ["C", "P"].map(|r| format!("XLU261218{r}{:08}", k * 1000)))
            .collect();
        let rows = options::chain_rows("XLU", day, symbols.iter().map(String::as_str));
        let name = |t: Template, at: usize, right: OptionRight| {
            template(t, &rows, at, right).map(|l| options::strategy_name(&l))
        };
        use OptionRight::{Call, Put};
        assert_eq!(
            name(Template::BullCall, 2, Call).as_deref(),
            Some("Bull call spread")
        );
        assert_eq!(
            name(Template::BearCall, 2, Call).as_deref(),
            Some("Bear call spread")
        );
        assert_eq!(
            name(Template::BearPut, 2, Put).as_deref(),
            Some("Bear put spread")
        );
        assert_eq!(
            name(Template::BullPut, 2, Put).as_deref(),
            Some("Bull put spread")
        );
        assert_eq!(
            name(Template::Straddle, 2, Call).as_deref(),
            Some("Long straddle")
        );
        assert_eq!(
            name(Template::Strangle, 2, Put).as_deref(),
            Some("Long strangle")
        );
        assert_eq!(
            name(Template::IronCondor, 2, Call).as_deref(),
            Some("Iron condor")
        );
        assert_eq!(
            name(Template::Butterfly, 2, Put).as_deref(),
            Some("Long put butterfly")
        );
        // Off the edge of the chain there is no strike to use.
        assert_eq!(name(Template::BullCall, 4, Call), None);
        assert_eq!(name(Template::IronCondor, 1, Call), None);
        // Adding legs by hand.
        let mut spread = Vec::new();
        add_leg(
            &mut spread,
            Leg::new("XLU261218C00045000", OrderSide::Buy, 1),
        );
        add_leg(
            &mut spread,
            Leg::new("XLU261218C00047000", OrderSide::Sell, 1),
        );
        add_leg(
            &mut spread,
            Leg::new("XLU261218C00047000", OrderSide::Sell, 1),
        );
        assert_eq!(spread[1].ratio, 2, "the same leg again adds to its ratio");
        add_leg(
            &mut spread,
            Leg::new("XLU261218C00045000", OrderSide::Sell, 1),
        );
        assert_eq!(
            spread[0].side,
            OrderSide::Sell,
            "the other side turns it round"
        );
    }
}
