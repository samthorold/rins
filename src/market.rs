use std::collections::HashMap;

use crate::events::{Event, Panel, PanelEntry, Peril, Risk};
use crate::syndicate::Syndicate;
use crate::types::{Day, PolicyId, SubmissionId, SyndicateId, Year};

/// Industry-wide statistics published at year-end.
/// Syndicates read these when pricing for the next year.
pub struct YearStats {
    pub year: Year,
    pub industry_loss_ratio: f64,
    pub active_syndicate_count: usize,
    /// Realised loss ratio per line of business for the completed year.
    /// Passed into each Syndicate::on_year_end so EWMA stays current.
    pub loss_ratios_by_line: HashMap<String, f64>,
}

/// Transient state for a submission while it is in the quoting pipeline.
struct PendingSubmission {
    submission_id: SubmissionId,
    broker_id: crate::types::BrokerId,
    risk: Risk,
    arrived_day: Day,
    lead_premium: Option<u64>,
    followers_invited: usize,
    followers_responded: usize,
    /// (syndicate_id, premium) — shares allocated at panel assembly.
    quoted_syndicates: Vec<(SyndicateId, u64)>,
    declined_count: usize,
}

/// A successfully bound policy; used to route losses.
pub struct BoundPolicy {
    pub policy_id: PolicyId,
    pub submission_id: SubmissionId,
    pub risk: Risk,
    pub panel: Panel,
}

pub struct Market {
    next_policy_id: u64,
    pending: HashMap<SubmissionId, PendingSubmission>,
    pub policies: HashMap<PolicyId, BoundPolicy>,
    /// Risk stashed at panel-assembly time, consumed when `PolicyBound` fires.
    risk_cache: HashMap<SubmissionId, Risk>,
    /// Year-to-date gross premiums written per line of business.
    ytd_premiums_by_line: HashMap<String, u64>,
    /// Year-to-date claims settled per line of business.
    ytd_claims_by_line: HashMap<String, u64>,
}

impl Market {
    pub fn new() -> Self {
        Market {
            next_policy_id: 0,
            pending: HashMap::new(),
            policies: HashMap::new(),
            risk_cache: HashMap::new(),
            ytd_premiums_by_line: HashMap::new(),
            ytd_claims_by_line: HashMap::new(),
        }
    }

    /// Compute industry statistics from the year-to-date accumulators.
    /// Resets YTD accumulators for the next year.
    /// Returns an owned value — caller can then mutably borrow agents.
    pub fn compute_year_stats(&mut self, syndicates: &[Syndicate], year: Year) -> YearStats {
        let total_premiums: u64 = self.ytd_premiums_by_line.values().sum();
        let total_claims: u64 = self.ytd_claims_by_line.values().sum();

        let industry_loss_ratio = if total_premiums > 0 {
            total_claims as f64 / total_premiums as f64
        } else {
            // No policies written this year; fall back to a plausible prior.
            0.65
        };

        let mut loss_ratios_by_line: HashMap<String, f64> = HashMap::new();
        for (line, &premiums) in &self.ytd_premiums_by_line {
            if premiums > 0 {
                let claims = self.ytd_claims_by_line.get(line).copied().unwrap_or(0);
                loss_ratios_by_line.insert(line.clone(), claims as f64 / premiums as f64);
            }
        }

        // Reset accumulators ready for next year.
        self.ytd_premiums_by_line.clear();
        self.ytd_claims_by_line.clear();

        YearStats {
            year,
            industry_loss_ratio,
            active_syndicate_count: syndicates.len(),
            loss_ratios_by_line,
        }
    }

    /// Record that a claim has been settled against a policy.
    /// Accumulates into the YTD claims total for the policy's line of business.
    pub fn on_claim_settled(&mut self, policy_id: PolicyId, amount: u64) {
        if let Some(policy) = self.policies.get(&policy_id) {
            *self
                .ytd_claims_by_line
                .entry(policy.risk.line_of_business.clone())
                .or_insert(0) += amount;
        }
    }

    /// Return a clone of the risk and the current lead premium for the given
    /// submission. Called by the dispatch layer before invoking Syndicate::on_quote_requested.
    pub fn quote_request_params(
        &self,
        submission_id: SubmissionId,
        _is_lead: bool,
    ) -> Option<(Risk, Option<u64>)> {
        self.pending
            .get(&submission_id)
            .map(|p| (p.risk.clone(), p.lead_premium))
    }

    /// A new submission has arrived. Store it and ask the lead syndicate to quote.
    /// MVP: lead = first syndicate in `available_syndicates`.
    pub fn on_submission_arrived(
        &mut self,
        day: Day,
        submission_id: SubmissionId,
        broker_id: crate::types::BrokerId,
        risk: Risk,
        available_syndicates: &[SyndicateId],
    ) -> Vec<(Day, Event)> {
        if available_syndicates.is_empty() {
            return vec![];
        }
        let lead_id = available_syndicates[0];
        self.pending.insert(
            submission_id,
            PendingSubmission {
                submission_id,
                broker_id,
                risk,
                arrived_day: day,
                lead_premium: None,
                followers_invited: 0,
                followers_responded: 0,
                quoted_syndicates: vec![],
                declined_count: 0,
            },
        );
        vec![(
            day,
            Event::QuoteRequested {
                submission_id,
                syndicate_id: lead_id,
                is_lead: true,
            },
        )]
    }

    /// The lead syndicate has issued a quote. Record it and invite followers.
    pub fn on_lead_quote_issued(
        &mut self,
        day: Day,
        submission_id: SubmissionId,
        syndicate_id: SyndicateId,
        premium: u64,
        available_syndicates: &[SyndicateId],
    ) -> Vec<(Day, Event)> {
        let pending = match self.pending.get_mut(&submission_id) {
            Some(p) => p,
            None => return vec![],
        };
        pending.lead_premium = Some(premium);
        pending.quoted_syndicates.push((syndicate_id, premium));

        // Followers are all syndicates except the lead.
        let followers: Vec<SyndicateId> = available_syndicates
            .iter()
            .copied()
            .filter(|&id| id != syndicate_id)
            .collect();

        if followers.is_empty() {
            // Single-syndicate market: assemble immediately.
            return self.assemble_panel(day, submission_id);
        }

        pending.followers_invited = followers.len();
        followers
            .into_iter()
            .map(|follower_id| {
                (
                    day.offset(1),
                    Event::QuoteRequested {
                        submission_id,
                        syndicate_id: follower_id,
                        is_lead: false,
                    },
                )
            })
            .collect()
    }

    /// A follower syndicate has issued a quote.
    pub fn on_follower_quote_issued(
        &mut self,
        day: Day,
        submission_id: SubmissionId,
        syndicate_id: SyndicateId,
        premium: u64,
    ) -> Vec<(Day, Event)> {
        let pending = match self.pending.get_mut(&submission_id) {
            Some(p) => p,
            None => return vec![],
        };
        pending.quoted_syndicates.push((syndicate_id, premium));
        pending.followers_responded += 1;
        if pending.followers_responded == pending.followers_invited {
            self.assemble_panel(day, submission_id)
        } else {
            vec![]
        }
    }

    /// A syndicate has declined to quote.
    pub fn on_quote_declined(
        &mut self,
        day: Day,
        submission_id: SubmissionId,
    ) -> Vec<(Day, Event)> {
        let pending = match self.pending.get_mut(&submission_id) {
            Some(p) => p,
            None => return vec![],
        };
        pending.declined_count += 1;
        pending.followers_responded += 1;
        if pending.followers_responded == pending.followers_invited {
            self.assemble_panel(day, submission_id)
        } else {
            vec![]
        }
    }

    /// Consume the stashed risk for a submission after `PolicyBound` fires.
    pub fn take_bound_risk(&mut self, submission_id: SubmissionId) -> Option<Risk> {
        self.risk_cache.remove(&submission_id)
    }

    /// Assemble the panel from all syndicates that quoted, with equal shares.
    /// Removes the submission from `pending` and returns a `PolicyBound` event.
    fn assemble_panel(&mut self, day: Day, submission_id: SubmissionId) -> Vec<(Day, Event)> {
        let pending = match self.pending.remove(&submission_id) {
            Some(p) => p,
            None => return vec![],
        };
        if pending.quoted_syndicates.is_empty() {
            // No quotes — submission is abandoned silently.
            return vec![];
        }
        // Stash the risk so the dispatch layer can pass it to on_policy_bound.
        self.risk_cache.insert(submission_id, pending.risk.clone());

        let n = pending.quoted_syndicates.len() as u32;
        let base_share = 10_000 / n;
        let remainder = 10_000 % n;

        let entries: Vec<PanelEntry> = pending
            .quoted_syndicates
            .iter()
            .enumerate()
            .map(|(i, &(syn_id, prem))| {
                let share_bps = base_share + if i == 0 { remainder } else { 0 };
                PanelEntry {
                    syndicate_id: syn_id,
                    share_bps,
                    premium: prem * share_bps as u64 / 10_000,
                }
            })
            .collect();

        vec![(
            day.offset(2),
            Event::PolicyBound {
                submission_id,
                panel: Panel { entries },
            },
        )]
    }

    /// Register a newly bound policy. Called when `PolicyBound` fires.
    /// Accumulates gross premium into the YTD counter for the policy's line.
    pub fn on_policy_bound(&mut self, submission_id: SubmissionId, risk: Risk, panel: Panel) {
        let total_premium: u64 = panel.entries.iter().map(|e| e.premium).sum();
        *self
            .ytd_premiums_by_line
            .entry(risk.line_of_business.clone())
            .or_insert(0) += total_premium;

        let policy_id = PolicyId(self.next_policy_id);
        self.next_policy_id += 1;
        self.policies.insert(
            policy_id,
            BoundPolicy {
                policy_id,
                submission_id,
                risk,
                panel,
            },
        );
    }

    /// Distribute a loss event across all matching bound policies.
    /// MVP: exposure_fraction = 1 (each policy treated as fully exposed).
    /// TODO (§7): divide total severity proportionally by sum_insured across all exposed policies.
    pub fn on_loss_event(
        &self,
        day: Day,
        region: &str,
        peril: Peril,
        severity: u64,
    ) -> Vec<(Day, Event)> {
        let mut out = vec![];
        for (policy_id, policy) in &self.policies {
            if policy.risk.territory != region {
                continue;
            }
            if !policy.risk.perils_covered.contains(&peril) {
                continue;
            }
            let gross_loss = severity.min(policy.risk.limit);
            let net_loss = gross_loss.saturating_sub(policy.risk.attachment);
            if net_loss == 0 {
                continue;
            }
            for entry in &policy.panel.entries {
                let syndicate_loss = net_loss * entry.share_bps as u64 / 10_000;
                if syndicate_loss == 0 {
                    continue;
                }
                out.push((
                    day,
                    Event::ClaimSettled {
                        policy_id: *policy_id,
                        syndicate_id: entry.syndicate_id,
                        amount: syndicate_loss,
                    },
                ));
            }
        }
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::events::Peril;
    use crate::types::SyndicateId;

    fn make_risk() -> Risk {
        Risk {
            line_of_business: "property".to_string(),
            sum_insured: 2_000_000,
            territory: "US-SE".to_string(),
            limit: 1_000_000,
            attachment: 100_000,
            perils_covered: vec![Peril::WindstormAtlantic],
        }
    }

    #[test]
    fn per_syndicate_loss_allocation_matches_share() {
        use crate::types::{PolicyId, SubmissionId};

        let risk = Risk {
            line_of_business: "property".to_string(),
            sum_insured: 2_000_000,
            territory: "US-SE".to_string(),
            limit: 1_000_000,
            attachment: 100_000,
            perils_covered: vec![Peril::WindstormAtlantic],
        };
        let panel = Panel {
            entries: vec![
                PanelEntry { syndicate_id: SyndicateId(1), share_bps: 6_000, premium: 0 },
                PanelEntry { syndicate_id: SyndicateId(2), share_bps: 4_000, premium: 0 },
            ],
        };

        let mut market = Market::new();
        let policy_id = PolicyId(0);
        market.policies.insert(
            policy_id,
            BoundPolicy { policy_id, submission_id: SubmissionId(1), risk, panel },
        );

        let events =
            market.on_loss_event(crate::types::Day(0), "US-SE", Peril::WindstormAtlantic, 800_000);

        // net_loss = min(800_000, 1_000_000) - 100_000 = 700_000
        // s1_loss  = 700_000 * 6000 / 10_000 = 420_000
        // s2_loss  = 700_000 * 4000 / 10_000 = 280_000
        let find = |sid: SyndicateId| {
            events
                .iter()
                .find_map(|(_, e)| match e {
                    Event::ClaimSettled { syndicate_id, amount, .. } if *syndicate_id == sid => {
                        Some(*amount)
                    }
                    _ => None,
                })
                .unwrap_or_else(|| panic!("no ClaimSettled for {sid:?}"))
        };
        assert_eq!(find(SyndicateId(1)), 420_000);
        assert_eq!(find(SyndicateId(2)), 280_000);
    }

    #[test]
    fn market_registers_policy_on_bound() {
        let mut market = Market::new();
        let sid = SubmissionId(1);
        let risk = make_risk();
        let panel = Panel {
            entries: vec![PanelEntry {
                syndicate_id: SyndicateId(1),
                share_bps: 10_000,
                premium: 50_000,
            }],
        };
        market.on_policy_bound(sid, risk.clone(), panel.clone());
        assert_eq!(market.policies.len(), 1);
        let policy = market.policies.values().next().unwrap();
        assert_eq!(policy.submission_id, sid);
        assert_eq!(policy.risk.limit, risk.limit);
        assert_eq!(policy.panel.entries.len(), 1);
    }
}
