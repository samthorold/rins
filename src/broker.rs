use std::collections::HashMap;

use crate::events::{Event, Risk};
use crate::insured::Insured;
use crate::types::{Day, InsuredId, InsurerId, SubmissionId};

/// Transient state while a submission is in flight.
/// Fields are kept for future extensions (e.g. broker relationship scoring).
#[allow(dead_code)]
struct PendingQuote {
    insured_id: InsuredId,
    insurer_id: InsurerId,
}

/// Single broker that services all insureds.
/// Routes coverage requests to insurers via round-robin and presents quotes back to insureds.
pub struct Broker {
    pub insureds: Vec<Insured>,
    insurer_ids: Vec<InsurerId>,
    next_insurer_idx: usize,
    next_submission_id: u64,
    pending: HashMap<SubmissionId, PendingQuote>,
}

impl Broker {
    pub fn new(insureds: Vec<Insured>, insurer_ids: Vec<InsurerId>) -> Self {
        Broker {
            insureds,
            insurer_ids,
            next_insurer_idx: 0,
            next_submission_id: 0,
            pending: HashMap::new(),
        }
    }

    /// An insured has requested coverage. Pick a lead insurer (round-robin), create a
    /// submission, and schedule `LeadQuoteRequested` for next day.
    pub fn on_coverage_requested(
        &mut self,
        day: Day,
        insured_id: InsuredId,
        risk: Risk,
    ) -> Vec<(Day, Event)> {
        if self.insurer_ids.is_empty() {
            return vec![];
        }
        let insurer_id = self.insurer_ids[self.next_insurer_idx % self.insurer_ids.len()];
        self.next_insurer_idx += 1;
        let submission_id = SubmissionId(self.next_submission_id);
        self.next_submission_id += 1;
        self.pending.insert(submission_id, PendingQuote { insured_id, insurer_id });
        vec![(
            day.offset(1),
            Event::LeadQuoteRequested { submission_id, insured_id, insurer_id, risk },
        )]
    }

    /// The lead insurer has issued a quote. Remove the pending entry and present the quote
    /// to the insured at `day+1`.
    pub fn on_lead_quote_issued(
        &mut self,
        day: Day,
        submission_id: SubmissionId,
        insured_id: InsuredId,
        insurer_id: InsurerId,
        premium: u64,
    ) -> Vec<(Day, Event)> {
        if self.pending.remove(&submission_id).is_none() {
            return vec![];
        }
        vec![(
            day.offset(1),
            Event::QuotePresented { submission_id, insured_id, insurer_id, premium },
        )]
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::{LARGE_ASSET_VALUE, SMALL_ASSET_VALUE};
    use crate::events::Peril;
    use crate::insured::AssetType;

    fn make_insured(id: u64, asset_type: AssetType) -> Insured {
        Insured::new(
            InsuredId(id),
            asset_type,
            "US-SE".to_string(),
            vec![Peril::WindstormAtlantic, Peril::Attritional],
        )
    }

    fn small_risk() -> Risk {
        Risk {
            sum_insured: SMALL_ASSET_VALUE,
            territory: "US-SE".to_string(),
            perils_covered: vec![Peril::WindstormAtlantic, Peril::Attritional],
        }
    }

    fn broker_with_insurers(n_small: usize, insurer_ids: Vec<u64>) -> Broker {
        let insureds = (1..=n_small as u64)
            .map(|i| make_insured(i, AssetType::Small))
            .collect();
        let insurer_ids = insurer_ids.into_iter().map(InsurerId).collect();
        Broker::new(insureds, insurer_ids)
    }

    // ── on_coverage_requested ─────────────────────────────────────────────────

    #[test]
    fn on_coverage_requested_returns_lead_quote_requested() {
        let mut broker = broker_with_insurers(1, vec![1, 2]);
        let events =
            broker.on_coverage_requested(Day(0), InsuredId(1), small_risk());
        assert_eq!(events.len(), 1);
        assert!(
            matches!(events[0].1, Event::LeadQuoteRequested { .. }),
            "must return LeadQuoteRequested"
        );
    }

    #[test]
    fn on_coverage_requested_scheduled_day_plus_one() {
        let mut broker = broker_with_insurers(1, vec![1]);
        let events = broker.on_coverage_requested(Day(5), InsuredId(1), small_risk());
        assert_eq!(events[0].0, Day(6), "LeadQuoteRequested must fire at day+1");
    }

    #[test]
    fn on_coverage_requested_carries_correct_fields() {
        let mut broker = broker_with_insurers(1, vec![7]);
        let risk = small_risk();
        let events = broker.on_coverage_requested(Day(0), InsuredId(42), risk.clone());
        if let Event::LeadQuoteRequested {
            submission_id,
            insured_id,
            insurer_id,
            risk: ev_risk,
        } = &events[0].1
        {
            assert_eq!(*insured_id, InsuredId(42));
            assert_eq!(*insurer_id, InsurerId(7));
            assert_eq!(*submission_id, SubmissionId(0));
            assert_eq!(*ev_risk, risk);
        } else {
            panic!("expected LeadQuoteRequested");
        }
    }

    #[test]
    fn on_coverage_requested_round_robin() {
        let mut broker = broker_with_insurers(6, vec![1, 2, 3]);
        let mut assigned: Vec<u64> = vec![];
        for id in 1..=6u64 {
            let events =
                broker.on_coverage_requested(Day(0), InsuredId(id), small_risk());
            if let Event::LeadQuoteRequested { insurer_id, .. } = events[0].1 {
                assigned.push(insurer_id.0);
            }
        }
        assert_eq!(assigned, vec![1, 2, 3, 1, 2, 3], "round-robin must cycle 1,2,3,1,2,3");
    }

    #[test]
    fn on_coverage_requested_empty_insurers_returns_empty() {
        let mut broker = broker_with_insurers(1, vec![]);
        let events = broker.on_coverage_requested(Day(0), InsuredId(1), small_risk());
        assert!(events.is_empty(), "no insurers → no LeadQuoteRequested");
    }

    #[test]
    fn on_coverage_requested_increments_submission_id() {
        let mut broker = broker_with_insurers(3, vec![1]);
        let mut ids = vec![];
        for id in 1..=3u64 {
            let events = broker.on_coverage_requested(Day(0), InsuredId(id), small_risk());
            if let Event::LeadQuoteRequested { submission_id, .. } = events[0].1 {
                ids.push(submission_id.0);
            }
        }
        assert_eq!(ids, vec![0, 1, 2], "submission_id must increment per request");
    }

    // ── on_lead_quote_issued ──────────────────────────────────────────────────

    #[test]
    fn on_lead_quote_issued_returns_quote_presented() {
        let mut broker = broker_with_insurers(1, vec![1]);
        // First create a pending submission.
        broker.on_coverage_requested(Day(0), InsuredId(1), small_risk());
        let events = broker.on_lead_quote_issued(
            Day(1),
            SubmissionId(0),
            InsuredId(1),
            InsurerId(1),
            50_000,
        );
        assert_eq!(events.len(), 1);
        assert!(matches!(events[0].1, Event::QuotePresented { .. }));
    }

    #[test]
    fn on_lead_quote_issued_scheduled_day_plus_one() {
        let mut broker = broker_with_insurers(1, vec![1]);
        broker.on_coverage_requested(Day(0), InsuredId(1), small_risk());
        let events = broker.on_lead_quote_issued(
            Day(1),
            SubmissionId(0),
            InsuredId(1),
            InsurerId(1),
            50_000,
        );
        assert_eq!(events[0].0, Day(2), "QuotePresented must fire at day+1 from LeadQuoteIssued");
    }

    #[test]
    fn on_lead_quote_issued_carries_correct_fields() {
        let mut broker = broker_with_insurers(1, vec![5]);
        broker.on_coverage_requested(Day(0), InsuredId(10), small_risk());
        let events = broker.on_lead_quote_issued(
            Day(1),
            SubmissionId(0),
            InsuredId(10),
            InsurerId(5),
            99_000,
        );
        if let Event::QuotePresented {
            submission_id,
            insured_id,
            insurer_id,
            premium,
        } = events[0].1
        {
            assert_eq!(submission_id, SubmissionId(0));
            assert_eq!(insured_id, InsuredId(10));
            assert_eq!(insurer_id, InsurerId(5));
            assert_eq!(premium, 99_000);
        } else {
            panic!("expected QuotePresented");
        }
    }

    #[test]
    fn on_lead_quote_issued_unknown_submission_returns_empty() {
        let mut broker = broker_with_insurers(1, vec![1]);
        let events = broker.on_lead_quote_issued(
            Day(1),
            SubmissionId(999),
            InsuredId(1),
            InsurerId(1),
            50_000,
        );
        assert!(events.is_empty(), "unknown submission_id must produce no events");
    }

    // ── insured population ────────────────────────────────────────────────────

    #[test]
    fn broker_holds_correct_insured_ids() {
        let insureds = vec![
            make_insured(10, AssetType::Small),
            make_insured(20, AssetType::Large),
        ];
        let broker = Broker::new(insureds, vec![InsurerId(1)]);
        let ids: Vec<u64> = broker.insureds.iter().map(|i| i.id.0).collect();
        assert_eq!(ids, vec![10, 20]);
    }

    #[test]
    fn small_insured_sum_insured_is_correct() {
        let broker = broker_with_insurers(1, vec![1]);
        assert_eq!(broker.insureds[0].sum_insured(), SMALL_ASSET_VALUE);
    }

    #[test]
    fn large_insured_sum_insured_is_correct() {
        let insureds = vec![make_insured(1, AssetType::Large)];
        let broker = Broker::new(insureds, vec![InsurerId(1)]);
        assert_eq!(broker.insureds[0].sum_insured(), LARGE_ASSET_VALUE);
    }
}
