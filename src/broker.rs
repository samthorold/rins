use std::collections::HashMap;

use crate::events::{Event, Risk};
use crate::insured::Insured;
use crate::types::{Day, InsuredId, InsurerId, SubmissionId};

/// Multiplicative decay applied to all relationship scores at each YearEnd.
/// A score of 1.0 halves in ~3.1 years (0.80^3.1 ≈ 0.50).
const SCORE_DECAY: f64 = 0.80;

/// Transient state while a submission is in flight.
struct PendingQuote {
    insured_id: InsuredId,
    /// The risk submitted, needed to emit FollowerQuoteRequested.
    risk: Risk,
    /// The highest-scored solicited insurer — sets terms when it issues.
    leader_id: InsurerId,
    /// Full score-sorted candidate list (up to k).
    candidates: Vec<InsurerId>,
    /// Index into `candidates` of the insurer currently acting as lead.
    lead_candidate_idx: usize,
    /// Lead's quoted premium — set once the lead issues; followers write at this rate.
    lead_premium: Option<u64>,
    /// Lead's actuarial technical price — carried for audit.
    lead_atp: Option<u64>,
    /// How many solicited followers have not yet responded.
    follower_outstanding: usize,
    /// Lines received so far: (insurer_id, premium, offered_line_size).
    panel_lines: Vec<(InsurerId, u64, f64)>,
    /// Sum of offered line sizes received so far.
    accumulated_line: f64,
}

/// Single broker that services all insureds.
/// Routes coverage requests to score-ranked insurers (incumbents get first look);
/// assembles a panel of fractional lines, normalised to sum to 1.0.
///
/// Quoting chain (lead-follow model):
/// 1. `on_coverage_requested` → emits exactly one `LeadQuoteRequested` to the top scorer.
/// 2. Lead issues → `on_lead_quote_issued` accumulates the lead's line, then emits
///    `FollowerQuoteRequested` for each remaining candidate.
/// 3. Lead declines → `on_lead_quote_declined` advances `lead_candidate_idx` and retries
///    the next candidate at the **same day** (preserving Inv 1).
/// 4. Followers respond via `on_follower_quote_issued` / `on_follower_quote_declined`.
/// 5. Panel finalises when accumulated_line ≥ 1.0 or all followers have responded.
pub struct Broker {
    pub insureds: Vec<Insured>,
    insurer_ids: Vec<InsurerId>,
    next_insurer_idx: usize,
    next_submission_id: u64,
    pending: HashMap<SubmissionId, PendingQuote>,
    /// Number of insurers solicited per submission (≥ 1, ≤ insurer_ids.len()).
    quotes_per_submission: usize,
    /// Accumulated relationship score per insurer. +1.0 per PolicyBound, ×0.80 per YearEnd.
    pub relationship_scores: HashMap<InsurerId, f64>,
    /// Count of declines received from each insurer since the last YearEnd.
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

    /// An insured has requested coverage. Solicits k insurers ordered by relationship score
    /// (descending); cyclic distance from `next_insurer_idx` breaks ties (round-robin fallback).
    ///
    /// Emits exactly **one** `LeadQuoteRequested` to the top scorer. The full k-length candidate
    /// list is stored so `on_lead_quote_declined` can retry the next candidate in order.
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

        let mut indices: Vec<usize> = (0..n).collect();
        let scores = &self.relationship_scores;
        let declines = &self.decline_counts;
        let insurer_ids = &self.insurer_ids;
        indices.sort_by(|&a, &b| {
            let net_a = scores.get(&insurer_ids[a]).copied().unwrap_or(0.0)
                - declines.get(&insurer_ids[a]).copied().unwrap_or(0.0);
            let net_b = scores.get(&insurer_ids[b]).copied().unwrap_or(0.0)
                - declines.get(&insurer_ids[b]).copied().unwrap_or(0.0);
            let net_ord = net_b.partial_cmp(&net_a).unwrap_or(std::cmp::Ordering::Equal);
            if net_ord != std::cmp::Ordering::Equal {
                return net_ord;
            }
            let da = (a + n - start_idx) % n;
            let db = (b + n - start_idx) % n;
            da.cmp(&db)
        });

        let submission_id = SubmissionId(self.next_submission_id);
        self.next_submission_id += 1;

        // Build the ordered candidate list (top k, score-sorted).
        let candidates: Vec<InsurerId> = indices[..k].iter().map(|&j| self.insurer_ids[j]).collect();
        let leader_id = candidates[0];

        self.pending.insert(
            submission_id,
            PendingQuote {
                insured_id,
                risk: risk.clone(),
                leader_id,
                candidates,
                lead_candidate_idx: 0,
                lead_premium: None,
                lead_atp: None,
                follower_outstanding: 0,
                panel_lines: vec![],
                accumulated_line: 0.0,
            },
        );

        // Emit exactly one LeadQuoteRequested for the top scorer.
        vec![(
            day.offset(1),
            Event::LeadQuoteRequested {
                submission_id,
                insured_id,
                insurer_id: leader_id,
                risk,
            },
        )]
    }

    /// Lead insurer has priced and issued a quote.
    ///
    /// 1. Store the lead's line and set `lead_premium` / `lead_atp`.
    /// 2. If `accumulated_line ≥ 1.0` → finalise immediately (lead filled the panel alone).
    /// 3. Collect remaining candidates as followers; if none → finalise.
    /// 4. Otherwise set `follower_outstanding` and emit `FollowerQuoteRequested` for each,
    ///    at the **same day** as `LeadQuoteIssued` (D+1).
    pub fn on_lead_quote_issued(
        &mut self,
        day: Day,
        submission_id: SubmissionId,
        _insured_id: InsuredId,
        insurer_id: InsurerId,
        atp: u64,
        premium: u64,
        line_size: f64,
    ) -> Vec<(Day, Event)> {
        let pq = match self.pending.get_mut(&submission_id) {
            Some(pq) => pq,
            None => return vec![],
        };

        pq.panel_lines.push((insurer_id, premium, line_size));
        pq.accumulated_line += line_size;
        pq.lead_premium = Some(premium);
        pq.lead_atp = Some(atp);

        if pq.accumulated_line >= 1.0 {
            let pq = self.pending.remove(&submission_id).unwrap();
            return self.finalise_panel(day, submission_id, pq);
        }

        // Collect followers: remaining candidates after the current lead.
        let follower_start = pq.lead_candidate_idx + 1;
        let follower_ids: Vec<InsurerId> = pq.candidates[follower_start..].to_vec();

        if follower_ids.is_empty() {
            let pq = self.pending.remove(&submission_id).unwrap();
            return self.finalise_panel(day, submission_id, pq);
        }

        let insured_id = pq.insured_id;
        let risk = pq.risk.clone();
        let lead_premium = premium;
        let lead_atp = atp;
        pq.follower_outstanding = follower_ids.len();

        follower_ids
            .into_iter()
            .map(|follower_id| {
                (
                    day,
                    Event::FollowerQuoteRequested {
                        submission_id,
                        insured_id,
                        insurer_id: follower_id,
                        risk: risk.clone(),
                        lead_premium,
                        lead_atp,
                    },
                )
            })
            .collect()
    }

    /// Lead insurer declined. Retry with the next scored candidate as lead (same day),
    /// or emit `SubmissionDropped` if all candidates are exhausted.
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

        pq.lead_candidate_idx += 1;

        if pq.lead_candidate_idx >= pq.candidates.len() {
            // All candidates exhausted.
            let pq = self.pending.remove(&submission_id).unwrap();
            return vec![(
                day,
                Event::SubmissionDropped { submission_id, insured_id: pq.insured_id },
            )];
        }

        // Retry with the next candidate as lead — same day to preserve Inv 1.
        let next_lead = pq.candidates[pq.lead_candidate_idx];
        pq.leader_id = next_lead;
        let insured_id = pq.insured_id;
        let risk = pq.risk.clone();

        vec![(
            day,
            Event::LeadQuoteRequested {
                submission_id,
                insured_id,
                insurer_id: next_lead,
                risk,
            },
        )]
    }

    /// A follower insurer agreed to participate at the lead's rate.
    /// Finalises the panel if accumulated_line ≥ 1.0 or all followers have responded.
    pub fn on_follower_quote_issued(
        &mut self,
        day: Day,
        submission_id: SubmissionId,
        insurer_id: InsurerId,
        line_size: f64,
    ) -> Vec<(Day, Event)> {
        let pq = match self.pending.get_mut(&submission_id) {
            Some(pq) => pq,
            None => return vec![],
        };

        let lead_premium = pq.lead_premium.unwrap_or(0);
        pq.panel_lines.push((insurer_id, lead_premium, line_size));
        pq.accumulated_line += line_size;
        pq.follower_outstanding = pq.follower_outstanding.saturating_sub(1);

        if pq.accumulated_line >= 1.0 || pq.follower_outstanding == 0 {
            let pq = self.pending.remove(&submission_id).unwrap();
            self.finalise_panel(day, submission_id, pq)
        } else {
            vec![]
        }
    }

    /// A follower insurer declined participation.
    /// Finalises the panel when all followers have responded.
    pub fn on_follower_quote_declined(
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

        pq.follower_outstanding = pq.follower_outstanding.saturating_sub(1);

        if pq.follower_outstanding == 0 {
            let pq = self.pending.remove(&submission_id).unwrap();
            self.finalise_panel(day, submission_id, pq)
        } else {
            vec![]
        }
    }

    /// Trim panel lines to fill exactly 1.0, scale to normalise, then emit
    /// `QuotePresented` with blended premium — or `SubmissionDropped` if no lines.
    ///
    /// Because all follower lines carry `lead_premium`, the blended premium equals
    /// `lead_premium` regardless of panel composition.
    fn finalise_panel(
        &self,
        day: Day,
        submission_id: SubmissionId,
        pq: PendingQuote,
    ) -> Vec<(Day, Event)> {
        if pq.panel_lines.is_empty() || pq.accumulated_line == 0.0 {
            return vec![(day.offset(1), Event::SubmissionDropped { submission_id, insured_id: pq.insured_id })];
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

        // Normalise so shares sum to exactly 1.0.
        let actual_total: f64 = included.iter().map(|&(_, _, l)| l).sum();
        let panel: Vec<(InsurerId, f64)> = included.iter()
            .map(|&(id, _, l)| (id, l / actual_total))
            .collect();

        // Blended premium = Σ share_i × premium_i.
        // Since all entries carry lead_premium, this equals lead_premium.
        let blended_premium = included.iter()
            .map(|&(_, prem, l)| prem as f64 * l / actual_total)
            .sum::<f64>()
            .round() as u64;

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

    fn broker_with_insurers(n: usize, insurer_ids: Vec<u64>) -> Broker {
        let qps = insurer_ids.len().max(1);
        let insureds = (1..=n as u64).map(|i| make_insured(i)).collect();
        let insurer_ids = insurer_ids.into_iter().map(InsurerId).collect();
        Broker::new(insureds, insurer_ids, qps)
    }

    fn broker_with_qps(n: usize, insurer_ids: Vec<u64>, qps: usize) -> Broker {
        let insureds = (1..=n as u64).map(|i| make_insured(i)).collect();
        let insurer_ids = insurer_ids.into_iter().map(InsurerId).collect();
        Broker::new(insureds, insurer_ids, qps)
    }

    // ── on_coverage_requested ─────────────────────────────────────────────────

    #[test]
    fn on_coverage_requested_emits_exactly_one_lead_quote_requested() {
        // 2 insurers, qps=2 → exactly 1 LeadQuoteRequested (to top scorer only).
        let mut broker = broker_with_insurers(1, vec![1, 2]);
        let events = broker.on_coverage_requested(Day(0), InsuredId(1), small_risk());
        assert_eq!(events.len(), 1);
        assert!(matches!(events[0].1, Event::LeadQuoteRequested { .. }));
    }

    #[test]
    fn on_coverage_requested_routes_to_highest_scorer() {
        // qps=2: top scorer (ins1 with score 5.0) always gets the lead request.
        let mut broker = broker_with_qps(3, vec![1, 2, 3], 2);
        for _ in 0..5 {
            broker.on_policy_bound(InsurerId(1));
        }
        for id in 1..=3u64 {
            let events = broker.on_coverage_requested(Day(0), InsuredId(id), small_risk());
            assert_eq!(events.len(), 1);
            if let Event::LeadQuoteRequested { insurer_id, .. } = events[0].1 {
                assert_eq!(insurer_id, InsurerId(1), "high-score insurer must be the lead");
            } else {
                panic!("expected LeadQuoteRequested");
            }
        }
    }

    #[test]
    fn on_coverage_requested_single_insurer_still_works() {
        let mut broker = broker_with_insurers(1, vec![7]);
        let events = broker.on_coverage_requested(Day(0), InsuredId(1), small_risk());
        assert_eq!(events.len(), 1);
        if let Event::LeadQuoteRequested { insurer_id, .. } = events[0].1 {
            assert_eq!(insurer_id, InsurerId(7));
        } else {
            panic!("expected LeadQuoteRequested");
        }
    }

    #[test]
    fn on_coverage_requested_still_increments_submission_id() {
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
        if let Event::LeadQuoteRequested { submission_id, insured_id, insurer_id, risk: ev_risk } =
            &events[0].1
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

    // ── on_lead_quote_issued ──────────────────────────────────────────────────

    #[test]
    fn on_lead_quote_issued_returns_quote_presented_when_single_candidate() {
        // 1 insurer: no followers, panel finalises immediately after lead issues.
        let mut broker = broker_with_insurers(1, vec![1]);
        broker.on_coverage_requested(Day(0), InsuredId(1), small_risk());
        let events = broker.on_lead_quote_issued(
            Day(1), SubmissionId(0), InsuredId(1), InsurerId(1), 50_000, 50_000, 1.0,
        );
        assert_eq!(events.len(), 1);
        assert!(matches!(events[0].1, Event::QuotePresented { .. }));
    }

    #[test]
    fn on_lead_quote_issued_scheduled_day_plus_one() {
        let mut broker = broker_with_insurers(1, vec![1]);
        broker.on_coverage_requested(Day(0), InsuredId(1), small_risk());
        let events = broker.on_lead_quote_issued(
            Day(1), SubmissionId(0), InsuredId(1), InsurerId(1), 50_000, 50_000, 1.0,
        );
        assert_eq!(events[0].0, Day(2), "QuotePresented must fire at day+1 from LeadQuoteIssued");
    }

    #[test]
    fn on_lead_quote_issued_carries_correct_fields() {
        let mut broker = broker_with_insurers(1, vec![5]);
        broker.on_coverage_requested(Day(0), InsuredId(10), small_risk());
        let events = broker.on_lead_quote_issued(
            Day(1), SubmissionId(0), InsuredId(10), InsurerId(5), 99_000, 99_000, 1.0,
        );
        if let Event::QuotePresented { submission_id, insured_id, leader_id, panel, premium } =
            &events[0].1
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
            Day(1), SubmissionId(999), InsuredId(1), InsurerId(1), 50_000, 50_000, 1.0,
        );
        assert!(events.is_empty(), "unknown submission_id must produce no events");
    }

    #[test]
    fn on_lead_quote_issued_emits_follower_requests_to_remaining_candidates() {
        // 3 insurers qps=3: lead=ins1, followers=[ins2, ins3].
        let mut broker = broker_with_insurers(1, vec![1, 2, 3]);
        broker.on_coverage_requested(Day(0), InsuredId(1), small_risk());
        let events = broker.on_lead_quote_issued(
            Day(1), SubmissionId(0), InsuredId(1), InsurerId(1), 100, 100, 0.3,
        );
        // Should emit 2 FollowerQuoteRequested events (for ins2 and ins3).
        assert_eq!(events.len(), 2, "expected 2 FollowerQuoteRequested");
        assert!(events.iter().all(|(_, e)| matches!(e, Event::FollowerQuoteRequested { .. })));
        let follower_ids: Vec<u64> = events
            .iter()
            .filter_map(|(_, e)| {
                if let Event::FollowerQuoteRequested { insurer_id, .. } = e {
                    Some(insurer_id.0)
                } else {
                    None
                }
            })
            .collect();
        assert!(follower_ids.contains(&2));
        assert!(follower_ids.contains(&3));
    }

    #[test]
    fn on_lead_quote_issued_carries_lead_premium_in_follower_requests() {
        let mut broker = broker_with_insurers(1, vec![1, 2]);
        broker.on_coverage_requested(Day(0), InsuredId(1), small_risk());
        let events = broker.on_lead_quote_issued(
            Day(1), SubmissionId(0), InsuredId(1), InsurerId(1), 77_777, 80_000, 0.4,
        );
        if let Event::FollowerQuoteRequested { lead_premium, lead_atp, .. } = &events[0].1 {
            assert_eq!(*lead_premium, 80_000, "follower must receive lead's premium");
            assert_eq!(*lead_atp, 77_777, "follower must receive lead's atp");
        } else {
            panic!("expected FollowerQuoteRequested");
        }
    }

    #[test]
    fn on_lead_quote_issued_finalises_immediately_when_lead_fills_panel() {
        // 2 insurers but lead writes line_size=1.0 → panel full, no followers needed.
        let mut broker = broker_with_insurers(1, vec![1, 2]);
        broker.on_coverage_requested(Day(0), InsuredId(1), small_risk());
        let events = broker.on_lead_quote_issued(
            Day(1), SubmissionId(0), InsuredId(1), InsurerId(1), 50_000, 50_000, 1.0,
        );
        assert_eq!(events.len(), 1);
        assert!(matches!(events[0].1, Event::QuotePresented { .. }));
    }

    #[test]
    fn on_lead_quote_issued_finalises_when_no_followers_available() {
        // qps=1: only 1 candidate → no followers available after lead issues.
        let mut broker = broker_with_qps(1, vec![1, 2, 3], 1);
        broker.on_coverage_requested(Day(0), InsuredId(1), small_risk());
        let events = broker.on_lead_quote_issued(
            Day(1), SubmissionId(0), InsuredId(1), InsurerId(1), 50_000, 50_000, 0.5,
        );
        // Undersubscribed (0.5) but no followers → finalise with partial panel.
        assert_eq!(events.len(), 1);
        assert!(matches!(events[0].1, Event::QuotePresented { .. }));
    }

    #[test]
    fn on_lead_quote_issued_premium_equals_lead_premium_not_blended() {
        // 2 insurers: lead issues premium=100k, follower responds.
        // Final QuotePresented.premium should equal 100k (all at lead rate).
        let mut broker = broker_with_insurers(1, vec![1, 2]);
        broker.on_coverage_requested(Day(0), InsuredId(1), small_risk());
        broker.on_lead_quote_issued(
            Day(1), SubmissionId(0), InsuredId(1), InsurerId(1), 100_000, 100_000, 0.4,
        );
        let events = broker.on_follower_quote_issued(Day(1), SubmissionId(0), InsurerId(2), 0.6);
        if let Event::QuotePresented { premium, .. } = &events[0].1 {
            assert_eq!(*premium, 100_000, "blended premium must equal lead premium");
        } else {
            panic!("expected QuotePresented");
        }
    }

    // ── on_lead_quote_declined ────────────────────────────────────────────────

    #[test]
    fn lead_decline_retries_next_candidate_as_lead() {
        // 2 insurers qps=2: ins1 declines as lead → ins2 becomes lead.
        let mut broker = broker_with_insurers(1, vec![1, 2]);
        broker.on_coverage_requested(Day(0), InsuredId(1), small_risk());
        let events = broker.on_lead_quote_declined(Day(1), SubmissionId(0), InsurerId(1));
        assert_eq!(events.len(), 1, "lead decline must emit LeadQuoteRequested for next candidate");
        if let Event::LeadQuoteRequested { insurer_id, .. } = events[0].1 {
            assert_eq!(insurer_id, InsurerId(2), "next candidate must become lead");
        } else {
            panic!("expected LeadQuoteRequested");
        }
    }

    #[test]
    fn lead_decline_all_candidates_exhausted_emits_submission_dropped() {
        // 2 insurers both decline as lead → SubmissionDropped.
        let mut broker = broker_with_insurers(1, vec![1, 2]);
        broker.on_coverage_requested(Day(0), InsuredId(1), small_risk());
        broker.on_lead_quote_declined(Day(1), SubmissionId(0), InsurerId(1));
        let events = broker.on_lead_quote_declined(Day(1), SubmissionId(0), InsurerId(2));
        assert_eq!(events.len(), 1);
        assert!(
            matches!(events[0].1, Event::SubmissionDropped { insured_id: InsuredId(1), .. }),
            "expected SubmissionDropped, got {:?}", events[0].1
        );
    }

    #[test]
    fn lead_decline_retry_emits_same_day_as_decline() {
        // Retry lead request must be at the same day as the decline (preserves Inv 1).
        let mut broker = broker_with_insurers(1, vec![1, 2]);
        broker.on_coverage_requested(Day(0), InsuredId(1), small_risk());
        let events = broker.on_lead_quote_declined(Day(5), SubmissionId(0), InsurerId(1));
        assert_eq!(events[0].0, Day(5), "retry LeadQuoteRequested must be same day as decline");
    }

    // Old tests updated for new semantics:

    #[test]
    fn lead_decline_with_single_candidate_drops_submission() {
        // qps=1: only 1 candidate; if it declines → SubmissionDropped.
        let mut broker = broker_with_qps(1, vec![1, 2], 1);
        broker.on_coverage_requested(Day(0), InsuredId(1), small_risk());
        let events = broker.on_lead_quote_declined(Day(1), SubmissionId(0), InsurerId(1));
        assert_eq!(events.len(), 1);
        assert!(matches!(events[0].1, Event::SubmissionDropped { .. }));
    }

    #[test]
    fn second_lead_fills_panel_after_first_declines() {
        // 2 insurers qps=2: ins1 declines as lead → ins2 becomes lead, issues → QuotePresented.
        let mut broker = broker_with_insurers(1, vec![1, 2]);
        broker.on_coverage_requested(Day(0), InsuredId(1), small_risk());

        let ev_retry = broker.on_lead_quote_declined(Day(1), SubmissionId(0), InsurerId(1));
        assert!(matches!(ev_retry[0].1, Event::LeadQuoteRequested { insurer_id: InsurerId(2), .. }));

        // ins2 is now lead; no more followers (ins1 already declined as lead, not in remainder)
        let ev_issued = broker.on_lead_quote_issued(
            Day(1), SubmissionId(0), InsuredId(1), InsurerId(2), 50_000, 50_000, 1.0,
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

    // ── on_follower_quote_issued + on_follower_quote_declined ─────────────────

    #[test]
    fn on_follower_quote_issued_accumulates_line() {
        // Lead takes 0.4; follower issues 0.3 → not full, still 0 followers outstanding.
        // With 2 insurers the follower list has 1 entry; after response outstanding=0 → finalise.
        let mut broker = broker_with_insurers(1, vec![1, 2]);
        broker.on_coverage_requested(Day(0), InsuredId(1), small_risk());
        broker.on_lead_quote_issued(
            Day(1), SubmissionId(0), InsuredId(1), InsurerId(1), 100, 100, 0.4,
        );
        let events = broker.on_follower_quote_issued(Day(1), SubmissionId(0), InsurerId(2), 0.3);
        // Only 1 follower outstanding → finalises after response; 0.4+0.3=0.7 → undersubscribed.
        assert_eq!(events.len(), 1);
        assert!(matches!(events[0].1, Event::QuotePresented { .. }));
    }

    #[test]
    fn on_follower_quote_issued_finalises_when_panel_full() {
        // Lead 0.4 + follower 0.7 = 1.1 ≥ 1.0 → finalise after follower issues.
        let mut broker = broker_with_insurers(1, vec![1, 2]);
        broker.on_coverage_requested(Day(0), InsuredId(1), small_risk());
        broker.on_lead_quote_issued(
            Day(1), SubmissionId(0), InsuredId(1), InsurerId(1), 100, 100, 0.4,
        );
        let events = broker.on_follower_quote_issued(Day(1), SubmissionId(0), InsurerId(2), 0.7);
        assert_eq!(events.len(), 1);
        if let Event::QuotePresented { panel, .. } = &events[0].1 {
            let total: f64 = panel.iter().map(|(_, s)| s).sum();
            assert!((total - 1.0).abs() < 1e-9, "panel shares must sum to 1.0: {total}");
        } else {
            panic!("expected QuotePresented");
        }
    }

    #[test]
    fn on_follower_quote_issued_premium_equals_lead_premium() {
        // Follower writes at lead_premium (not its own pricing).
        let mut broker = broker_with_insurers(1, vec![1, 2]);
        broker.on_coverage_requested(Day(0), InsuredId(1), small_risk());
        broker.on_lead_quote_issued(
            Day(1), SubmissionId(0), InsuredId(1), InsurerId(1), 123_456, 123_456, 0.4,
        );
        let events = broker.on_follower_quote_issued(Day(1), SubmissionId(0), InsurerId(2), 0.6);
        if let Event::QuotePresented { premium, .. } = &events[0].1 {
            assert_eq!(*premium, 123_456, "QuotePresented.premium must equal lead_premium");
        } else {
            panic!("expected QuotePresented");
        }
    }

    #[test]
    fn on_follower_quote_declined_decrements_outstanding() {
        // 3 insurers: lead=ins1, followers=[ins2, ins3].
        // ins2 declines → outstanding goes from 2 to 1 → no finalise yet.
        let mut broker = broker_with_insurers(1, vec![1, 2, 3]);
        broker.on_coverage_requested(Day(0), InsuredId(1), small_risk());
        broker.on_lead_quote_issued(
            Day(1), SubmissionId(0), InsuredId(1), InsurerId(1), 100, 100, 0.3,
        );
        let events = broker.on_follower_quote_declined(Day(1), SubmissionId(0), InsurerId(2));
        assert!(events.is_empty(), "still 1 follower outstanding → no finalise");
    }

    #[test]
    fn on_follower_quote_declined_all_followers_decline_finalises_partial_panel() {
        // 3 insurers: lead=ins1 takes 0.4; both followers decline → finalise with leader only.
        let mut broker = broker_with_insurers(1, vec![1, 2, 3]);
        broker.on_coverage_requested(Day(0), InsuredId(1), small_risk());
        broker.on_lead_quote_issued(
            Day(1), SubmissionId(0), InsuredId(1), InsurerId(1), 100, 100, 0.4,
        );
        broker.on_follower_quote_declined(Day(1), SubmissionId(0), InsurerId(2));
        let events = broker.on_follower_quote_declined(Day(1), SubmissionId(0), InsurerId(3));
        assert_eq!(events.len(), 1, "all followers declined → finalise with partial panel");
        if let Event::QuotePresented { panel, .. } = &events[0].1 {
            assert_eq!(panel.len(), 1, "only leader in panel");
            assert_eq!(panel[0].0, InsurerId(1));
        } else {
            panic!("expected QuotePresented");
        }
    }

    #[test]
    fn on_follower_quote_declined_unknown_submission_returns_empty() {
        let mut broker = broker_with_insurers(1, vec![1]);
        let events = broker.on_follower_quote_declined(Day(1), SubmissionId(999), InsurerId(1));
        assert!(events.is_empty());
    }

    // ── Panel assembly ────────────────────────────────────────────────────────

    #[test]
    fn panel_fills_from_two_insurers_with_large_lines() {
        // Lead=ins1 (line=0.7), follower=ins2 (line=0.7) → accumulated=1.4 ≥ 1.0 → finalise.
        // Trim: take 0.7 from ins1, then 0.3 from ins2. Normalise: total=1.0.
        let mut broker = broker_with_insurers(1, vec![1, 2]);
        broker.on_coverage_requested(Day(0), InsuredId(1), small_risk());

        let ev1 = broker.on_lead_quote_issued(
            Day(1), SubmissionId(0), InsuredId(1), InsurerId(1), 100, 100, 0.7,
        );
        assert_eq!(ev1.len(), 1);
        assert!(matches!(ev1[0].1, Event::FollowerQuoteRequested { insurer_id: InsurerId(2), .. }));

        let ev2 = broker.on_follower_quote_issued(Day(1), SubmissionId(0), InsurerId(2), 0.7);
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
        // Lead=ins1 (line=0.4), follower=ins2 (line=0.4) → total=0.8 < 1.0; normalise to 0.5 each.
        let mut broker = broker_with_insurers(1, vec![1, 2]);
        broker.on_coverage_requested(Day(0), InsuredId(1), small_risk());

        let ev1 = broker.on_lead_quote_issued(
            Day(1), SubmissionId(0), InsuredId(1), InsurerId(1), 100, 100, 0.4,
        );
        assert!(matches!(ev1[0].1, Event::FollowerQuoteRequested { .. }));

        let ev2 = broker.on_follower_quote_issued(Day(1), SubmissionId(0), InsurerId(2), 0.4);
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
            Day(1), SubmissionId(0), InsuredId(1), InsurerId(1), 50_000, 50_000, 1.0,
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
        let mut broker = broker_with_qps(3, vec![1, 2], 1);
        for i in 0..5u64 {
            let result = broker.on_lead_quote_declined(Day(0), SubmissionId(1000 + i), InsurerId(1));
            assert!(result.is_empty(), "unknown submission → no events");
        }
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
        let mut broker = broker_with_qps(1, vec![1, 2], 1);
        broker.on_coverage_requested(Day(0), InsuredId(1), small_risk());
        // With qps=1, only 1 candidate → decline exhausts candidates → SubmissionDropped.
        broker.on_lead_quote_declined(Day(1), SubmissionId(0), InsurerId(1));
        broker.on_year_end();
        let ev1 = broker.on_coverage_requested(Day(360), InsuredId(1), small_risk());
        let ev2 = broker.on_coverage_requested(Day(360), InsuredId(1), small_risk());
        let id1 = if let Event::LeadQuoteRequested { insurer_id, .. } = ev1[0].1 { insurer_id } else { panic!() };
        let id2 = if let Event::LeadQuoteRequested { insurer_id, .. } = ev2[0].1 { insurer_id } else { panic!() };
        assert_ne!(id1, id2, "after year-end reset, round-robin must cycle both insurers");
    }
}
