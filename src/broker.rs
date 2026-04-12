use std::collections::HashMap;

use crate::events::{Event, Risk};
use crate::insured::Insured;
use crate::types::{Day, InsuredId, InsurerId, SubmissionId};

/// Multiplicative decay applied to all relationship scores at each YearEnd.
/// A score of 1.0 halves in ~3.1 years (0.80^3.1 ≈ 0.50).
const SCORE_DECAY: f64 = 0.80;

/// Decline counts are reset to zero at each YearEnd (not decayed).
/// Cat-agg capacity resets naturally as old policies expire across the year boundary,
/// so an insurer that was full last year may have room again this year.

/// Transient state while a submission is in flight.
/// Lines are accumulated greedily in solicitation (score) order.
struct PendingQuote {
    insured_id: InsuredId,
    /// The highest-scored solicited insurer — sets terms; identified at request time.
    leader_id: InsurerId,
    /// How many solicited insurers have not yet responded (issued or declined).
    quotes_outstanding: usize,
    /// Lines received so far: (insurer_id, premium, offered_line_size).
    panel_lines: Vec<(InsurerId, u64, f64)>,
    /// Sum of offered line sizes received so far.
    accumulated_line: f64,
}

/// Single broker that services all insureds.
/// Routes coverage requests to score-ranked insurers (incumbents get first look);
/// assembles a panel of fractional lines, normalised to sum to 1.0.
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
    /// Count of declines received from each insurer since the last YearEnd.
    /// Used as a secondary sort key to route away from capacity-constrained insurers.
    /// Reset to 0.0 at each YearEnd.
    decline_counts: HashMap<InsurerId, f64>,
}

impl Broker {
    pub fn new(insureds: Vec<Insured>, insurer_ids: Vec<InsurerId>, quotes_per_submission: usize) -> Self {
        let mut relationship_scores = HashMap::new();
        let mut decline_counts = HashMap::new();
        for &id in &insurer_ids {
            relationship_scores.insert(id, 0.0);
            decline_counts.insert(id, 0.0);
        }
        Broker {
            insureds,
            insurer_ids,
            next_insurer_idx: 0,
            next_submission_id: 0,
            pending: HashMap::new(),
            quotes_per_submission,
            relationship_scores,
            decline_counts,
        }
    }

    /// Add a new insurer to the routing pool.
    /// Re-entrants (previously seen InsurerId) retain their decayed score.
    /// New InsurerId values start at 0.0.
    pub fn add_insurer(&mut self, id: InsurerId) {
        self.insurer_ids.push(id);
        self.relationship_scores.entry(id).or_insert(0.0);
        self.decline_counts.entry(id).or_insert(0.0);
    }

    /// A policy was bound with this insurer. Increment their relationship score by 1.0.
    pub fn on_policy_bound(&mut self, insurer_id: InsurerId) {
        *self.relationship_scores.entry(insurer_id).or_insert(0.0) += 1.0;
    }

    /// Year ended. Decay all relationship scores by SCORE_DECAY and reset decline counts.
    pub fn on_year_end(&mut self) {
        for score in self.relationship_scores.values_mut() {
            *score *= SCORE_DECAY;
        }
        for count in self.decline_counts.values_mut() {
            *count = 0.0;
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

        // Sort pool indices by (net_score DESC, cyclic_distance_from_start_idx ASC) where
        //   net_score = relationship_score − decline_count
        // Relationship score rewards incumbents; subtracting decline_count deprioritises
        // capacity-constrained insurers even when they have high relationship scores.
        // Cyclic distance breaks exact ties (round-robin fallback).
        let mut indices: Vec<usize> = (0..n).collect();
        let scores = &self.relationship_scores;
        let declines = &self.decline_counts;
        let insurer_ids = &self.insurer_ids;
        indices.sort_by(|&a, &b| {
            let net_a = scores.get(&insurer_ids[a]).copied().unwrap_or(0.0)
                - declines.get(&insurer_ids[a]).copied().unwrap_or(0.0);
            let net_b = scores.get(&insurer_ids[b]).copied().unwrap_or(0.0)
                - declines.get(&insurer_ids[b]).copied().unwrap_or(0.0);
            // Primary: higher net score first.
            let net_ord = net_b.partial_cmp(&net_a).unwrap_or(std::cmp::Ordering::Equal);
            if net_ord != std::cmp::Ordering::Equal {
                return net_ord;
            }
            // Tiebreaker: smaller cyclic distance from start_idx first (round-robin).
            let da = (a + n - start_idx) % n;
            let db = (b + n - start_idx) % n;
            da.cmp(&db)
        });

        let submission_id = SubmissionId(self.next_submission_id);
        self.next_submission_id += 1;

        // Leader is the highest-scored insurer in the solicited set.
        let leader_id = self.insurer_ids[indices[0]];
        self.pending.insert(
            submission_id,
            PendingQuote {
                insured_id,
                leader_id,
                quotes_outstanding: k,
                panel_lines: vec![],
                accumulated_line: 0.0,
            },
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

    /// An insurer issued a quote with a line_size. Accumulate into the panel.
    /// If accumulated_line ≥ 1.0 or all solicited insurers have responded,
    /// finalise the panel and emit `QuotePresented`.
    pub fn on_lead_quote_issued(
        &mut self,
        day: Day,
        submission_id: SubmissionId,
        _insured_id: InsuredId,
        insurer_id: InsurerId,
        premium: u64,
        line_size: f64,
    ) -> Vec<(Day, Event)> {
        let pq = match self.pending.get_mut(&submission_id) {
            Some(pq) => pq,
            None => return vec![],
        };

        pq.panel_lines.push((insurer_id, premium, line_size));
        pq.accumulated_line += line_size;
        pq.quotes_outstanding -= 1;

        if pq.accumulated_line >= 1.0 || pq.quotes_outstanding == 0 {
            let pq = self.pending.remove(&submission_id).unwrap();
            self.finalise_panel(day, submission_id, pq)
        } else {
            vec![]
        }
    }

    /// An insurer declined. Record the decline, decrement outstanding count.
    /// When all solicited insurers have responded, finalise the panel with whatever
    /// accepted quotes were received — or drop the submission if none accepted.
    pub fn on_lead_quote_declined(
        &mut self,
        day: Day,
        submission_id: SubmissionId,
        insurer_id: InsurerId,
    ) -> Vec<(Day, Event)> {
        *self.decline_counts.entry(insurer_id).or_insert(0.0) += 1.0;
        let pq = match self.pending.get_mut(&submission_id) {
            Some(pq) => pq,
            None => return vec![],
        };

        pq.quotes_outstanding -= 1;

        if pq.quotes_outstanding == 0 {
            let pq = self.pending.remove(&submission_id).unwrap();
            self.finalise_panel(day, submission_id, pq)
        } else {
            vec![]
        }
    }

    /// Trim panel lines to fill exactly 1.0, scale to normalise, then emit
    /// `QuotePresented` with blended premium — or `SubmissionDropped` if no lines.
    fn finalise_panel(
        &self,
        day: Day,
        submission_id: SubmissionId,
        pq: PendingQuote,
    ) -> Vec<(Day, Event)> {
        if pq.panel_lines.is_empty() || pq.accumulated_line == 0.0 {
            return vec![(day, Event::SubmissionDropped { submission_id, insured_id: pq.insured_id })];
        }

        // Reorder so the leader is always first; remaining in response-arrival order.
        let mut ordered = pq.panel_lines.clone();
        if let Some(leader_pos) = ordered.iter().position(|&(id, _, _)| id == pq.leader_id) {
            if leader_pos != 0 {
                ordered.swap(0, leader_pos);
            }
        }

        // Greedily include lines up to a total of 1.0; cap the last line if it would overflow.
        let mut running = 0.0f64;
        let mut included: Vec<(InsurerId, u64, f64)> = vec![];
        for &(ins_id, prem, line) in &ordered {
            let room = 1.0 - running;
            if room <= 0.0 { break; }
            let take = line.min(room);
            included.push((ins_id, prem, take));
            running += take;
        }

        // Normalise so shares sum to exactly 1.0 (handles floating-point imprecision).
        let actual_total: f64 = included.iter().map(|&(_, _, l)| l).sum();
        let panel: Vec<(InsurerId, f64)> = included.iter()
            .map(|&(id, _, l)| (id, l / actual_total))
            .collect();

        // Blended premium = Σ share_i × premium_i (rounded to nearest cent).
        let blended_premium = included.iter()
            .map(|&(_, prem, l)| prem as f64 * l / actual_total)
            .sum::<f64>()
            .round() as u64;

        // Effective leader = first insurer in the assembled panel.
        // If the original leader provided a line they are first (reordered above);
        // otherwise the first responding insurer takes the lead role.
        let effective_leader = panel[0].0;

        vec![(
            day.offset(1),
            Event::QuotePresented {
                submission_id,
                insured_id: pq.insured_id,
                leader_id: effective_leader,
                panel,
                premium: blended_premium,
            },
        )]
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
            Day(1), SubmissionId(0), InsuredId(1), InsurerId(1), 50_000, 1.0,
        );
        assert_eq!(events.len(), 1);
        assert!(matches!(events[0].1, Event::QuotePresented { .. }));
    }

    #[test]
    fn on_lead_quote_issued_scheduled_day_plus_one() {
        let mut broker = broker_with_insurers(1, vec![1]);
        broker.on_coverage_requested(Day(0), InsuredId(1), small_risk());
        let events = broker.on_lead_quote_issued(
            Day(1), SubmissionId(0), InsuredId(1), InsurerId(1), 50_000, 1.0,
        );
        assert_eq!(events[0].0, Day(2), "QuotePresented must fire at day+1 from LeadQuoteIssued");
    }

    #[test]
    fn on_lead_quote_issued_carries_correct_fields() {
        let mut broker = broker_with_insurers(1, vec![5]);
        broker.on_coverage_requested(Day(0), InsuredId(10), small_risk());
        let events = broker.on_lead_quote_issued(
            Day(1), SubmissionId(0), InsuredId(10), InsurerId(5), 99_000, 1.0,
        );
        if let Event::QuotePresented {
            submission_id,
            insured_id,
            leader_id,
            panel,
            premium,
        } = &events[0].1
        {
            assert_eq!(*submission_id, SubmissionId(0));
            assert_eq!(*insured_id, InsuredId(10));
            assert_eq!(*leader_id, InsurerId(5));
            assert_eq!(panel.len(), 1);
            assert_eq!(panel[0].0, InsurerId(5));
            assert!((panel[0].1 - 1.0).abs() < 1e-9);
            assert_eq!(*premium, 99_000);
        } else {
            panic!("expected QuotePresented");
        }
    }

    #[test]
    fn on_lead_quote_issued_unknown_submission_returns_empty() {
        let mut broker = broker_with_insurers(1, vec![1]);
        let events = broker.on_lead_quote_issued(
            Day(1), SubmissionId(999), InsuredId(1), InsurerId(1), 50_000, 1.0,
        );
        assert!(events.is_empty(), "unknown submission_id must produce no events");
    }

    #[test]
    fn panel_fills_from_two_insurers_with_large_lines() {
        // 2 insurers both offering line_size=0.7 → accumulated=1.4 ≥ 1.0 after first response.
        // First response (insurer 1, line=0.7): accumulated=0.7 < 1.0, wait.
        // Second response (insurer 2, line=0.7): accumulated=1.4 ≥ 1.0 → finalise.
        // Trim: take 0.7 from insurer 1, then room=0.3 from insurer 2. Normalise: 0.7+0.3=1.0.
        let mut broker = broker_with_insurers(1, vec![1, 2]);
        broker.on_coverage_requested(Day(0), InsuredId(1), small_risk());

        let ev1 = broker.on_lead_quote_issued(
            Day(1), SubmissionId(0), InsuredId(1), InsurerId(1), 100, 0.7,
        );
        assert!(ev1.is_empty(), "still 1 outstanding → no QuotePresented yet");

        let ev2 = broker.on_lead_quote_issued(
            Day(1), SubmissionId(0), InsuredId(1), InsurerId(2), 100, 0.7,
        );
        assert_eq!(ev2.len(), 1);
        if let Event::QuotePresented { panel, .. } = &ev2[0].1 {
            let total: f64 = panel.iter().map(|(_, s)| s).sum();
            assert!((total - 1.0).abs() < 1e-9, "panel shares must sum to 1.0: {total}");
            assert_eq!(panel.len(), 2);
        } else {
            panic!("expected QuotePresented");
        }
    }

    #[test]
    fn panel_assembled_when_all_responded_undersubscribed() {
        // 2 insurers both offering line_size=0.4 → total=0.8 < 1.0; normalise to 0.5 each.
        let mut broker = broker_with_insurers(1, vec![1, 2]);
        broker.on_coverage_requested(Day(0), InsuredId(1), small_risk());

        let ev1 = broker.on_lead_quote_issued(
            Day(1), SubmissionId(0), InsuredId(1), InsurerId(1), 100, 0.4,
        );
        assert!(ev1.is_empty(), "still 1 outstanding → no QuotePresented yet");

        let ev2 = broker.on_lead_quote_issued(
            Day(1), SubmissionId(0), InsuredId(1), InsurerId(2), 100, 0.4,
        );
        assert_eq!(ev2.len(), 1);
        if let Event::QuotePresented { panel, .. } = &ev2[0].1 {
            let total: f64 = panel.iter().map(|(_, s)| s).sum();
            assert!((total - 1.0).abs() < 1e-9, "shares must sum to 1.0 after normalisation");
            for &(_, share) in panel {
                assert!((share - 0.5).abs() < 1e-9, "each share must be 0.5 after normalisation");
            }
        } else {
            panic!("expected QuotePresented");
        }
    }

    #[test]
    fn single_insurer_full_line_degenerates_to_old_behaviour() {
        // 1 insurer, line_size=1.0 → panel=[(InsurerId(1), 1.0)], premium passes through.
        let mut broker = broker_with_insurers(1, vec![1]);
        broker.on_coverage_requested(Day(0), InsuredId(1), small_risk());
        let events = broker.on_lead_quote_issued(
            Day(1), SubmissionId(0), InsuredId(1), InsurerId(1), 50_000, 1.0,
        );
        if let Event::QuotePresented { leader_id, panel, premium, .. } = &events[0].1 {
            assert_eq!(*leader_id, InsurerId(1));
            assert_eq!(panel.len(), 1);
            assert_eq!(panel[0].0, InsurerId(1));
            assert!((panel[0].1 - 1.0).abs() < 1e-9);
            assert_eq!(*premium, 50_000);
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
        let events = broker.on_lead_quote_declined(Day(1), SubmissionId(0), InsurerId(1));
        assert!(events.is_empty(), "still 1 outstanding → must return empty");
    }

    #[test]
    fn one_declines_one_issues_still_presents() {
        // 2 insurers. Insurer 1 declines, insurer 2 issues → QuotePresented with insurer 2.
        let mut broker = broker_with_insurers(1, vec![1, 2]);
        broker.on_coverage_requested(Day(0), InsuredId(1), small_risk());

        let ev_declined = broker.on_lead_quote_declined(Day(1), SubmissionId(0), InsurerId(1));
        assert!(ev_declined.is_empty(), "1 outstanding remaining → no event");

        let ev_issued = broker.on_lead_quote_issued(
            Day(1), SubmissionId(0), InsuredId(1), InsurerId(2), 50_000, 1.0,
        );
        assert_eq!(ev_issued.len(), 1);
        if let Event::QuotePresented { panel, premium, .. } = &ev_issued[0].1 {
            assert_eq!(panel.len(), 1);
            assert_eq!(panel[0].0, InsurerId(2));
            assert_eq!(*premium, 50_000);
        } else {
            panic!("expected QuotePresented");
        }
    }

    #[test]
    fn all_decline_drops_submission() {
        // 2 insurers, both decline → SubmissionDropped emitted (no QuotePresented).
        let mut broker = broker_with_insurers(1, vec![1, 2]);
        broker.on_coverage_requested(Day(0), InsuredId(1), small_risk());

        let ev1 = broker.on_lead_quote_declined(Day(1), SubmissionId(0), InsurerId(1));
        assert!(ev1.is_empty(), "still 1 outstanding → must return empty");

        let ev2 = broker.on_lead_quote_declined(Day(1), SubmissionId(0), InsurerId(2));
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
    fn low_decline_insurer_preferred_when_k_lt_n() {
        // 2 insurers, equal relationship scores (both 0.0).
        // Insurer 1 accumulates 5 declines; insurer 2 has none.
        // k=1 → every submission must route to insurer 2 (fewer declines).
        let mut broker = broker_with_qps(3, vec![1, 2], 1);
        // Decline counts increment even for unknown submission IDs (count is recorded first,
        // then the pending lookup fails and returns empty). Use this to directly inject
        // decline history for insurer 1 without running a full quoting flow.
        for i in 0..5u64 {
            let result = broker.on_lead_quote_declined(Day(0), SubmissionId(1000 + i), InsurerId(1));
            assert!(result.is_empty(), "unknown submission → no events");
        }
        // Insurer 1 now has decline_count=5, insurer 2 has 0.

        let events = broker.on_coverage_requested(Day(10), InsuredId(1), small_risk());
        assert_eq!(events.len(), 1);
        if let Event::LeadQuoteRequested { insurer_id, .. } = events[0].1 {
            assert_eq!(insurer_id, InsurerId(2), "low-decline insurer must be preferred");
        } else {
            panic!("expected LeadQuoteRequested");
        }
    }

    #[test]
    fn decline_counts_reset_at_year_end() {
        // After on_year_end, decline counts clear so the previously-penalised insurer
        // is no longer deprioritised (equal scores → round-robin resumes).
        let mut broker = broker_with_qps(1, vec![1, 2], 1);

        // Give insurer 1 a decline penalty.
        broker.on_coverage_requested(Day(0), InsuredId(1), small_risk());
        broker.on_lead_quote_declined(Day(1), SubmissionId(0), InsurerId(1));

        broker.on_year_end(); // resets decline_counts

        // With counts reset and equal scores, round-robin picks insurer 1 first again.
        let ev1 = broker.on_coverage_requested(Day(360), InsuredId(1), small_risk());
        let ev2 = broker.on_coverage_requested(Day(360), InsuredId(1), small_risk());
        let id1 = if let Event::LeadQuoteRequested { insurer_id, .. } = ev1[0].1 { insurer_id } else { panic!() };
        let id2 = if let Event::LeadQuoteRequested { insurer_id, .. } = ev2[0].1 { insurer_id } else { panic!() };
        // After reset, both insurers should appear (round-robin, not stuck on insurer 2).
        assert_ne!(id1, id2, "after year-end reset, round-robin must cycle both insurers");
    }
}
