//! Alerts: rules evaluated against live data every frame, firing once when a
//! condition becomes true (edge-triggered) and re-arming when it clears.
//!
//! Rules live in `config.toml` under `[[alerts]]`; the ALRT function edits
//! them. The engine itself is pure: give it rules and a view of the data, get
//! back the alerts that just fired.

use std::collections::{HashMap, VecDeque};

use chrono::NaiveDateTime;
use mt_core::BindingConstraints;
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
        }
    }

    pub fn needs_prices(&self) -> bool {
        matches!(self, Self::PriceAbove { .. } | Self::PriceBelow { .. })
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
        }
    }
}

/// What the rules can see.
pub struct AlertData<'a> {
    /// Latest RT price and its interval for a node.
    pub price: &'a dyn Fn(&str) -> Option<(NaiveDateTime, f64)>,
    pub constraints: Option<&'a BindingConstraints>,
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
            engine
                .evaluate(
                    &rules,
                    &AlertData {
                        price: &f,
                        constraints: None,
                    },
                )
                .len()
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
                .evaluate(
                    &rules,
                    &AlertData {
                        price: &none,
                        constraints: None
                    }
                )
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
        let fired = AlertEngine::default().evaluate(
            &rules,
            &AlertData {
                price: &none,
                constraints: Some(&c),
            },
        );
        assert_eq!(fired.len(), 2);
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
