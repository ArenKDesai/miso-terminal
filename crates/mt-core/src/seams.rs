//! The seams: MISO's interface pricing nodes with its neighbours, and PJM's
//! forecast at the MISO-PJM interface used for coordinated transaction
//! scheduling (CTS).

use chrono::NaiveDateTime;

/// MISO's pricing node for the PJM interface.
pub const PJM_INTERFACE: &str = "PJMC";

/// Who is on the other side of a MISO interface pricing node, where it is
/// unambiguous. Other interface nodes are shown by name only.
pub fn interface_neighbour(node: &str) -> Option<&'static str> {
    Some(match node {
        "PJMC" => "PJM",
        "SWPP" => "SPP",
        "TVA" => "TVA",
        "SOCO" => "Southern Company",
        "ONT" => "Ontario (IESO)",
        "NYISO" => "NYISO",
        "AECI" => "Associated Electric",
        "LGEE" => "LG&E and KU",
        "DUK" => "Duke Energy",
        "CPLE" => "Duke Energy Progress East",
        "CPLW" => "Duke Energy Progress West",
        "EDE" => "Empire District",
        "KCPL" => "Evergy (KCP&L)",
        "NPPD" => "Nebraska Public Power",
        "OPPD" => "Omaha Public Power",
        "SPA" => "Southwestern Power Admin.",
        "SPC" => "SaskPower",
        _ => return None,
    })
}

/// One of PJM's forecast LMPs at the interface.
#[derive(Clone, Debug, PartialEq)]
pub struct CtsForecast {
    /// When the forecast case was approved.
    pub case_time: NaiveDateTime,
    /// The 15-minute interval forecast (MISO's `SOLUTIONTIME`).
    pub time: NaiveDateTime,
    /// PJM's forecast LMP at the MISO interface, $/MWh.
    pub lmp: f64,
}

/// Today's CTS forecast cases: a new case about every 15 minutes, each
/// forecasting the next couple of hours.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Cts {
    pub forecasts: Vec<CtsForecast>,
}

impl Cts {
    pub fn latest_case_time(&self) -> Option<NaiveDateTime> {
        self.forecasts.iter().map(|f| f.case_time).max()
    }

    /// The most recent case's forecast, by interval.
    pub fn latest_case(&self) -> Vec<(NaiveDateTime, f64)> {
        let Some(latest) = self.latest_case_time() else {
            return Vec::new();
        };
        let mut out: Vec<_> = self
            .forecasts
            .iter()
            .filter(|f| f.case_time == latest)
            .map(|f| (f.time, f.lmp))
            .collect();
        out.sort_by_key(|p| p.0);
        out
    }

    /// For every interval, the most recent forecast of it: shortest lead time
    /// through the day so far, then the latest case into the future.
    pub fn freshest(&self) -> Vec<(NaiveDateTime, f64)> {
        let mut best: std::collections::BTreeMap<NaiveDateTime, (NaiveDateTime, f64)> =
            std::collections::BTreeMap::new();
        for f in &self.forecasts {
            let slot = best.entry(f.time).or_insert((f.case_time, f.lmp));
            if f.case_time > slot.0 {
                *slot = (f.case_time, f.lmp);
            }
        }
        best.into_iter().map(|(t, (_, v))| (t, v)).collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn t(h: u32, m: u32) -> NaiveDateTime {
        chrono::NaiveDate::from_ymd_opt(2026, 10, 2)
            .unwrap()
            .and_hms_opt(h, m, 0)
            .unwrap()
    }

    #[test]
    fn freshest_prefers_the_latest_case() {
        let f = |case: NaiveDateTime, time, lmp| CtsForecast {
            case_time: case,
            time,
            lmp,
        };
        let cts = Cts {
            forecasts: vec![
                f(t(10, 5), t(10, 0), 30.0),
                f(t(10, 5), t(10, 15), 31.0),
                f(t(10, 20), t(10, 15), 35.0),
                f(t(10, 20), t(10, 30), 36.0),
            ],
        };
        assert_eq!(cts.latest_case_time(), Some(t(10, 20)));
        assert_eq!(
            cts.latest_case(),
            vec![(t(10, 15), 35.0), (t(10, 30), 36.0)]
        );
        assert_eq!(
            cts.freshest(),
            vec![(t(10, 0), 30.0), (t(10, 15), 35.0), (t(10, 30), 36.0)]
        );
        assert_eq!(interface_neighbour("PJMC"), Some("PJM"));
        assert_eq!(interface_neighbour("MPW.MPW"), None);
    }
}
