use std::cmp::Ordering;

use serde::Serialize;

use crate::types::{Day, InsuredId, InsurerId, PolicyId, SubmissionId, Year};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize)]
pub enum Peril {
    WindstormAtlantic,
    Attritional,
}

/// The risk being submitted for coverage.
/// Full coverage: the insurer writes limit = sum_insured, attachment = 0.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Risk {
    pub sum_insured: u64, // monetary units (e.g. USD cents)
    pub territory: String,
    pub perils_covered: Vec<Peril>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub enum Event {
    /// Fires once at Day(0) to bootstrap the simulation. Schedules YearStart(year_start).
    SimulationStart { year_start: Year },
    /// Fires at the start of each simulated year.
    YearStart { year: Year },
    /// Fires at the end of each simulated year.
    YearEnd { year: Year },
    /// An insured requests coverage for the year. Broker routes to a lead insurer.
    CoverageRequested { insured_id: InsuredId, risk: Risk },
    /// Broker asks the selected lead insurer to price a risk.
    LeadQuoteRequested {
        submission_id: SubmissionId,
        insured_id: InsuredId,
        insurer_id: InsurerId,
        risk: Risk,
    },
    /// Lead insurer has priced the risk and issued a quote.
    LeadQuoteIssued {
        submission_id: SubmissionId,
        insured_id: InsuredId,
        insurer_id: InsurerId,
        premium: u64,
    },
    /// Broker presents the quote to the insured.
    QuotePresented {
        submission_id: SubmissionId,
        insured_id: InsuredId,
        insurer_id: InsurerId,
        premium: u64,
    },
    /// Insured accepts the quote. Market creates the policy record.
    QuoteAccepted {
        submission_id: SubmissionId,
        insured_id: InsuredId,
        insurer_id: InsurerId,
        premium: u64,
    },
    /// Insured rejects the quote (no-op in this model; kept for completeness).
    QuoteRejected { submission_id: SubmissionId, insured_id: InsuredId },
    /// Policy is formally bound. Activates the policy for loss routing.
    PolicyBound {
        policy_id: PolicyId,
        submission_id: SubmissionId,
        insured_id: InsuredId,
        insurer_id: InsurerId,
        premium: u64,
    },
    PolicyExpired {
        policy_id: PolicyId,
    },
    #[allow(clippy::enum_variant_names)] // LossEvent is a domain term, not a naming error
    LossEvent {
        event_id: u64,
        peril: Peril,
    },
    InsuredLoss {
        policy_id: PolicyId,
        insured_id: InsuredId,
        peril: Peril,
        ground_up_loss: u64,
    },
    ClaimSettled {
        policy_id: PolicyId,
        insurer_id: InsurerId,
        amount: u64,
        peril: Peril,
    },
}

/// A dispatched event with its simulation day. Position in `Simulation.log` is its implicit sequence number.
///
/// Serves as both the immutable log entry and the priority queue entry. Ordering is by `day` only.
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
    use crate::types::{InsurerId, SubmissionId};

    #[test]
    fn peril_covered_membership() {
        let risk = Risk {
            sum_insured: 1_000_000,
            territory: "US-SE".to_string(),
            perils_covered: vec![Peril::WindstormAtlantic],
        };
        assert!(risk.perils_covered.contains(&Peril::WindstormAtlantic));
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
    fn policy_bound_serializes() {
        let ev = SimEvent {
            day: Day(10),
            event: Event::PolicyBound {
                policy_id: PolicyId(0),
                submission_id: SubmissionId(1),
                insured_id: InsuredId(5),
                insurer_id: InsurerId(2),
                premium: 50_000,
            },
        };
        let value = serde_json::to_value(&ev).unwrap();
        assert_eq!(value["event"]["PolicyBound"]["policy_id"], 0);
        assert_eq!(value["event"]["PolicyBound"]["insurer_id"], 2);
        assert_eq!(value["event"]["PolicyBound"]["insured_id"], 5);
        assert_eq!(value["event"]["PolicyBound"]["premium"], 50_000);
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
                event: Event::LossEvent { event_id: 1, peril: Peril::WindstormAtlantic },
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

    #[test]
    fn quote_chain_events_serialize() {
        let ev = SimEvent {
            day: Day(1),
            event: Event::LeadQuoteRequested {
                submission_id: SubmissionId(0),
                insured_id: InsuredId(1),
                insurer_id: InsurerId(1),
                risk: Risk {
                    sum_insured: 1_000_000,
                    territory: "US-SE".to_string(),
                    perils_covered: vec![Peril::WindstormAtlantic],
                },
            },
        };
        let value = serde_json::to_value(&ev).unwrap();
        assert!(value["event"]["LeadQuoteRequested"].is_object());
    }
}
