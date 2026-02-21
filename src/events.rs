use std::cmp::Ordering;

use serde::Serialize;

use crate::types::{BrokerId, Day, InsuredId, LossEventId, PolicyId, SubmissionId, SyndicateId, Year};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize)]
pub enum Peril {
    WindstormAtlantic,
    WindstormEuropean,
    EarthquakeUS,
    EarthquakeJapan,
    Flood,
    Attritional,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Risk {
    pub line_of_business: String,
    pub sum_insured: u64, // pence
    pub territory: String,
    pub limit: u64,
    pub attachment: u64,
    pub perils_covered: Vec<Peril>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct PanelEntry {
    pub syndicate_id: SyndicateId,
    pub share_bps: u32, // basis points; entries must sum to 10_000
    pub premium: u64,   // syndicate's share, pence
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Panel {
    pub entries: Vec<PanelEntry>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[allow(clippy::enum_variant_names)] // LossEvent is a domain term, not a naming error
pub enum Event {
    SimulationStart {
        year_start: Year,
    },
    YearEnd {
        year: Year,
    },
    SubmissionArrived {
        submission_id: SubmissionId,
        broker_id: BrokerId,
        insured_id: InsuredId,
        risk: Risk,
    },
    QuoteRequested {
        submission_id: SubmissionId,
        syndicate_id: SyndicateId,
        is_lead: bool,
    },
    QuoteIssued {
        submission_id: SubmissionId,
        syndicate_id: SyndicateId,
        premium: u64,
        is_lead: bool,
    },
    QuoteDeclined {
        submission_id: SubmissionId,
        syndicate_id: SyndicateId,
    },
    SubmissionAbandoned {
        submission_id: SubmissionId,
    },
    PolicyBound {
        submission_id: SubmissionId,
        panel: Panel,
    },
    LossEvent {
        event_id: LossEventId,
        region: String,
        peril: Peril,
        severity: u64,
    },
    ClaimSettled {
        policy_id: PolicyId,
        syndicate_id: SyndicateId,
        amount: u64,
    },
    SyndicateEntered {
        syndicate_id: SyndicateId,
    },
    SyndicateInsolvency {
        syndicate_id: SyndicateId,
    },
}

/// Unified event record — serves as both the immutable log entry and the
/// priority queue entry. Ordering is by `day` only; `Event` has no
/// meaningful ordering.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct SimEvent {
    pub day: Day,
    pub event: Event,
}

impl Ord for SimEvent {
    fn cmp(&self, other: &Self) -> Ordering {
        self.day.cmp(&other.day)
    }
}

impl PartialOrd for SimEvent {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

#[cfg(test)]
mod tests {
    use std::io::{BufWriter, Write};

    use super::*;
    use crate::types::{LossEventId, SyndicateId};

    #[test]
    fn peril_covered_membership() {
        let risk = Risk {
            line_of_business: "property".to_string(),
            sum_insured: 1_000_000,
            territory: "US-SE".to_string(),
            limit: 500_000,
            attachment: 50_000,
            perils_covered: vec![Peril::WindstormAtlantic, Peril::Flood],
        };
        assert!(risk.perils_covered.contains(&Peril::WindstormAtlantic));
        assert!(risk.perils_covered.contains(&Peril::Flood));
        assert!(!risk.perils_covered.contains(&Peril::EarthquakeUS));
        assert!(!risk.perils_covered.contains(&Peril::Attritional));
    }

    #[test]
    fn sim_event_serializes_day_and_event_fields() {
        let ev = SimEvent {
            day: Day(42),
            event: Event::YearEnd { year: Year(3) },
        };
        let json = serde_json::to_string(&ev).unwrap();
        assert_eq!(json, r#"{"day":42,"event":{"YearEnd":{"year":3}}}"#);
    }

    #[test]
    fn simulation_start_json_shape() {
        let ev = SimEvent {
            day: Day(0),
            event: Event::SimulationStart { year_start: Year(1) },
        };
        let json = serde_json::to_string(&ev).unwrap();
        assert_eq!(json, r#"{"day":0,"event":{"SimulationStart":{"year_start":1}}}"#);
    }

    #[test]
    fn risk_perils_serialize_as_string_array() {
        let risk = Risk {
            line_of_business: "property".to_string(),
            sum_insured: 500_000,
            territory: "US-SE".to_string(),
            limit: 250_000,
            attachment: 25_000,
            perils_covered: vec![Peril::WindstormAtlantic, Peril::Flood],
        };
        let value = serde_json::to_value(&risk).unwrap();
        assert_eq!(value["perils_covered"], serde_json::json!(["WindstormAtlantic", "Flood"]));
    }

    #[test]
    fn policy_bound_panel_entries_serialize() {
        let ev = SimEvent {
            day: Day(10),
            event: Event::PolicyBound {
                submission_id: crate::types::SubmissionId(1),
                panel: Panel {
                    entries: vec![
                        PanelEntry { syndicate_id: SyndicateId(1), share_bps: 6_000, premium: 60_000 },
                        PanelEntry { syndicate_id: SyndicateId(2), share_bps: 4_000, premium: 40_000 },
                    ],
                },
            },
        };
        let value = serde_json::to_value(&ev).unwrap();
        let entries = &value["event"]["PolicyBound"]["panel"]["entries"];
        assert_eq!(entries.as_array().unwrap().len(), 2);
        assert_eq!(entries[0]["syndicate_id"], 1);
        assert_eq!(entries[0]["share_bps"], 6_000);
        assert_eq!(entries[0]["premium"], 60_000);
        assert_eq!(entries[1]["syndicate_id"], 2);
        assert_eq!(entries[1]["share_bps"], 4_000);
        assert_eq!(entries[1]["premium"], 40_000);
    }

    #[test]
    fn ndjson_stream_one_line_per_event() {
        let events = vec![
            SimEvent {
                day: Day(0),
                event: Event::SimulationStart { year_start: Year(1) },
            },
            SimEvent {
                day: Day(359),
                event: Event::YearEnd { year: Year(1) },
            },
            SimEvent {
                day: Day(180),
                event: Event::LossEvent {
                    event_id: LossEventId(1),
                    region: "US-SE".to_string(),
                    peril: Peril::WindstormAtlantic,
                    severity: 1_000_000,
                },
            },
        ];

        let mut buf: Vec<u8> = Vec::new();
        {
            let mut writer = BufWriter::new(&mut buf);
            for e in &events {
                serde_json::to_writer(&mut writer, e).unwrap();
                writeln!(writer).unwrap();
            }
        }

        let text = String::from_utf8(buf).unwrap();
        let lines: Vec<&str> = text.split('\n').filter(|l| !l.is_empty()).collect();
        assert_eq!(lines.len(), 3);
        for line in &lines {
            let v: serde_json::Value = serde_json::from_str(line).unwrap();
            assert!(v.get("day").is_some(), "missing 'day' key in: {line}");
            assert!(v.get("event").is_some(), "missing 'event' key in: {line}");
        }
    }
}
