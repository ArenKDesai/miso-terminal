//! System notifications for alerts. The UI only knows this trait; the binary
//! supplies the platform implementation (Windows toast notifications).

/// Shows a notification outside the window.
pub trait Notifier: Send + Sync {
    /// Show one notification. Must return quickly (hand slow work to a thread).
    fn notify(&self, title: &str, body: &str);
}

/// The notifications to show for alerts that fired together: one each for a
/// few, one summary for a burst (so a market spike is not a wall of toasts).
pub fn for_alerts(fired: &[crate::alerts::AlertEvent]) -> Vec<(String, String)> {
    const MAX_SEPARATE: usize = 3;
    if fired.len() <= MAX_SEPARATE {
        return fired
            .iter()
            .map(|e| (e.rule.clone(), e.detail.clone()))
            .collect();
    }
    let mut body: Vec<String> = fired
        .iter()
        .take(MAX_SEPARATE)
        .map(|e| e.rule.clone())
        .collect();
    body.push(format!(
        "and {} more (see ALRT)",
        fired.len() - MAX_SEPARATE
    ));
    vec![(format!("{} MISO alerts", fired.len()), body.join("\n"))]
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::alerts::AlertEvent;

    fn event(rule: &str) -> AlertEvent {
        AlertEvent {
            at: chrono::Local::now(),
            rule: rule.into(),
            detail: format!("{rule} detail"),
        }
    }

    #[test]
    fn a_burst_becomes_one_summary() {
        let few: Vec<_> = ["a", "b"].map(event).to_vec();
        assert_eq!(
            for_alerts(&few),
            vec![
                ("a".into(), "a detail".into()),
                ("b".into(), "b detail".into())
            ]
        );
        let many: Vec<_> = ["a", "b", "c", "d", "e"].map(event).to_vec();
        let out = for_alerts(&many);
        assert_eq!(out.len(), 1);
        assert_eq!(out[0].0, "5 MISO alerts");
        assert!(out[0].1.ends_with("and 2 more (see ALRT)"));
    }
}
