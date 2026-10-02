//! The command line: parsing and completion. Pure functions, unit-tested.
//!
//! Accepted forms (case-insensitive):
//!
//! - `CODE [args...]`               e.g. `GP MINN.HUB 14`
//! - `args... CODE`                 e.g. `MINN.HUB GP` (security first, Bloomberg style)
//! - `NODE`                         a bare pricing node opens `GP NODE`
//! - a trailing `GO` / `<GO>` is ignored

use crate::function::{Registry, Route};

#[derive(Clone, Debug, PartialEq)]
pub enum Parsed {
    Empty,
    Route(Route),
    Unknown(String),
}

pub fn parse(input: &str, registry: &Registry, is_node: impl Fn(&str) -> bool) -> Parsed {
    let mut tokens: Vec<&str> = input.split_whitespace().collect();
    if tokens
        .last()
        .is_some_and(|t| t.eq_ignore_ascii_case("GO") || t.eq_ignore_ascii_case("<GO>"))
    {
        tokens.pop();
    }
    let Some(first) = tokens.first() else {
        return Parsed::Empty;
    };

    if let Some(spec) = registry.find(first) {
        return Parsed::Route(Route::new(spec.code, tokens[1..].iter().copied()));
    }
    if tokens.len() > 1
        && let Some(spec) = tokens.last().and_then(|t| registry.find(t))
    {
        return Parsed::Route(Route::new(
            spec.code,
            tokens[..tokens.len() - 1].iter().copied(),
        ));
    }
    if tokens.len() == 1 && is_node(&first.to_ascii_uppercase()) {
        return Parsed::Route(Route::new("GP", [first.to_ascii_uppercase()]));
    }
    Parsed::Unknown(input.trim().to_owned())
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
    let node_matches = |prefix: &str| {
        let contains: Vec<&String> = nodes.iter().filter(|n| n.contains(prefix)).collect();
        let (mut starts, rest): (Vec<&String>, Vec<&String>) =
            contains.into_iter().partition(|n| n.starts_with(prefix));
        starts.extend(rest);
        starts
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
            for spec in registry.specs() {
                let code_hit = spec.code.starts_with(&upper)
                    || spec.aliases.iter().any(|a| a.starts_with(&upper));
                let name_hit = upper.len() >= 3 && spec.name.to_ascii_uppercase().contains(&upper);
                if code_hit || name_hit {
                    out.push(Suggestion {
                        command: spec.code.to_owned(),
                        detail: spec.name.to_owned(),
                    });
                }
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
