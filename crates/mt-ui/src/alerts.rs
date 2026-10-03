//! Alerts: rules evaluated against live data every frame, firing once when a
//! condition becomes true (edge-triggered) and re-arming when it clears.
//!
//! Rules live in `config.toml` under `[[alerts]]`; the ALRT function edits
//! them. The engine itself is pure: give it rules and a view of the data, get
//! back the alerts that just fired.

use std::collections::{HashMap, VecDeque};

use chrono::NaiveDateTime;
use chrono::Timelike;
use mt_core::{Ace, BindingConstraints, RegionalTransfer, SystemLoad};
use serde::{Deserialize, Serialize};

use crate::widgets::fmt;

/// Keep this many fired alerts for the ALRT history.
const HISTORY: usize = 200;

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum AlertRule {
    /// RT five-minute LMP at `node` at or above `value` ($/MWh).
    PriceAbove { node: String, value: f64 },
    /// RT five-minute LMP at `node` at or below `value` ($/MWh).
    PriceBelow { node: String, value: f64 },
    /// A binding constraint whose name contains `contains` (case-insensitive).
    ConstraintBinds { contains: String },
    /// Any binding constraint with |shadow price| at or above `value` ($/MWh).
    ShadowPriceAbove { value: f64 },
    /// RT price at `a` minus RT price at `b` at or above `value` ($/MWh).
    SpreadAbove { a: String, b: String, value: f64 },
    /// North-South regional transfer at or above `pct`% of its limit.
    TransferAbove { pct: f64 },
    /// Actual load at least `pct`% above the medium-term forecast for the hour.
    LoadAboveForecast { pct: f64 },
    /// |Area control error| at or above `mw`.
    AceAbove { mw: f64 },
}

/// Which feeds a rule reads, so the shell only watches what is needed.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Feeds {
    pub prices: bool,
    pub constraints: bool,
    pub transfer: bool,
    pub load: bool,
    pub ace: bool,
}

impl Feeds {
    pub fn union(self, o: Self) -> Self {
        Self {
            prices: self.prices || o.prices,
            constraints: self.constraints || o.constraints,
            transfer: self.transfer || o.transfer,
            load: self.load || o.load,
            ace: self.ace || o.ace,
        }
    }
}

impl AlertRule {
    /// Human description, also the rule's identity for edge tracking.
    pub fn describe(&self) -> String {
        match self {
            Self::PriceAbove { node, value } => format!("{node} RT ≥ {}", fmt::price(*value)),
            Self::PriceBelow { node, value } => format!("{node} RT ≤ {}", fmt::price(*value)),
            Self::ConstraintBinds { contains } => format!("constraint binds: “{contains}”"),
            Self::ShadowPriceAbove { value } => {
                format!("any |shadow price| ≥ {}", fmt::price(*value))
            }
            Self::SpreadAbove { a, b, value } => format!("{a} − {b} RT ≥ {}", fmt::signed(*value)),
            Self::TransferAbove { pct } => format!("N–S transfer ≥ {pct:.0}% of limit"),
            Self::LoadAboveForecast { pct } => format!("load ≥ {pct:.1}% above forecast"),
            Self::AceAbove { mw } => format!("|ACE| ≥ {} MW", fmt::mw(*mw)),
        }
    }

    pub fn feeds(&self) -> Feeds {
        match self {
            Self::PriceAbove { .. } | Self::PriceBelow { .. } | Self::SpreadAbove { .. } => Feeds {
                prices: true,
                ..Feeds::default()
            },
            Self::ConstraintBinds { .. } | Self::ShadowPriceAbove { .. } => Feeds {
                constraints: true,
                ..Feeds::default()
            },
            Self::TransferAbove { .. } => Feeds {
                transfer: true,
                ..Feeds::default()
            },
            Self::LoadAboveForecast { .. } => Feeds {
                load: true,
                ..Feeds::default()
            },
            Self::AceAbove { .. } => Feeds {
                ace: true,
                ..Feeds::default()
            },
        }
    }

    /// Pricing nodes the rule reads.
    pub fn nodes(&self) -> Vec<&str> {
        match self {
            Self::PriceAbove { node, .. } | Self::PriceBelow { node, .. } => vec![node],
            Self::SpreadAbove { a, b, .. } => vec![a, b],
            _ => Vec::new(),
        }
    }

    /// Whether the condition holds now, with a detail line; `None` when the
    /// data it needs has not arrived.
    fn check(&self, data: &AlertData<'_>) -> Option<(bool, String)> {
        match self {
            Self::PriceAbove { node, value } | Self::PriceBelow { node, value } => {
                let (at, price) = (data.price)(node)?;
                let hit = if matches!(self, Self::PriceAbove { .. }) {
                    price >= *value
                } else {
                    price <= *value
                };
                Some((
                    hit,
                    format!("{node} at {} EST: {}", fmt::hm(at), fmt::price(price)),
                ))
            }
            Self::ConstraintBinds { contains } => {
                let needle = contains.to_ascii_uppercase();
                let c = data.constraints?;
                let found = c
                    .constraints
                    .iter()
                    .find(|k| k.name.to_ascii_uppercase().contains(&needle));
                Some(match found {
                    Some(k) => (
                        true,
                        format!(
                            "{} binding, shadow {}",
                            k.name,
                            fmt::price_opt(k.shadow_price)
                        ),
                    ),
                    None => (false, String::new()),
                })
            }
            Self::ShadowPriceAbove { value } => {
                let c = data.constraints?;
                let worst = c
                    .constraints
                    .iter()
                    .filter_map(|k| Some((k, k.shadow_price?.abs())))
                    .max_by(|a, b| a.1.total_cmp(&b.1));
                Some(match worst {
                    Some((k, s)) if s >= *value => (
                        true,
                        format!("{} shadow {}", k.name, fmt::price_opt(k.shadow_price)),
                    ),
                    _ => (false, String::new()),
                })
            }
            Self::SpreadAbove { a, b, value } => {
                let ((at, pa), (_, pb)) = ((data.price)(a)?, (data.price)(b)?);
                let spread = pa - pb;
                Some((
                    spread >= *value,
                    format!("{a} − {b} at {} EST: {}", fmt::hm(at), fmt::signed(spread)),
                ))
            }
            Self::TransferAbove { pct } => {
                let p = data
                    .transfer?
                    .points
                    .iter()
                    .rev()
                    .find(|p| p.flow.is_some())?;
                let used = p.utilization()? * 100.0;
                let flow = p.flow.unwrap_or_default();
                Some((
                    used >= *pct,
                    format!(
                        "{} MW at {} EST = {used:.0}% of limit",
                        fmt::mw_signed(flow),
                        fmt::hm(p.time)
                    ),
                ))
            }
            Self::LoadAboveForecast { pct } => {
                let load = data.load?;
                let (at, actual) = load.latest()?;
                // Five-minute stamps are interval-beginning; HE = hour + 1.
                let he = u8::try_from(at.hour() + 1).ok()?;
                let forecast = load.forecast.iter().find(|(h, _)| *h == he)?.1;
                let miss = (actual - forecast) / forecast * 100.0;
                Some((
                    miss >= *pct,
                    format!(
                        "{} MW vs {} forecast HE{he} ({miss:+.1}%)",
                        fmt::mw(actual),
                        fmt::mw(forecast)
                    ),
                ))
            }
            Self::AceAbove { mw } => {
                let &(at, ace) = data.ace?.points.last()?;
                Some((
                    ace.abs() >= *mw,
                    format!(
                        "ACE {} MW at {}",
                        fmt::mw_signed(ace),
                        at.format("%H:%M:%S")
                    ),
                ))
            }
        }
    }
}

/// What the rules can see. Feeds a rule needs but that have not loaded are
/// `None`, and that rule is skipped until they arrive.
pub struct AlertData<'a> {
    /// Latest RT price and its interval for a node.
    pub price: &'a dyn Fn(&str) -> Option<(NaiveDateTime, f64)>,
    pub constraints: Option<&'a BindingConstraints>,
    pub transfer: Option<&'a RegionalTransfer>,
    pub load: Option<&'a SystemLoad>,
    pub ace: Option<&'a Ace>,
}

impl<'a> AlertData<'a> {
    /// Only prices (and constraints), for callers that have nothing else.
    pub fn prices(
        price: &'a dyn Fn(&str) -> Option<(NaiveDateTime, f64)>,
        constraints: Option<&'a BindingConstraints>,
    ) -> Self {
        Self {
            price,
            constraints,
            transfer: None,
            load: None,
            ace: None,
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct AlertEvent {
    /// Local wall-clock time it fired.
    pub at: chrono::DateTime<chrono::Local>,
    pub rule: String,
    pub detail: String,
}

/// Rule state per rule description: `Some(true)` while the condition holds.
#[derive(Default)]
pub struct AlertEngine {
    state: HashMap<String, bool>,
    pub history: VecDeque<AlertEvent>,
    /// Fired since the ALRT function was last looked at.
    pub unseen: usize,
}

impl AlertEngine {
    /// Evaluate every rule; returns the alerts that fired this time.
    pub fn evaluate(&mut self, rules: &[AlertRule], data: &AlertData<'_>) -> Vec<AlertEvent> {
        let mut fired = Vec::new();
        for rule in rules {
            let key = rule.describe();
            let Some((hit, detail)) = rule.check(data) else {
                continue;
            };
            let was = self.state.insert(key.clone(), hit).unwrap_or(false);
            if hit && !was {
                fired.push(AlertEvent {
                    at: chrono::Local::now(),
                    rule: key,
                    detail,
                });
            }
        }
        // Forget state for rules that no longer exist.
        let live: Vec<String> = rules.iter().map(AlertRule::describe).collect();
        self.state.retain(|k, _| live.contains(k));
        for e in &fired {
            if self.history.len() == HISTORY {
                self.history.pop_back();
            }
            self.history.push_front(e.clone());
        }
        self.unseen += fired.len();
        fired
    }

    /// Whether a rule's condition currently holds (`None` = not evaluated yet).
    pub fn is_active(&self, rule: &AlertRule) -> Option<bool> {
        self.state.get(&rule.describe()).copied()
    }
}

#[cfg(test)]
mod tests {
    use mt_core::BindingConstraint;

    use super::*;

    fn t() -> NaiveDateTime {
        chrono::NaiveDate::from_ymd_opt(2026, 10, 2)
            .unwrap()
            .and_hms_opt(16, 30, 0)
            .unwrap()
    }

    #[test]
    fn price_alerts_fire_once_and_rearm() {
        let rules = vec![AlertRule::PriceAbove {
            node: "MINN.HUB".into(),
            value: 100.0,
        }];
        let mut engine = AlertEngine::default();
        let at = |p: f64| move |_: &str| Some((t(), p));
        let run = |engine: &mut AlertEngine, p: f64| {
            let f = at(p);
            engine.evaluate(&rules, &AlertData::prices(&f, None)).len()
        };
        assert_eq!(run(&mut engine, 50.0), 0);
        assert_eq!(run(&mut engine, 120.0), 1, "crossing fires");
        assert_eq!(run(&mut engine, 130.0), 0, "still above: no repeat");
        assert_eq!(run(&mut engine, 90.0), 0, "clears");
        assert_eq!(run(&mut engine, 101.0), 1, "re-armed");
        assert_eq!(engine.unseen, 2);
        assert_eq!(engine.history.len(), 2);
        assert_eq!(engine.is_active(&rules[0]), Some(true));
    }

    #[test]
    fn missing_data_neither_fires_nor_clears() {
        let rules = vec![AlertRule::PriceBelow {
            node: "X".into(),
            value: 0.0,
        }];
        let mut engine = AlertEngine::default();
        let none = |_: &str| None;
        assert!(
            engine
                .evaluate(&rules, &AlertData::prices(&none, None))
                .is_empty()
        );
        assert_eq!(engine.is_active(&rules[0]), None);
    }

    #[test]
    fn constraint_alerts() {
        let c = BindingConstraints {
            interval: None,
            constraints: vec![BindingConstraint {
                name: "TMP141_SIOUX_CITY_XFMR".into(),
                shadow_price: Some(-531.7),
                ..Default::default()
            }],
        };
        let rules = vec![
            AlertRule::ConstraintBinds {
                contains: "sioux".into(),
            },
            AlertRule::ShadowPriceAbove { value: 500.0 },
            AlertRule::ShadowPriceAbove { value: 1000.0 },
        ];
        let none = |_: &str| None;
        let fired = AlertEngine::default().evaluate(&rules, &AlertData::prices(&none, Some(&c)));
        assert_eq!(fired.len(), 2);
    }

    #[test]
    fn spread_transfer_load_and_ace_alerts() {
        use mt_core::TransferPoint;
        let price = |n: &str| Some((t(), if n == "A" { 50.0 } else { 30.0 }));
        let transfer = RegionalTransfer {
            points: vec![TransferPoint {
                time: t(),
                flow: Some(2_300.0),
                raw: None,
                north_south_limit: Some(-3_000.0),
                south_north_limit: Some(2_500.0),
            }],
        };
        // 16:30 EST is HE17; actual 3% over forecast.
        let load = SystemLoad {
            actual_5min: vec![(t(), 103_000.0)],
            forecast: vec![(17, 100_000.0)],
            ..Default::default()
        };
        let ace = Ace {
            points: vec![(t(), -1_200.0)],
        };
        let data = AlertData {
            price: &price,
            constraints: None,
            transfer: Some(&transfer),
            load: Some(&load),
            ace: Some(&ace),
        };
        let rules = vec![
            AlertRule::SpreadAbove {
                a: "A".into(),
                b: "B".into(),
                value: 15.0,
            },
            AlertRule::SpreadAbove {
                a: "A".into(),
                b: "B".into(),
                value: 25.0,
            },
            AlertRule::TransferAbove { pct: 90.0 },
            AlertRule::TransferAbove { pct: 95.0 },
            AlertRule::LoadAboveForecast { pct: 2.5 },
            AlertRule::LoadAboveForecast { pct: 5.0 },
            AlertRule::AceAbove { mw: 1_000.0 },
            AlertRule::AceAbove { mw: 1_500.0 },
        ];
        let fired: Vec<String> = AlertEngine::default()
            .evaluate(&rules, &data)
            .into_iter()
            .map(|e| e.rule)
            .collect();
        assert_eq!(
            fired,
            vec![
                rules[0].describe(),
                rules[2].describe(),
                rules[4].describe(),
                rules[6].describe()
            ]
        );
        let feeds = rules
            .iter()
            .fold(Feeds::default(), |f, r| f.union(r.feeds()));
        assert!(feeds.prices && feeds.transfer && feeds.load && feeds.ace && !feeds.constraints);
        assert_eq!(rules[0].nodes(), vec!["A", "B"]);
    }

    #[test]
    fn rules_round_trip_through_toml() {
        #[derive(Serialize, Deserialize, PartialEq, Debug)]
        struct Wrap {
            alerts: Vec<AlertRule>,
        }
        let w = Wrap {
            alerts: vec![
                AlertRule::PriceAbove {
                    node: "MINN.HUB".into(),
                    value: 100.0,
                },
                AlertRule::ConstraintBinds {
                    contains: "SIOUX".into(),
                },
            ],
        };
        let s = toml::to_string(&w).unwrap();
        assert!(s.contains("kind = \"price_above\""), "{s}");
        assert_eq!(toml::from_str::<Wrap>(&s).unwrap(), w);
    }
}
