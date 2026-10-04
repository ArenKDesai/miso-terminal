//! The command line: parsing and completion. Pure functions, unit-tested.
//!
//! Accepted forms (case-insensitive):
//!
//! - `CODE [args...]`               e.g. `GP MINN.HUB 14`
//! - `SUBJECT CODE [args...]`       e.g. `MINN.HUB GP 14`, `XLU US GP 30` (subject first, Bloomberg style)
//! - `args... CODE`                 e.g. `MINN.HUB ALTE.ALTE CMP`
//! - `NODE`                         a bare pricing node opens `GP NODE`
//! - a trailing `GO` / `<GO>` is ignored
//!
//! A security is a ticker plus its market code, `XLU US` (Bloomberg's trailing
//! `Equity` is accepted), so it is never confused with a node such as `AECI`.
//! Its tokens become one argument: `XLU US GP 30` is `GP` with `XLU US` and `30`.
//! OCC option symbols (`XLU261218C00082500`) are single tokens already.

use mt_core::instrument::{self, Instrument};

use crate::function::{Registry, Route};

#[derive(Clone, Debug, PartialEq)]
pub enum Parsed {
    Empty,
    Route(Route),
    /// A security or option on its own, with no function.
    Instrument(Instrument),
    Unknown(String),
}

/// Join each security's tokens into one argument, written the standard way
/// (`xlu us equity` -> `XLU US`).
fn group_instruments(tokens: &[&str]) -> Vec<String> {
    let mut out = Vec::with_capacity(tokens.len());
    let mut i = 0;
    while i < tokens.len() {
        match instrument::parse_tokens(&tokens[i..]) {
            Some((inst, used)) => {
                out.push(inst.to_string());
                i += used;
            }
            None => {
                out.push(tokens[i].to_owned());
                i += 1;
            }
        }
    }
    out
}

pub fn parse(input: &str, registry: &Registry, is_node: impl Fn(&str) -> bool) -> Parsed {
    let mut raw: Vec<&str> = input.split_whitespace().collect();
    if raw
        .last()
        .is_some_and(|t| t.eq_ignore_ascii_case("GO") || t.eq_ignore_ascii_case("<GO>"))
    {
        raw.pop();
    }
    let tokens = group_instruments(&raw);
    let Some(first) = tokens.first() else {
        return Parsed::Empty;
    };

    if let Some(spec) = registry.find(first) {
        return Parsed::Route(Route::new(spec.code, &tokens[1..]));
    }
    if let Some(spec) = tokens.get(1).and_then(|t| registry.find(t)) {
        let args = std::iter::once(first).chain(&tokens[2..]);
        return Parsed::Route(Route::new(spec.code, args));
    }
    if tokens.len() > 1
        && let Some(spec) = tokens.last().and_then(|t| registry.find(t))
    {
        return Parsed::Route(Route::new(spec.code, &tokens[..tokens.len() - 1]));
    }
    if tokens.len() == 1 {
        if let Some(inst) = Instrument::parse_security(first) {
            return Parsed::Instrument(inst);
        }
        if is_node(&first.to_ascii_uppercase()) {
            return Parsed::Route(Route::new("GP", [first.to_ascii_uppercase()]));
        }
    }
    Parsed::Unknown(input.trim().to_owned())
}

/// The first security or option among a route's arguments, if any.
pub fn security_arg(route: &Route) -> Option<Instrument> {
    route
        .args
        .iter()
        .find_map(|a| Instrument::parse_security(a))
}

/// Usage and description of the function being typed, once its code is
/// recognised: `GP <node> [days] [HEAT] — Price chart for one node…`.
pub fn hint(input: &str, registry: &Registry) -> Option<(&'static str, &'static str)> {
    let first = input.split_whitespace().next()?;
    let spec = registry.find(first)?;
    Some((spec.usage, spec.description))
}

/// Whether every character of `needle` appears in `hay` in order.
fn is_subsequence(needle: &str, hay: &str) -> bool {
    let mut hay = hay.chars();
    needle.chars().all(|c| hay.any(|h| h == c))
}

#[derive(Clone, Debug, PartialEq)]
pub struct Suggestion {
    /// Full command-line text to run when chosen.
    pub command: String,
    pub detail: String,
}

/// Completions for the current input. `nodes` should list favourites first.
pub fn suggest(
    input: &str,
    registry: &Registry,
    nodes: &[String],
    limit: usize,
) -> Vec<Suggestion> {
    let upper = input.trim_start().to_ascii_uppercase();
    let mut out = Vec::new();
    // Prefix matches first, then substring, then (for 3+ characters) fuzzy
    // subsequence matches such as `michub` -> `MICHIGAN.HUB`.
    let node_matches = |needle: &str| {
        let mut ranked: Vec<(u8, &String)> = nodes
            .iter()
            .filter_map(|n| {
                let rank = if n.starts_with(needle) {
                    0
                } else if n.contains(needle) {
                    1
                } else if needle.len() >= 3 && is_subsequence(needle, n) {
                    2
                } else {
                    return None;
                };
                Some((rank, n))
            })
            .collect();
        ranked.sort_by_key(|(rank, _)| *rank);
        ranked.into_iter().map(|(_, n)| n).collect::<Vec<_>>()
    };

    match upper.split_once(' ') {
        // Completing a function's argument.
        Some((code, rest)) => {
            let Some(spec) = registry.find(code) else {
                return out;
            };
            if spec.takes_node && !rest.contains(' ') {
                for n in node_matches(rest.trim()).into_iter().take(limit) {
                    out.push(Suggestion {
                        command: format!("{} {n}", spec.code),
                        detail: spec.name.to_owned(),
                    });
                }
            }
        }
        // Completing a function code, or a bare node.
        None => {
            if upper.is_empty() {
                return out;
            }
            let mut fuzzy = Vec::new();
            for spec in registry.specs() {
                let code_hit = spec.code.starts_with(&upper)
                    || spec.aliases.iter().any(|a| a.starts_with(&upper));
                let name_hit = upper.len() >= 3 && spec.name.to_ascii_uppercase().contains(&upper);
                let suggestion = Suggestion {
                    command: spec.code.to_owned(),
                    detail: spec.name.to_owned(),
                };
                if code_hit || name_hit {
                    out.push(suggestion);
                } else if upper.len() >= 2 && is_subsequence(&upper, spec.code) {
                    fuzzy.push(suggestion);
                }
            }
            // Fuzzy code matches (`SPD` -> `SPRD`) only when nothing matched exactly.
            if out.is_empty() {
                out.extend(fuzzy);
            }
            if upper.len() >= 2 {
                for n in node_matches(&upper) {
                    if out.len() >= limit {
                        break;
                    }
                    out.push(Suggestion {
                        command: format!("GP {n}"),
                        detail: "Graph price".into(),
                    });
                }
            }
        }
    }
    out.truncate(limit);
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn reg() -> Registry {
        Registry::builtin()
    }

    fn node(s: &str) -> bool {
        s.ends_with(".HUB") || s == "ALTE.ALTE"
    }

    #[test]
    fn parses_all_forms() {
        let r = reg();
        assert_eq!(
            parse("gp minn.hub 14", &r, node),
            Parsed::Route(Route::new("GP", ["minn.hub", "14"]))
        );
        assert_eq!(
            parse("MINN.HUB GP <GO>", &r, node),
            Parsed::Route(Route::new("GP", ["MINN.HUB"]))
        );
        assert_eq!(
            parse("MINN.HUB GP 14", &r, node),
            Parsed::Route(Route::new("GP", ["MINN.HUB", "14"]))
        );
        assert_eq!(
            parse("MINN.HUB ALTE.ALTE CMP", &r, node),
            Parsed::Route(Route::new("CMP", ["MINN.HUB", "ALTE.ALTE"]))
        );
        assert_eq!(
            parse("alte.alte", &r, node),
            Parsed::Route(Route::new("GP", ["ALTE.ALTE"]))
        );
        assert_eq!(parse("lmp", &r, node), Parsed::Route(Route::code("LMP")));
        assert_eq!(parse("   ", &r, node), Parsed::Empty);
        assert_eq!(
            parse("make coffee", &r, node),
            Parsed::Unknown("make coffee".into())
        );
    }

    #[test]
    fn securities_are_one_argument_and_never_nodes() {
        let r = reg();
        // AECI is a node; AECI US would be a security.
        let node = |s: &str| s == "AECI" || s.ends_with(".HUB");
        assert_eq!(
            parse("aeci", &r, node),
            Parsed::Route(Route::new("GP", ["AECI"]))
        );
        assert_eq!(
            parse("xlu us gp 30", &r, node),
            Parsed::Route(Route::new("GP", ["XLU US", "30"]))
        );
        assert_eq!(
            parse("GP XLU US Equity 30 <GO>", &r, node),
            Parsed::Route(Route::new("GP", ["XLU US", "30"]))
        );
        assert_eq!(
            parse("CMP MINN.HUB XEL US", &r, node),
            Parsed::Route(Route::new("CMP", ["MINN.HUB", "XEL US"]))
        );
        let Parsed::Instrument(inst) = parse("xlu us", &r, node) else {
            panic!("a bare security");
        };
        assert_eq!(inst.to_string(), "XLU US");
        let Parsed::Route(route) = parse("XLU261218C00082500 GP", &r, node) else {
            panic!()
        };
        assert_eq!(route.args, ["XLU261218C00082500"]);
        assert!(matches!(security_arg(&route), Some(Instrument::Option(_))));
        // A route's text parses back to the same route (saved layouts, --run).
        let route = Route::new("GP", ["XLU US", "30"]);
        assert_eq!(parse(&route.to_string(), &r, node), Parsed::Route(route));
        assert_eq!(security_arg(&Route::new("GP", ["MINN.HUB"])), None);
    }

    #[test]
    fn aliases_normalise_to_the_main_code() {
        let r = reg();
        let spec = r
            .specs()
            .iter()
            .find(|s| !s.aliases.is_empty())
            .expect("some function has an alias");
        let Parsed::Route(route) = parse(spec.aliases[0], &r, node) else {
            panic!()
        };
        assert_eq!(route.code, spec.code);
    }

    #[test]
    fn suggests_codes_nodes_and_arguments() {
        let r = reg();
        let nodes: Vec<String> = ["MINN.HUB", "MICHIGAN.HUB", "ALTE.ALTE"]
            .map(String::from)
            .to_vec();
        let s = suggest("l", &r, &nodes, 10);
        assert!(s.iter().any(|s| s.command == "LMP"));
        let s = suggest("mi", &r, &nodes, 10);
        assert!(s.iter().any(|s| s.command == "GP MINN.HUB"));
        let s = suggest("gp al", &r, &nodes, 10);
        assert_eq!(s[0].command, "GP ALTE.ALTE");
        assert!(suggest("", &r, &nodes, 10).is_empty());
        assert!(suggest("gp", &r, &nodes, 3).len() <= 3);
    }

    #[test]
    fn fuzzy_matches_rank_after_exact_ones() {
        let r = reg();
        let nodes: Vec<String> = ["MICHIGAN.HUB", "MINN.HUB", "AMIL.MICH1"]
            .map(String::from)
            .to_vec();
        let s = suggest("michub", &r, &nodes, 10);
        assert_eq!(s[0].command, "GP MICHIGAN.HUB");
        let s = suggest("spd", &r, &nodes, 10);
        assert_eq!(s[0].command, "SPRD", "fuzzy code match");
        // An exact prefix match wins over fuzzy ones.
        assert_eq!(suggest("gp", &r, &nodes, 10)[0].command, "GP");
        assert!(is_subsequence("MHB", "MICHIGAN.HUB") && !is_subsequence("BHM", "MICHIGAN.HUB"));
    }

    #[test]
    fn hints_follow_the_typed_code() {
        let r = reg();
        let gp = r.find("GP").unwrap().usage;
        assert_eq!(hint("gp min", &r).map(|h| h.0), Some(gp));
        assert_eq!(hint("graph", &r).map(|h| h.0), Some(gp), "aliases too");
        assert!(hint("", &r).is_none());
        assert!(hint("nonsense", &r).is_none());
    }

    #[test]
    fn every_function_opens_with_no_arguments() {
        // Functions must cope with a bare code (they prompt for missing args).
        let r = reg();
        for spec in r.specs() {
            assert!(
                (spec.open)(&[]).is_ok(),
                "{} failed to open without args",
                spec.code
            );
        }
    }
}
