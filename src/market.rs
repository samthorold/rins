use std::collections::{HashMap, HashSet};

use rand::Rng;

use crate::events::{Event, Panel, PanelEntry, Peril, Risk};
use crate::perils::DamageFractionModel;
use crate::syndicate::Syndicate;
use crate::types::{Day, InsuredId, PolicyId, SubmissionId, SyndicateId, Year};

/// Industry-wide statistics published at year-end.
/// Syndicates read these when pricing for the next year.
#[allow(dead_code)]
pub struct YearStats {
    pub year: Year,
    pub industry_loss_ratio: f64,
    pub active_syndicate_count: usize,
    /// Realised loss ratio per line of business for the completed year.
    /// Passed into each Syndicate::on_year_end so EWMA stays current.
    pub loss_ratios_by_line: HashMap<String, f64>,
}

/// Transient state for a submission while it is in the quoting pipeline.
#[allow(dead_code)]
struct PendingSubmission {
    submission_id: SubmissionId,
    broker_id: crate::types::BrokerId,
    insured_id: InsuredId,
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
    #[allow(dead_code)]
    pub submission_id: SubmissionId,
    pub insured_id: InsuredId,
    pub risk: Risk,
    pub panel: Panel,
    /// The simulation year in which this policy was bound.
    /// Policies are annual: they expire at the end of `bound_year` and must
    /// not generate claims or be carried forward into the following year.
    pub bound_year: Year,
}

pub struct Market {
    next_policy_id: u64,
    pending: HashMap<SubmissionId, PendingSubmission>,
    pub policies: HashMap<PolicyId, BoundPolicy>,
    /// Inverted index: (territory, peril) → policy IDs. Populated in
    /// `on_policy_bound`; used by `on_loss_event` to skip non-matching policies.
    pub peril_territory_index: HashMap<(String, Peril), Vec<PolicyId>>,
    /// Risk stashed at panel-assembly time, consumed when `PolicyBound` fires.
    risk_cache: HashMap<SubmissionId, Risk>,
    /// InsuredId stashed at panel-assembly time alongside `risk_cache`.
    insured_cache: HashMap<SubmissionId, InsuredId>,
    /// Year-to-date gross premiums written per line of business.
    ytd_premiums_by_line: HashMap<String, u64>,
    /// Year-to-date claims settled per line of business.
    ytd_claims_by_line: HashMap<String, u64>,
}

impl Default for Market {
    fn default() -> Self {
        Self::new()
    }
}

impl Market {
    pub fn new() -> Self {
        Market {
            next_policy_id: 0,
            pending: HashMap::new(),
            policies: HashMap::new(),
            peril_territory_index: HashMap::new(),
            risk_cache: HashMap::new(),
            insured_cache: HashMap::new(),
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
        insured_id: InsuredId,
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
                insured_id,
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
            day.offset(2),
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
                    day.offset(3),
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
        // Lead declined: lead_premium is None iff the lead has not yet quoted.
        // Abandon the submission — no followers will be invited.
        if pending.lead_premium.is_none() {
            self.pending.remove(&submission_id);
            return vec![(day, Event::SubmissionAbandoned { submission_id })];
        }
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
        // Stash risk and insured_id so the dispatch layer can pass them to on_policy_bound.
        self.risk_cache.insert(submission_id, pending.risk.clone());
        self.insured_cache.insert(submission_id, pending.insured_id);

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
            day.offset(5),
            Event::PolicyBound {
                submission_id,
                panel: Panel { entries },
            },
        )]
    }

    /// Register a newly bound policy. Called when `PolicyBound` fires.
    /// Accumulates gross premium into the YTD counter for the policy's line.
    /// Returns the assigned `PolicyId` so the caller can schedule per-policy events.
    pub fn on_policy_bound(
        &mut self,
        submission_id: SubmissionId,
        risk: Risk,
        panel: Panel,
        year: Year,
    ) -> PolicyId {
        let total_premium: u64 = panel.entries.iter().map(|e| e.premium).sum();
        *self
            .ytd_premiums_by_line
            .entry(risk.line_of_business.clone())
            .or_insert(0) += total_premium;

        // Retrieve insured_id stashed at panel-assembly time.
        let insured_id = self.insured_cache.remove(&submission_id).unwrap_or(InsuredId(0));

        let policy_id = PolicyId(self.next_policy_id);
        self.next_policy_id += 1;
        for &peril in &risk.perils_covered {
            if peril == Peril::Attritional {
                // Attritional claims are scheduled per-policy at bind time;
                // no global LossEvent routing needed.
                continue;
            }
            self.peril_territory_index
                .entry((risk.territory.clone(), peril))
                .or_default()
                .push(policy_id);
        }
        self.policies.insert(
            policy_id,
            BoundPolicy {
                policy_id,
                submission_id,
                insured_id,
                risk,
                panel,
                bound_year: year,
            },
        );
        policy_id
    }

    /// Remove all policies written in `year` from the active portfolio.
    ///
    /// Lloyd's policies are annual contracts: they expire at the end of the year
    /// in which they were bound. Calling this at YearEnd prevents stale policies
    /// from accumulating across years and being hit by subsequent loss events.
    pub fn expire_policies(&mut self, year: Year) {
        let expired: HashSet<PolicyId> = self
            .policies
            .values()
            .filter(|p| p.bound_year == year)
            .map(|p| p.policy_id)
            .collect();

        if expired.is_empty() {
            return;
        }

        // Remove expired IDs from every index bucket; drop buckets that go empty.
        for ids in self.peril_territory_index.values_mut() {
            ids.retain(|id| !expired.contains(id));
        }
        self.peril_territory_index.retain(|_, ids| !ids.is_empty());

        self.policies.retain(|_, p| p.bound_year != year);
    }

    /// Distribute a loss event to all matching bound policies.
    ///
    /// For each policy that covers `peril` in `region`, samples a damage fraction
    /// from `damage_models` and computes:
    ///   `ground_up_loss = damage_fraction × sum_insured` (naturally ≤ sum_insured).
    ///
    /// Emits one `InsuredLoss` per matching policy. The caller dispatches each
    /// `InsuredLoss` to `on_insured_loss` to produce `ClaimSettled` events.
    pub fn on_loss_event(
        &self,
        day: Day,
        region: &str,
        peril: Peril,
        damage_models: &HashMap<Peril, DamageFractionModel>,
        rng: &mut impl Rng,
    ) -> Vec<(Day, Event)> {
        let Some(ids) = self.peril_territory_index.get(&(region.to_string(), peril)) else {
            return vec![];
        };

        let Some(model) = damage_models.get(&peril) else {
            return vec![];
        };

        ids.iter()
            .map(|policy_id| {
                let policy = &self.policies[policy_id];
                let damage_fraction = model.sample(rng);
                // ground_up_loss ≤ sum_insured because damage_fraction ∈ [0, 1].
                let ground_up_loss = (damage_fraction * policy.risk.sum_insured as f64) as u64;
                (
                    day,
                    Event::InsuredLoss {
                        policy_id: *policy_id,
                        insured_id: policy.insured_id,
                        peril,
                        ground_up_loss,
                    },
                )
            })
            .collect()
    }

    /// Apply policy terms to a ground-up loss and emit `ClaimSettled` events.
    ///
    /// Called when an `InsuredLoss` fires.
    ///   `gross = min(ground_up_loss, limit)`
    ///   `net   = gross − attachment`
    ///
    /// Emits one `ClaimSettled` per panel entry. Policies whose net loss is zero
    /// (ground_up_loss ≤ attachment, or policy not found) produce no events.
    pub fn on_insured_loss(
        &self,
        day: Day,
        policy_id: PolicyId,
        ground_up_loss: u64,
    ) -> Vec<(Day, Event)> {
        let Some(policy) = self.policies.get(&policy_id) else {
            return vec![];
        };
        let gross = ground_up_loss.min(policy.risk.limit);
        let net = gross.saturating_sub(policy.risk.attachment);
        if net == 0 {
            return vec![];
        }
        policy
            .panel
            .entries
            .iter()
            .filter_map(|entry| {
                let syndicate_loss = net * entry.share_bps as u64 / 10_000;
                if syndicate_loss == 0 {
                    return None;
                }
                Some((
                    day,
                    Event::ClaimSettled {
                        policy_id,
                        syndicate_id: entry.syndicate_id,
                        amount: syndicate_loss,
                    },
                ))
            })
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use rand::SeedableRng;
    use rand_chacha::ChaCha20Rng;

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

    fn rng() -> ChaCha20Rng {
        ChaCha20Rng::seed_from_u64(42)
    }

    /// A damage_models map with a single full-damage model (always returns 1.0).
    /// Pareto(scale=1.0, shape=2.0) always produces values ≥ 1.0, clipped to 1.0.
    fn full_damage_models() -> HashMap<Peril, DamageFractionModel> {
        [(Peril::WindstormAtlantic, DamageFractionModel::Pareto { scale: 1.0, shape: 2.0 })]
            .into_iter()
            .collect()
    }

    // ─── on_insured_loss: policy terms ────────────────────────────────────────

    /// ground_up_loss < attachment → no ClaimSettled.
    #[test]
    fn insured_loss_below_attachment_generates_no_claim() {
        let mut market = Market::new();
        // make_risk: attachment = 100_000, limit = 1_000_000
        market.on_policy_bound(SubmissionId(1), make_risk(), Panel {
            entries: vec![PanelEntry {
                syndicate_id: SyndicateId(1),
                share_bps: 10_000,
                premium: 0,
            }],
        }, Year(1));

        // ground_up_loss (50_000) < attachment (100_000) → net = 0
        let events = market.on_insured_loss(Day(0), PolicyId(0), 50_000);
        assert!(
            events.is_empty(),
            "expected no ClaimSettled when ground_up_loss <= attachment, got {events:?}"
        );
    }

    /// damage_fraction=1.0 → ground_up_loss = sum_insured → capped at limit.
    #[test]
    fn insured_loss_capped_at_sum_insured_then_limit() {
        let mut market = Market::new();
        // make_risk: sum_insured=2M, limit=1M, attachment=100K
        market.on_policy_bound(SubmissionId(1), make_risk(), Panel {
            entries: vec![PanelEntry {
                syndicate_id: SyndicateId(1),
                share_bps: 10_000,
                premium: 0,
            }],
        }, Year(1));

        // ground_up_loss = sum_insured = 2_000_000 → gross = min(2M, 1M) = 1M
        // net = 1M - 100K = 900K
        let events = market.on_insured_loss(Day(0), PolicyId(0), 2_000_000);
        let amount = events.iter().find_map(|(_, e)| match e {
            Event::ClaimSettled { amount, .. } => Some(*amount),
            _ => None,
        }).expect("expected ClaimSettled");
        assert_eq!(amount, 900_000, "claim should be capped at limit−attachment");
    }

    /// Per-syndicate allocation matches shares via on_insured_loss.
    #[test]
    fn per_syndicate_loss_allocation_matches_share() {
        use crate::types::SubmissionId;

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
        market.on_policy_bound(SubmissionId(1), risk, panel, Year(1));

        // ground_up_loss = 800_000; gross = min(800K, 1M) = 800K; net = 800K - 100K = 700K
        // s1 = 700K * 6000/10000 = 420K; s2 = 700K * 4000/10000 = 280K
        let events = market.on_insured_loss(Day(0), PolicyId(0), 800_000);

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
    fn ytd_accumulates_premiums_and_claims_then_resets() {
        use crate::types::{PolicyId, SubmissionId, Year};

        let mut market = Market::new();
        // Bind a policy with known premium (80_000 for "property" on US-SE).
        let risk = make_risk(); // property / US-SE / attachment=100_000 / limit=1_000_000
        let panel = Panel {
            entries: vec![PanelEntry {
                syndicate_id: SyndicateId(1),
                share_bps: 10_000,
                premium: 80_000,
            }],
        };
        market.on_policy_bound(SubmissionId(1), risk, panel, Year(1));

        // on_policy_bound assigns PolicyId(0) (next_policy_id starts at 0).
        market.on_claim_settled(PolicyId(0), 40_000);

        let stats = market.compute_year_stats(&[], Year(1));

        // loss_ratio = 40_000 / 80_000 = 0.50
        assert!(
            (stats.industry_loss_ratio - 0.5).abs() < 1e-10,
            "industry_loss_ratio={} expected 0.50",
            stats.industry_loss_ratio
        );
        assert!(
            (stats.loss_ratios_by_line["property"] - 0.5).abs() < 1e-10,
            "property loss_ratio={} expected 0.50",
            stats.loss_ratios_by_line["property"]
        );

        // Accumulators must reset: a second call with no new data returns the fallback.
        let stats2 = market.compute_year_stats(&[], Year(2));
        assert!(
            (stats2.industry_loss_ratio - 0.65).abs() < 1e-10,
            "YTD should reset; expected fallback 0.65, got {}",
            stats2.industry_loss_ratio
        );
        assert!(stats2.loss_ratios_by_line.is_empty());
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
        market.on_policy_bound(sid, risk.clone(), panel.clone(), Year(1));
        assert_eq!(market.policies.len(), 1);
        let policy = market.policies.values().next().unwrap();
        assert_eq!(policy.submission_id, sid);
        assert_eq!(policy.risk.limit, risk.limit);
        assert_eq!(policy.panel.entries.len(), 1);
    }

    #[test]
    fn index_populated_on_policy_bound() {
        use crate::events::{Panel, PanelEntry, Peril, Risk};
        use crate::types::{PolicyId, SubmissionId, SyndicateId};

        let mut market = Market::new();
        let risk = Risk {
            line_of_business: "property".to_string(),
            sum_insured: 1_000_000,
            territory: "US-SE".to_string(),
            limit: 500_000,
            attachment: 0,
            // Both a cat peril and Attritional: only WindstormAtlantic goes in the index.
            perils_covered: vec![Peril::WindstormAtlantic, Peril::Attritional],
        };
        let panel = Panel {
            entries: vec![PanelEntry {
                syndicate_id: SyndicateId(1),
                share_bps: 10_000,
                premium: 0,
            }],
        };
        market.on_policy_bound(SubmissionId(0), risk, panel, Year(1));

        let ids_wind = market
            .peril_territory_index
            .get(&("US-SE".to_string(), Peril::WindstormAtlantic))
            .expect("WindstormAtlantic index entry missing");
        assert_eq!(ids_wind.len(), 1);
        assert_eq!(ids_wind[0], PolicyId(0));

        // Attritional is handled per-policy at bind time — must NOT be in the index.
        assert!(
            market
                .peril_territory_index
                .get(&("US-SE".to_string(), Peril::Attritional))
                .is_none(),
            "Attritional must not be in the peril_territory_index"
        );
    }

    // ─── on_loss_event: InsuredLoss emission ─────────────────────────────────

    /// on_loss_event must emit InsuredLoss events (not ClaimSettled directly).
    #[test]
    fn on_loss_event_emits_insured_loss() {
        let mut market = Market::new();
        market.on_policy_bound(SubmissionId(1), make_risk(), Panel {
            entries: vec![PanelEntry { syndicate_id: SyndicateId(1), share_bps: 10_000, premium: 0 }],
        }, Year(1));

        let events = market.on_loss_event(
            Day(0), "US-SE", Peril::WindstormAtlantic, &full_damage_models(), &mut rng(),
        );

        assert_eq!(events.len(), 1, "one InsuredLoss per matching policy");
        assert!(
            matches!(events[0].1, Event::InsuredLoss { .. }),
            "expected InsuredLoss, got {:?}", events[0].1
        );
    }

    /// ground_up_loss must be ≤ sum_insured for every InsuredLoss.
    #[test]
    fn on_loss_event_ground_up_loss_le_sum_insured() {
        let mut market = Market::new();
        let risk = make_risk(); // sum_insured = 2_000_000
        market.on_policy_bound(SubmissionId(1), risk.clone(), Panel {
            entries: vec![PanelEntry { syndicate_id: SyndicateId(1), share_bps: 10_000, premium: 0 }],
        }, Year(1));

        let events = market.on_loss_event(
            Day(0), "US-SE", Peril::WindstormAtlantic, &full_damage_models(), &mut rng(),
        );

        for (_, e) in &events {
            if let Event::InsuredLoss { ground_up_loss, .. } = e {
                assert!(
                    *ground_up_loss <= risk.sum_insured,
                    "ground_up_loss {ground_up_loss} > sum_insured {}",
                    risk.sum_insured
                );
            }
        }
    }

    /// on_loss_event with damage_fraction=1.0 (full damage):
    /// ground_up_loss must equal sum_insured exactly.
    #[test]
    fn on_loss_event_full_damage_gives_sum_insured() {
        let mut market = Market::new();
        let risk = make_risk(); // sum_insured = 2_000_000
        market.on_policy_bound(SubmissionId(1), risk.clone(), Panel {
            entries: vec![PanelEntry { syndicate_id: SyndicateId(1), share_bps: 10_000, premium: 0 }],
        }, Year(1));

        let events = market.on_loss_event(
            Day(0), "US-SE", Peril::WindstormAtlantic, &full_damage_models(), &mut rng(),
        );

        if let Event::InsuredLoss { ground_up_loss, .. } = &events[0].1 {
            assert_eq!(
                *ground_up_loss, risk.sum_insured,
                "full damage fraction must yield ground_up_loss == sum_insured"
            );
        }
    }

    /// No InsuredLoss for unmatched peril.
    #[test]
    fn no_insured_loss_for_unmatched_peril() {
        let mut market = Market::new();
        market.on_policy_bound(
            SubmissionId(1),
            make_risk(), // perils_covered = [WindstormAtlantic]
            Panel { entries: vec![PanelEntry { syndicate_id: SyndicateId(1), share_bps: 10_000, premium: 0 }] },
            Year(1),
        );
        let events = market.on_loss_event(
            Day(0), "US-SE", Peril::EarthquakeUS, &full_damage_models(), &mut rng(),
        );
        assert!(events.is_empty(), "expected no InsuredLoss for unmatched peril, got {events:?}");
    }

    /// No InsuredLoss for unmatched territory.
    #[test]
    fn no_insured_loss_for_unmatched_territory() {
        let mut market = Market::new();
        market.on_policy_bound(
            SubmissionId(1),
            make_risk(), // territory = "US-SE"
            Panel { entries: vec![PanelEntry { syndicate_id: SyndicateId(1), share_bps: 10_000, premium: 0 }] },
            Year(1),
        );
        let models: HashMap<Peril, DamageFractionModel> = [(
            Peril::WindstormAtlantic,
            DamageFractionModel::Pareto { scale: 1.0, shape: 2.0 },
        )].into_iter().collect();
        let events = market.on_loss_event(Day(0), "EU", Peril::WindstormAtlantic, &models, &mut rng());
        assert!(events.is_empty(), "expected no InsuredLoss for unmatched territory, got {events:?}");
    }

    /// on_loss_event with no damage model for the peril → no events.
    #[test]
    fn no_insured_loss_when_damage_model_missing() {
        let mut market = Market::new();
        market.on_policy_bound(
            SubmissionId(1),
            make_risk(),
            Panel { entries: vec![PanelEntry { syndicate_id: SyndicateId(1), share_bps: 10_000, premium: 0 }] },
            Year(1),
        );
        // Empty damage_models — no model for WindstormAtlantic.
        let events = market.on_loss_event(
            Day(0), "US-SE", Peril::WindstormAtlantic, &HashMap::new(), &mut rng(),
        );
        assert!(events.is_empty(), "expected no events when damage model missing");
    }

    // ─── Claim-splitting via on_insured_loss ──────────────────────────────────

    #[test]
    fn claim_below_attachment_produces_no_event() {
        let mut market = Market::new();
        // make_risk: attachment = 100_000, limit = 1_000_000
        market.on_policy_bound(
            SubmissionId(1),
            make_risk(),
            Panel { entries: vec![PanelEntry { syndicate_id: SyndicateId(1), share_bps: 10_000, premium: 0 }] },
            Year(1),
        );
        // ground_up_loss (50_000) < attachment (100_000) → net_loss = 0
        let events = market.on_insured_loss(Day(0), PolicyId(0), 50_000);
        assert!(
            events.is_empty(),
            "expected no ClaimSettled when ground_up_loss <= attachment, got {events:?}"
        );
    }

    #[test]
    fn claim_severity_capped_at_policy_limit() {
        let mut market = Market::new();
        // make_risk: limit = 1_000_000, attachment = 100_000
        market.on_policy_bound(
            SubmissionId(1),
            make_risk(),
            Panel { entries: vec![PanelEntry { syndicate_id: SyndicateId(1), share_bps: 10_000, premium: 0 }] },
            Year(1),
        );
        // ground_up = 5_000_000 > limit 1_000_000
        // gross = 1_000_000; net = 900_000; amount = 900_000
        let events = market.on_insured_loss(Day(0), PolicyId(0), 5_000_000);
        let amount = events.iter().find_map(|(_, e)| match e {
            Event::ClaimSettled { syndicate_id, amount, .. } if *syndicate_id == SyndicateId(1) => Some(*amount),
            _ => None,
        }).expect("expected ClaimSettled for SyndicateId(1)");
        assert_eq!(amount, 900_000, "capped claim should be limit - attachment");
    }

    #[test]
    fn sum_of_claims_le_net_loss_three_syndicates() {
        let mut market = Market::new();
        market.on_policy_bound(
            SubmissionId(1),
            make_risk(),
            Panel {
                entries: vec![
                    PanelEntry { syndicate_id: SyndicateId(1), share_bps: 5_000, premium: 0 },
                    PanelEntry { syndicate_id: SyndicateId(2), share_bps: 3_000, premium: 0 },
                    PanelEntry { syndicate_id: SyndicateId(3), share_bps: 2_000, premium: 0 },
                ],
            },
            Year(1),
        );
        // ground_up = 600_000; gross = 600_000; net = 600_000 - 100_000 = 500_000
        let events = market.on_insured_loss(Day(0), PolicyId(0), 600_000);
        let net_loss = 500_000u64;

        let find = |sid: SyndicateId| {
            events.iter().find_map(|(_, e)| match e {
                Event::ClaimSettled { syndicate_id, amount, .. } if *syndicate_id == sid => Some(*amount),
                _ => None,
            }).unwrap_or_else(|| panic!("no ClaimSettled for {sid:?}"))
        };

        let a1 = find(SyndicateId(1)); // 500_000 * 5000 / 10_000 = 250_000
        let a2 = find(SyndicateId(2)); // 500_000 * 3000 / 10_000 = 150_000
        let a3 = find(SyndicateId(3)); // 500_000 * 2000 / 10_000 = 100_000

        assert_eq!(a1, 250_000);
        assert_eq!(a2, 150_000);
        assert_eq!(a3, 100_000);

        let sum = a1 + a2 + a3;
        assert!(sum <= net_loss, "sum_claims={sum} > net_loss={net_loss}");
        assert!((net_loss - sum) < 3, "rounding gap {} should be < n_syndicates(3)", net_loss - sum);
    }

    // ─── Policy expiry tests ──────────────────────────────────────────────────

    /// A policy expired at YearEnd must not generate claims in the following year.
    #[test]
    fn expired_policy_does_not_generate_claims() {
        use crate::events::{Panel, PanelEntry};
        use crate::types::{Day, SubmissionId, SyndicateId, Year};

        let mut market = Market::new();
        let risk = Risk {
            line_of_business: "property".to_string(),
            sum_insured: 2_000_000,
            territory: "US-SE".to_string(),
            limit: 1_000_000,
            attachment: 0,
            perils_covered: vec![Peril::WindstormAtlantic],
        };
        let panel = Panel {
            entries: vec![PanelEntry { syndicate_id: SyndicateId(1), share_bps: 10_000, premium: 50_000 }],
        };

        market.on_policy_bound(SubmissionId(1), risk, panel, Year(1));
        market.expire_policies(Year(1));

        assert!(market.policies.is_empty(), "expired policy should be removed from policies");
        assert!(
            market.peril_territory_index.is_empty(),
            "index should be empty after expiry"
        );

        let events = market.on_loss_event(
            Day(360), "US-SE", Peril::WindstormAtlantic, &full_damage_models(), &mut rng(),
        );
        assert!(events.is_empty(), "expected no InsuredLoss for expired policy, got {events:?}");
    }

    /// Regression: per-policy independent losses (not pro-rated).
    #[test]
    fn loss_above_attachment_generates_claims_with_many_policies() {
        let attachment = 500_000u64;
        let limit = 5_000_000u64;
        let sum_insured = 10_000_000u64;

        let mut market = Market::new();
        for i in 0..200u64 {
            market.on_policy_bound(
                SubmissionId(i),
                Risk {
                    line_of_business: "property".to_string(),
                    sum_insured,
                    territory: "US-SE".to_string(),
                    limit,
                    attachment,
                    perils_covered: vec![Peril::WindstormAtlantic],
                },
                Panel {
                    entries: vec![PanelEntry { syndicate_id: SyndicateId(i + 1), share_bps: 10_000, premium: 0 }],
                },
                Year(1),
            );
        }

        // With full_damage_models (damage_fraction=1.0), ground_up = 10M per policy.
        // gross = min(10M, 5M) = 5M; net = 5M - 500K = 4.5M → 200 ClaimSettled events.
        let insured_events = market.on_loss_event(
            Day(1), "US-SE", Peril::WindstormAtlantic, &full_damage_models(), &mut rng(),
        );
        assert_eq!(insured_events.len(), 200, "expected 200 InsuredLoss events");

        let mut claim_count = 0;
        for (_, e) in &insured_events {
            if let Event::InsuredLoss { policy_id, ground_up_loss, .. } = e {
                let claims = market.on_insured_loss(Day(1), *policy_id, *ground_up_loss);
                claim_count += claims.len();
                for (_, ce) in &claims {
                    if let Event::ClaimSettled { amount, .. } = ce {
                        let expected = (sum_insured.min(limit) - attachment);
                        assert_eq!(*amount, expected, "wrong claim amount");
                    }
                }
            }
        }
        assert_eq!(claim_count, 200, "expected 200 ClaimSettled events");
    }

    #[test]
    fn each_policy_gets_independent_claim_based_on_ground_up_loss() {
        // Two policies with different limits.
        // Policy A: limit=3M, attachment=0 → claim=min(ground_up, 3M)
        // Policy B: limit=500K, attachment=0 → claim=min(ground_up, 500K)
        // With full damage (ground_up = sum_insured):
        //   Policy A: sum_insured=3M → ground_up=3M → gross=3M → net=3M
        //   Policy B: sum_insured=500K → ground_up=500K → gross=500K → net=500K
        let mut market = Market::new();
        market.on_policy_bound(
            SubmissionId(1),
            Risk {
                line_of_business: "property".to_string(),
                sum_insured: 3_000_000,
                territory: "US-SE".to_string(),
                limit: 3_000_000,
                attachment: 0,
                perils_covered: vec![Peril::WindstormAtlantic],
            },
            Panel { entries: vec![PanelEntry { syndicate_id: SyndicateId(1), share_bps: 10_000, premium: 0 }] },
            Year(1),
        );
        market.on_policy_bound(
            SubmissionId(2),
            Risk {
                line_of_business: "property".to_string(),
                sum_insured: 500_000,
                territory: "US-SE".to_string(),
                limit: 500_000,
                attachment: 0,
                perils_covered: vec![Peril::WindstormAtlantic],
            },
            Panel { entries: vec![PanelEntry { syndicate_id: SyndicateId(2), share_bps: 10_000, premium: 0 }] },
            Year(1),
        );

        let insured_events = market.on_loss_event(
            Day(0), "US-SE", Peril::WindstormAtlantic, &full_damage_models(), &mut rng(),
        );

        let mut claim_a = None;
        let mut claim_b = None;
        for (_, e) in &insured_events {
            if let Event::InsuredLoss { policy_id, ground_up_loss, .. } = e {
                let claims = market.on_insured_loss(Day(0), *policy_id, *ground_up_loss);
                for (_, ce) in claims {
                    if let Event::ClaimSettled { syndicate_id, amount, .. } = ce {
                        if syndicate_id == SyndicateId(1) { claim_a = Some(amount); }
                        if syndicate_id == SyndicateId(2) { claim_b = Some(amount); }
                    }
                }
            }
        }

        assert_eq!(claim_a, Some(3_000_000), "policy A claim should equal sum_insured=limit");
        assert_eq!(claim_b, Some(500_000), "policy B claim should equal sum_insured=limit");
    }

    #[test]
    fn only_expired_year_policies_are_removed() {
        use crate::events::{Panel, PanelEntry};
        use crate::types::{Day, SubmissionId, SyndicateId, Year};

        let mut market = Market::new();
        let make_panel = |sid: u64| Panel {
            entries: vec![PanelEntry { syndicate_id: SyndicateId(sid), share_bps: 10_000, premium: 0 }],
        };
        let risk = Risk {
            line_of_business: "property".to_string(),
            sum_insured: 2_000_000,
            territory: "US-SE".to_string(),
            limit: 1_000_000,
            attachment: 0,
            perils_covered: vec![Peril::WindstormAtlantic],
        };

        market.on_policy_bound(SubmissionId(1), risk.clone(), make_panel(1), Year(1));
        market.on_policy_bound(SubmissionId(2), risk, make_panel(2), Year(2));

        market.expire_policies(Year(1));

        assert_eq!(market.policies.len(), 1, "only year-2 policy should remain");

        let insured_events = market.on_loss_event(
            Day(360), "US-SE", Peril::WindstormAtlantic, &full_damage_models(), &mut rng(),
        );
        // Only year-2 policy (Syn 2) should be hit.
        let mut hit_syndicates: Vec<u64> = vec![];
        for (_, e) in &insured_events {
            if let Event::InsuredLoss { policy_id, ground_up_loss, .. } = e {
                let claims = market.on_insured_loss(Day(360), *policy_id, *ground_up_loss);
                for (_, ce) in claims {
                    if let Event::ClaimSettled { syndicate_id, .. } = ce {
                        hit_syndicates.push(syndicate_id.0);
                    }
                }
            }
        }
        assert!(hit_syndicates.contains(&2), "year-2 policy (Syn 2) should be hit");
        assert!(!hit_syndicates.contains(&1), "expired year-1 policy (Syn 1) must not be hit");
    }
}
