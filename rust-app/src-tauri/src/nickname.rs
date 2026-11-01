use crate::automation::clean_roster_name;

pub(crate) const HUD_SAMPLE_LIMIT: u8 = 1;

/// Take the single HUD reading directly, keeping all three slots and unknowns.
pub(crate) fn roster(readings: &[String]) -> (Vec<String>, Option<usize>) {
    let names: Vec<_> = (0..3)
        .map(|slot| {
            readings
                .get(slot)
                .map_or_else(String::new, |name| clean_roster_name(name))
        })
        .collect();
    let local_slot = (!names[2].is_empty()).then_some(2);
    (names, local_slot)
}

pub(crate) fn audit_roster(readings: &[String]) -> serde_json::Value {
    let (names, local_slot) = roster(readings);
    serde_json::json!({
        "readings":readings,
        "actual_roster":{"names":names,"local_slot":local_slot,"order":"hud"},
        "identity_policy":"02_hud_single_read_only",
        "selection_samples":0,"sample_limit":HUD_SAMPLE_LIMIT,
        "rule":"single_read_no_vote","empty_readings":"unresolved_no_retry",
        "storage_source":"voted_roster"
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn single_read_keeps_hud_slots_and_exact_normalized_names() {
        let readings = vec![
            " 小 助 专 丢 臭 p 炸 你 ".into(),
            "trust you".into(),
            "Archer".into(),
        ];
        assert_eq!(
            roster(&readings),
            (
                vec![
                    "小助专丢臭p炸你".into(),
                    "trust you".into(),
                    "Archer".into()
                ],
                Some(2)
            )
        );
        let report = audit_roster(&readings);
        assert_eq!(report["rule"], "single_read_no_vote");
        assert_eq!(report["sample_limit"], 1);
        assert!(report.get("tie_break").is_none());
        assert_eq!(report["actual_roster"]["names"][2], "Archer");
    }

    #[test]
    fn empty_single_read_keeps_unknown_slots_without_filling_from_others() {
        assert_eq!(roster(&[]), (vec![String::new(); 3], None));
        assert_eq!(
            roster(&["队友".into(), " ".into()]),
            (vec!["队友".into(), String::new(), String::new()], None)
        );
    }
}
