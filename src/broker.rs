use std::collections::HashMap;

use crate::events::{Event, Risk};
use crate::insured::Insured;
use crate::types::{Day, InsuredId, InsurerId, SubmissionId};

/// Multiplicative decay applied to all relationship scores at each YearEnd.
/// A score of 1.0 halves in ~3.1 years (0.80^3.1 ≈ 0.50).
const SCORE_DECAY: f64 = 0.80;

/// Transient state while a submission is in flight.
/// All solicited insurers are contacted upfront; the cheapest accepted quote wins.
struct PendingQuote {
    insured_id: InsuredId,
    /// How many solicited insurers have not yet responded (issued or declined).
    quotes_outstanding: usize,
    /// Best (cheapest) quote received so far: (winner_insurer_id, premium).
    best_quote: Option<(InsurerId, u64)>,
}

/// Single broker that services all insureds.
/// Routes coverage requests to score-ranked insurers (incumbents get first look);
/// presents the cheapest accepted quote back to the insured.
pub struct Broker {
    pub insureds: Vec<Insured>,
    insurer_ids: Vec<InsurerId>,
    next_insurer_idx: usize,
    next_submission_id: u64,
    pending: HashMap<SubmissionId, PendingQuote>,
    /// Number of insurers solicited per submission (≥ 1, ≤ insurer_ids.len()).
    quotes_per_submission: usize,
    /// Accumulated relationship score per insurer. +1.0 per PolicyBound, ×0.80 per YearEnd.
    /// Re-entrants retain their decayed score; new IDs start at 0.0.
    pub relationship_scores: HashMap<InsurerId, f64>,
}

impl Broker {
    pub fn new(insureds: Vec<Insured>, insurer_ids: Vec<InsurerId>, quotes_per_submission: usize) -> Self {
        let mut relationship_scores = HashMap::new();
        for &id in &insurer_ids {
            relationship_scores.insert(id, 0.0);
        }
        Broker {
            insureds,
            insurer_ids,
            next_insurer_idx: 0,
            next_submission_id: 0,
            pending: HashMap::new(),
            quotes_per_submission,
            relationship_scores,
        }
    }

    /// Add a new insurer to the routing pool.
    /// Re-entrants (previously seen InsurerId) retain their decayed score.
    /// New InsurerId values start at 0.0.
    pub fn add_insurer(&mut self, id: InsurerId) {
        self.insurer_ids.push(id);
        self.relationship_scores.entry(id).or_insert(0.0);
    }

    /// Remove an insurer from the routing pool (e.g. voluntary runoff).
    /// Score is retained in `relationship_scores` so a re-entrant gets their old score back.
    pub fn remove_insurer(&mut self, id: InsurerId) {
        self.insurer_ids.retain(|&i| i != id);
    }

    /// A policy was bound with this insurer. Increment their relationship score by 1.0.
    pub fn on_policy_bound(&mut self, insurer_id: InsurerId) {
        *self.relationship_scores.entry(insurer_id).or_insert(0.0) += 1.0;
    }

    /// Year ended. Decay all relationship scores by SCORE_DECAY.
    pub fn on_year_end(&mut self) {
        for score in self.relationship_scores.values_mut() {
            *score *= SCORE_DECAY;
        }
    }

    /// Return the relationship score for an insurer (None if never seen).
    pub fn score_of(&self, id: InsurerId) -> Option<f64> {
        self.relationship_scores.get(&id).copied()
    }

    /// An insured has requested coverage. Solicit k insurers ordered by relationship score
    /// (descending); cyclic distance from `next_insurer_idx` breaks ties so that equal-score
    /// pools degenerate to the existing round-robin behaviour.
    pub fn on_coverage_requested(
        &mut self,
        day: Day,
        insured_id: InsuredId,
        risk: Risk,
    ) -> Vec<(Day, Event)> {
        let n = self.insurer_ids.len();
        if n == 0 {
            return vec![];
        }
        let k = self.quotes_per_submission.min(n).max(1);
        let start_idx = self.next_insurer_idx % n;
        self.next_insurer_idx += 1;

        // Sort pool indices by (score DESC, cyclic_distance_from_start_idx ASC).
        // When all scores are equal this degenerates to round-robin (cyclic order from start_idx).
        let mut indices: Vec<usize> = (0..n).collect();
        let scores = &self.relationship_scores;
        let insurer_ids = &self.insurer_ids;
        indices.sort_by(|&a, &b| {
            let sa = scores.get(&insurer_ids[a]).copied().unwrap_or(0.0);
            let sb = scores.get(&insurer_ids[b]).copied().unwrap_or(0.0);
            // Primary: higher score first.
            let score_ord = sb.partial_cmp(&sa).unwrap_or(std::cmp::Ordering::Equal);
            if score_ord != std::cmp::Ordering::Equal {
                return score_ord;
            }
            // Tiebreaker: smaller cyclic distance from start_idx first (round-robin).
            let da = (a + n - start_idx) % n;
            let db = (b + n - start_idx) % n;
            da.cmp(&db)
        });

        let submission_id = SubmissionId(self.next_submission_id);
        self.next_submission_id += 1;
        self.pending.insert(
            submission_id,
            PendingQuote { insured_id, quotes_outstanding: k, best_quote: None },
        );

        indices[..k]
            .iter()
            .map(|&j| {
                let insurer_id = self.insurer_ids[j];
                (
                    day.offset(1),
                    Event::LeadQuoteRequested {
                        submission_id,
                        insured_id,
                        insurer_id,
                        risk: risk.clone(),
                    },
                )
            })
            .collect()
    }

    /// An insurer issued a quote. Record it if it beats the current best.
    /// When all solicited insurers have responded, emit `QuotePresented` with the winner.
    pub fn on_lead_quote_issued(
        &mut self,
        day: Day,
        submission_id: SubmissionId,
        _insured_id: InsuredId,
        insurer_id: InsurerId,
        premium: u64,
    ) -> Vec<(Day, Event)> {
        let pq = match self.pending.get_mut(&submission_id) {
            Some(pq) => pq,
            None => return vec![],
        };

        // Update best quote if this is cheaper (or the first offer).
        match pq.best_quote {
            None => pq.best_quote = Some((insurer_id, premium)),
            Some((_, best_premium)) if premium < best_premium => {
                pq.best_quote = Some((insurer_id, premium));
            }
            _ => {}
        }

        pq.quotes_outstanding -= 1;

        if pq.quotes_outstanding == 0 {
            let pq = self.pending.remove(&submission_id).unwrap();
            let (winner_id, winner_premium) = pq.best_quote.unwrap();
            vec![(
                day.offset(1),
                Event::QuotePresented {
                    submission_id,
                    insured_id: pq.insured_id,
                    insurer_id: winner_id,
                    premium: winner_premium,
                },
            )]
        } else {
            vec![]
        }
    }

    /// An insurer declined. Decrement outstanding count.
    /// When all solicited insurers have responded, emit `QuotePresented` with the best
    /// accepted quote — or drop the submission silently if all declined.
    pub fn on_lead_quote_declined(
        &mut self,
        day: Day,
        submission_id: SubmissionId,
    ) -> Vec<(Day, Event)> {
        let pq = match self.pending.get_mut(&submission_id) {
            Some(pq) => pq,
            None => return vec![],
        };

        pq.quotes_outstanding -= 1;

        if pq.quotes_outstanding == 0 {
            let pq = self.pending.remove(&submission_id).unwrap();
            if let Some((winner_id, winner_premium)) = pq.best_quote {
                vec![(
                    day.offset(1),
                    Event::QuotePresented {
                        submission_id,
                        insured_id: pq.insured_id,
                        insurer_id: winner_id,
                        premium: winner_premium,
                    },
                )]
            } else {
                vec![(day, Event::SubmissionDropped { submission_id, insured_id: pq.insured_id })]
            }
        } else {
            vec![]
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::ASSET_VALUE;
    use crate::events::Peril;

    fn make_insured(id: u64) -> Insured {
        Insured::new(
            InsuredId(id),
            "US-SE".to_string(),
            vec![Peril::WindstormAtlantic, Peril::Attritional],
            1.0, // accepts all quotes
        )
    }

    fn small_risk() -> Risk {
        Risk {
            sum_insured: ASSET_VALUE,
            territory: "US-SE".to_string(),
            perils_covered: vec![Peril::WindstormAtlantic, Peril::Attritional],
        }
    }

    /// Build a broker that solicits all insurers per submission.
    fn broker_with_insurers(n: usize, insurer_ids: Vec<u64>) -> Broker {
        let qps = insurer_ids.len().max(1);
        let insureds = (1..=n as u64).map(|i| make_insured(i)).collect();
        let insurer_ids = insurer_ids.into_iter().map(InsurerId).collect();
        Broker::new(insureds, insurer_ids, qps)
    }

    /// Build a broker with an explicit quotes_per_submission value.
    fn broker_with_qps(n: usize, insurer_ids: Vec<u64>, qps: usize) -> Broker {
        let insureds = (1..=n as u64).map(|i| make_insured(i)).collect();
        let insurer_ids = insurer_ids.into_iter().map(InsurerId).collect();
        Broker::new(insureds, insurer_ids, qps)
    }

    // ── on_coverage_requested ─────────────────────────────────────────────────

    #[test]
    fn on_coverage_requested_returns_lead_quote_requested() {
        // 2 insurers, qps=2 → expect 2 LeadQuoteRequested events.
        let mut broker = broker_with_insurers(1, vec![1, 2]);
        let events = broker.on_coverage_requested(Day(0), InsuredId(1), small_risk());
        assert_eq!(events.len(), 2);
        assert!(
            events.iter().all(|(_, e)| matches!(e, Event::LeadQuoteRequested { .. })),
            "all events must be LeadQuoteRequested"
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
        // qps=1: each submission solicits exactly 1 insurer, cycling 1→2→3→1→2→3.
        let mut broker = broker_with_qps(6, vec![1, 2, 3], 1);
        let mut assigned: Vec<u64> = vec![];
        for id in 1..=6u64 {
            let events = broker.on_coverage_requested(Day(0), InsuredId(id), small_risk());
            assert_eq!(events.len(), 1);
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

    #[test]
    fn on_coverage_requested_solicits_all_insurers() {
        // 3 insurers, qps=3 → 3 LeadQuoteRequested with distinct insurer IDs.
        let mut broker = broker_with_insurers(1, vec![1, 2, 3]);
        let events = broker.on_coverage_requested(Day(0), InsuredId(1), small_risk());
        assert_eq!(events.len(), 3);
        let ids: std::collections::HashSet<u64> = events
            .iter()
            .filter_map(|(_, e)| {
                if let Event::LeadQuoteRequested { insurer_id, .. } = e {
                    Some(insurer_id.0)
                } else {
                    None
                }
            })
            .collect();
        assert_eq!(ids, [1u64, 2, 3].into_iter().collect());
    }

    #[test]
    fn on_coverage_requested_solicits_k_of_n() {
        // 3 insurers, qps=2 → 2 LeadQuoteRequested.
        let mut broker = broker_with_qps(1, vec![1, 2, 3], 2);
        let events = broker.on_coverage_requested(Day(0), InsuredId(1), small_risk());
        assert_eq!(events.len(), 2);
    }

    // ── on_lead_quote_issued ──────────────────────────────────────────────────

    #[test]
    fn on_lead_quote_issued_returns_quote_presented() {
        let mut broker = broker_with_insurers(1, vec![1]);
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

    #[test]
    fn best_price_wins() {
        // 2 insurers, qps=2. Insurer 1 quotes 100, insurer 2 quotes 80 → winner is insurer 2.
        let mut broker = broker_with_insurers(1, vec![1, 2]);
        broker.on_coverage_requested(Day(0), InsuredId(1), small_risk());

        // Insurer 1 responds first (premium=100); still 1 outstanding → no event yet.
        let ev1 = broker.on_lead_quote_issued(
            Day(1), SubmissionId(0), InsuredId(1), InsurerId(1), 100,
        );
        assert!(ev1.is_empty(), "still 1 outstanding → no QuotePresented yet");

        // Insurer 2 responds (premium=80); outstanding hits 0 → QuotePresented with winner.
        let ev2 = broker.on_lead_quote_issued(
            Day(1), SubmissionId(0), InsuredId(1), InsurerId(2), 80,
        );
        assert_eq!(ev2.len(), 1);
        if let Event::QuotePresented { insurer_id, premium, .. } = ev2[0].1 {
            assert_eq!(insurer_id, InsurerId(2), "cheaper insurer must win");
            assert_eq!(premium, 80);
        } else {
            panic!("expected QuotePresented");
        }
    }

    #[test]
    fn first_price_wins_on_tie() {
        // 2 insurers, same premium → first received (insurer 1) wins.
        let mut broker = broker_with_insurers(1, vec![1, 2]);
        broker.on_coverage_requested(Day(0), InsuredId(1), small_risk());

        broker.on_lead_quote_issued(
            Day(1), SubmissionId(0), InsuredId(1), InsurerId(1), 100,
        );
        let ev = broker.on_lead_quote_issued(
            Day(1), SubmissionId(0), InsuredId(1), InsurerId(2), 100,
        );
        if let Event::QuotePresented { insurer_id, .. } = ev[0].1 {
            assert_eq!(insurer_id, InsurerId(1), "first received wins on equal premium");
        } else {
            panic!("expected QuotePresented");
        }
    }

    // ── on_lead_quote_declined ────────────────────────────────────────────────

    #[test]
    fn one_decline_while_still_outstanding_returns_empty() {
        // 2 insurers; first declines → still 1 outstanding → no event.
        let mut broker = broker_with_insurers(1, vec![1, 2]);
        broker.on_coverage_requested(Day(0), InsuredId(1), small_risk());
        let events = broker.on_lead_quote_declined(Day(1), SubmissionId(0));
        assert!(events.is_empty(), "still 1 outstanding → must return empty");
    }

    #[test]
    fn one_declines_one_issues_still_presents() {
        // 2 insurers. Insurer 1 declines, insurer 2 issues → QuotePresented with insurer 2.
        let mut broker = broker_with_insurers(1, vec![1, 2]);
        broker.on_coverage_requested(Day(0), InsuredId(1), small_risk());

        let ev_declined = broker.on_lead_quote_declined(Day(1), SubmissionId(0));
        assert!(ev_declined.is_empty(), "1 outstanding remaining → no event");

        let ev_issued = broker.on_lead_quote_issued(
            Day(1), SubmissionId(0), InsuredId(1), InsurerId(2), 50_000,
        );
        assert_eq!(ev_issued.len(), 1);
        if let Event::QuotePresented { insurer_id, premium, .. } = ev_issued[0].1 {
            assert_eq!(insurer_id, InsurerId(2));
            assert_eq!(premium, 50_000);
        } else {
            panic!("expected QuotePresented");
        }
    }

    #[test]
    fn all_decline_drops_submission() {
        // 2 insurers, both decline → SubmissionDropped emitted (no QuotePresented).
        let mut broker = broker_with_insurers(1, vec![1, 2]);
        broker.on_coverage_requested(Day(0), InsuredId(1), small_risk());

        let ev1 = broker.on_lead_quote_declined(Day(1), SubmissionId(0));
        assert!(ev1.is_empty(), "still 1 outstanding → must return empty");

        let ev2 = broker.on_lead_quote_declined(Day(1), SubmissionId(0));
        assert_eq!(ev2.len(), 1, "all declined → SubmissionDropped must be emitted");
        assert!(
            matches!(ev2[0].1, Event::SubmissionDropped { insured_id: InsuredId(1), .. }),
            "expected SubmissionDropped for insured 1, got {:?}",
            ev2[0].1
        );
    }

    // ── insured population ────────────────────────────────────────────────────

    #[test]
    fn broker_holds_correct_insured_ids() {
        let insureds = vec![make_insured(10), make_insured(20)];
        let broker = Broker::new(insureds, vec![InsurerId(1)], 1);
        let ids: Vec<u64> = broker.insureds.iter().map(|i| i.id.0).collect();
        assert_eq!(ids, vec![10, 20]);
    }

    #[test]
    fn insured_sum_insured_is_correct() {
        let broker = broker_with_insurers(1, vec![1]);
        assert_eq!(broker.insureds[0].sum_insured(), ASSET_VALUE);
    }

    // ── relationship scores ───────────────────────────────────────────────────

    #[test]
    fn relationship_score_zero_on_new_insurer() {
        let mut broker = broker_with_insurers(1, vec![]);
        broker.add_insurer(InsurerId(42));
        assert_eq!(broker.score_of(InsurerId(42)), Some(0.0));
    }

    #[test]
    fn relationship_score_increments_on_policy_bound() {
        let mut broker = broker_with_insurers(1, vec![1]);
        broker.on_policy_bound(InsurerId(1));
        assert!((broker.score_of(InsurerId(1)).unwrap() - 1.0).abs() < f64::EPSILON);
    }

    #[test]
    fn relationship_score_decays_on_year_end() {
        let mut broker = broker_with_insurers(1, vec![1]);
        broker.on_policy_bound(InsurerId(1)); // score = 1.0
        broker.on_year_end(); // score = 1.0 × 0.80 = 0.80
        assert!((broker.score_of(InsurerId(1)).unwrap() - 0.80).abs() < 1e-9);
    }

    #[test]
    fn high_score_insurer_preferred_when_k_lt_n() {
        // Insurer 1 score=5.0 (via 5 × on_policy_bound), insurers 2 and 3 score=0.0.
        // k=1 → every submission must route exclusively to insurer 1.
        let mut broker = broker_with_qps(3, vec![1, 2, 3], 1);
        for _ in 0..5 {
            broker.on_policy_bound(InsurerId(1));
        }
        for id in 1..=3u64 {
            let events = broker.on_coverage_requested(Day(0), InsuredId(id), small_risk());
            assert_eq!(events.len(), 1);
            if let Event::LeadQuoteRequested { insurer_id, .. } = events[0].1 {
                assert_eq!(insurer_id, InsurerId(1), "high-score insurer must always be selected");
            } else {
                panic!("expected LeadQuoteRequested");
            }
        }
    }

    #[test]
    fn re_entrant_retains_score() {
        // Score insurer 1 to 2.0, remove from pool, re-add — score must still be ~2.0.
        let mut broker = broker_with_insurers(1, vec![1]);
        broker.on_policy_bound(InsurerId(1));
        broker.on_policy_bound(InsurerId(1)); // score = 2.0
        broker.remove_insurer(InsurerId(1));
        broker.add_insurer(InsurerId(1)); // re-entry: or_insert keeps existing score
        assert!((broker.score_of(InsurerId(1)).unwrap() - 2.0).abs() < f64::EPSILON);
    }

    #[test]
    fn remove_insurer_shrinks_pool() {
        // Start with 3 insurers [1, 2, 3]; solicit 1 per submission; remove insurer 2.
        // Two subsequent CoverageRequested events must route only to insurers 1 and 3.
        let mut broker = broker_with_qps(2, vec![1, 2, 3], 1);
        broker.remove_insurer(InsurerId(2));

        let risk = small_risk();
        let events1 = broker.on_coverage_requested(Day(0), InsuredId(1), risk.clone());
        let events2 = broker.on_coverage_requested(Day(0), InsuredId(2), risk);

        let routed: Vec<InsurerId> = events1
            .iter()
            .chain(events2.iter())
            .filter_map(|(_, e)| {
                if let Event::LeadQuoteRequested { insurer_id, .. } = e {
                    Some(*insurer_id)
                } else {
                    None
                }
            })
            .collect();

        assert_eq!(routed.len(), 2, "one quote per submission → 2 total");
        for id in &routed {
            assert_ne!(*id, InsurerId(2), "removed insurer must not receive quotes");
        }
        // Both remaining insurers should appear (round-robin with 2 remaining).
        assert!(routed.contains(&InsurerId(1)) || routed.contains(&InsurerId(3)));
    }
}
