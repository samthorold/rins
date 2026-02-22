use std::collections::HashMap;

use rand::Rng;

use crate::events::{Event, Peril, Risk};
use crate::perils::DamageFractionModel;
use crate::types::{Day, InsuredId, InsurerId, PolicyId, SubmissionId, Year};

/// A successfully bound policy.
pub struct BoundPolicy {
    pub policy_id: PolicyId,
    pub submission_id: SubmissionId,
    pub insured_id: InsuredId,
    pub insurer_id: InsurerId,
    pub risk: Risk,
    pub premium: u64,
    pub bound_year: Year,
}

/// Transient state while a submission awaits a quote.
struct PendingSubmission {
    insured_id: InsuredId,
    risk: Risk,
}

pub struct Market {
    next_policy_id: u64,
    pending: HashMap<SubmissionId, PendingSubmission>,
    pub policies: HashMap<PolicyId, BoundPolicy>,
    /// insured_id → active PolicyId for this year.
    pub insured_active_policies: HashMap<InsuredId, PolicyId>,
    /// YTD accumulators for loss ratio computation.
    ytd_premiums: u64,
    ytd_claims: u64,
    /// Per-(policy, year) remaining insurable asset value.
    /// Initialized to sum_insured on first hit; decremented to prevent aggregate GUL > sum_insured.
    remaining_asset_value: HashMap<(PolicyId, Year), u64>,
    /// Round-robin cursor for insurer selection.
    pub next_insurer_idx: usize,
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
            insured_active_policies: HashMap::new(),
            ytd_premiums: 0,
            ytd_claims: 0,
            remaining_asset_value: HashMap::new(),
            next_insurer_idx: 0,
        }
    }

    /// Store the submission and schedule a QuoteRequested to the next insurer (round-robin).
    pub fn on_submission_arrived(
        &mut self,
        day: Day,
        submission_id: SubmissionId,
        insured_id: InsuredId,
        risk: Risk,
        insurers: &[InsurerId],
    ) -> Vec<(Day, Event)> {
        if insurers.is_empty() {
            return vec![];
        }
        let insurer_id = insurers[self.next_insurer_idx % insurers.len()];
        self.next_insurer_idx += 1;
        self.pending.insert(submission_id, PendingSubmission { insured_id, risk });
        vec![(day.offset(1), Event::QuoteRequested { submission_id, insurer_id })]
    }

    /// Retrieve the risk for a pending submission (used by dispatch to pass to insurer).
    pub fn get_quote_params(&self, submission_id: SubmissionId) -> Option<(InsuredId, Risk)> {
        self.pending
            .get(&submission_id)
            .map(|p| (p.insured_id, p.risk.clone()))
    }

    /// Insurer has quoted. Bind immediately (insured always accepts).
    /// Returns PolicyBound + PolicyExpired events.
    pub fn on_quote_issued(
        &mut self,
        day: Day,
        submission_id: SubmissionId,
        insurer_id: InsurerId,
        premium: u64,
        year: Year,
    ) -> Vec<(Day, Event)> {
        let pending = match self.pending.remove(&submission_id) {
            Some(p) => p,
            None => return vec![],
        };

        let policy_id = PolicyId(self.next_policy_id);
        self.next_policy_id += 1;

        self.ytd_premiums += premium;
        self.insured_active_policies.insert(pending.insured_id, policy_id);

        self.policies.insert(
            policy_id,
            BoundPolicy {
                policy_id,
                submission_id,
                insured_id: pending.insured_id,
                insurer_id,
                risk: pending.risk,
                premium,
                bound_year: year,
            },
        );

        let bind_day = day.offset(1);
        let expire_day = bind_day.offset(360);

        vec![
            (bind_day, Event::PolicyBound { policy_id, submission_id, insurer_id }),
            (expire_day, Event::PolicyExpired { policy_id }),
        ]
    }

    /// Remove a policy when its PolicyExpired event fires.
    pub fn on_policy_expired(&mut self, policy_id: PolicyId) {
        if let Some(policy) = self.policies.remove(&policy_id) {
            // Remove from active-policy map if it's still the current one.
            self.insured_active_policies
                .retain(|_, &mut pid| pid != policy_id);
            drop(policy); // silence dead-code warning on fields
        }
    }

    /// A catastrophe loss event has fired. Emit InsuredLoss for every active policy
    /// that covers this peril. Full coverage: ground_up_loss = damage_fraction × sum_insured.
    pub fn on_loss_event(
        &self,
        day: Day,
        peril: Peril,
        damage_models: &HashMap<Peril, DamageFractionModel>,
        rng: &mut impl Rng,
    ) -> Vec<(Day, Event)> {
        let Some(model) = damage_models.get(&peril) else {
            return vec![];
        };
        self.policies
            .values()
            .filter(|p| p.risk.perils_covered.contains(&peril))
            .filter_map(|policy| {
                let df = model.sample(rng);
                let gul = (df * policy.risk.sum_insured as f64) as u64;
                if gul == 0 {
                    return None;
                }
                Some((
                    day,
                    Event::InsuredLoss {
                        policy_id: policy.policy_id,
                        insured_id: policy.insured_id,
                        peril,
                        ground_up_loss: gul,
                    },
                ))
            })
            .collect()
    }

    /// Apply full coverage (limit = sum_insured, attachment = 0) to a ground-up loss.
    /// Caps at remaining insurable asset value for the (policy, year) to prevent
    /// aggregate annual GUL exceeding sum_insured.
    /// Returns a ClaimSettled event if the effective loss is non-zero.
    pub fn on_insured_loss(
        &mut self,
        day: Day,
        policy_id: PolicyId,
        ground_up_loss: u64,
        peril: Peril,
    ) -> Vec<(Day, Event)> {
        let policy = match self.policies.get(&policy_id) {
            Some(p) => p,
            None => return vec![],
        };
        let insurer_id = policy.insurer_id;
        let sum_insured = policy.risk.sum_insured;

        let year = Year((day.0 / Day::DAYS_PER_YEAR) as u32 + 1);
        let remaining = self
            .remaining_asset_value
            .entry((policy_id, year))
            .or_insert(sum_insured);
        let effective_gul = ground_up_loss.min(*remaining);
        *remaining = remaining.saturating_sub(effective_gul);

        if effective_gul == 0 {
            return vec![];
        }

        self.ytd_claims += effective_gul;

        vec![(
            day,
            Event::ClaimSettled { policy_id, insurer_id, amount: effective_gul, peril },
        )]
    }

    pub fn loss_ratio(&self) -> f64 {
        if self.ytd_premiums > 0 {
            self.ytd_claims as f64 / self.ytd_premiums as f64
        } else {
            0.0
        }
    }

    pub fn total_premiums(&self) -> u64 {
        self.ytd_premiums
    }

    pub fn total_claims(&self) -> u64 {
        self.ytd_claims
    }

    pub fn reset_ytd(&mut self) {
        self.ytd_premiums = 0;
        self.ytd_claims = 0;
    }
}

#[cfg(test)]
mod tests {
    use rand::SeedableRng;
    use rand_chacha::ChaCha20Rng;

    use super::*;
    use crate::config::SMALL_ASSET_VALUE;
    use crate::perils::DamageFractionModel;

    fn rng() -> ChaCha20Rng {
        ChaCha20Rng::seed_from_u64(42)
    }

    fn small_risk() -> Risk {
        Risk {
            sum_insured: SMALL_ASSET_VALUE,
            territory: "US-SE".to_string(),
            perils_covered: vec![Peril::WindstormAtlantic, Peril::Attritional],
        }
    }

    fn full_damage_models() -> HashMap<Peril, DamageFractionModel> {
        // Pareto(scale=1.0, shape=2.0) always produces values ≥ 1.0, clipped to 1.0.
        [(Peril::WindstormAtlantic, DamageFractionModel::Pareto { scale: 1.0, shape: 2.0 })]
            .into_iter()
            .collect()
    }

    fn bind_policy(market: &mut Market, submission_id: u64, insured_id: u64) -> PolicyId {
        let sid = SubmissionId(submission_id);
        let iid = InsuredId(insured_id);
        let insurer_id = InsurerId(1);
        market.pending.insert(sid, PendingSubmission { insured_id: iid, risk: small_risk() });
        let events =
            market.on_quote_issued(Day(0), sid, insurer_id, 100_000, Year(1));
        events
            .iter()
            .find_map(|(_, e)| match e {
                Event::PolicyBound { policy_id, .. } => Some(*policy_id),
                _ => None,
            })
            .expect("expected PolicyBound")
    }

    // ── on_submission_arrived ─────────────────────────────────────────────────

    #[test]
    fn on_submission_arrived_returns_quote_requested() {
        let mut market = Market::new();
        let insurers = vec![InsurerId(1), InsurerId(2)];
        let events = market.on_submission_arrived(
            Day(0),
            SubmissionId(1),
            InsuredId(1),
            small_risk(),
            &insurers,
        );
        assert_eq!(events.len(), 1);
        assert!(matches!(events[0].1, Event::QuoteRequested { insurer_id: InsurerId(1), .. }));
    }

    #[test]
    fn round_robin_distributes_across_insurers() {
        let mut market = Market::new();
        let insurers = vec![InsurerId(1), InsurerId(2), InsurerId(3)];
        let mut assigned = vec![];
        for i in 0..6 {
            let events = market.on_submission_arrived(
                Day(0),
                SubmissionId(i),
                InsuredId(i),
                small_risk(),
                &insurers,
            );
            if let Event::QuoteRequested { insurer_id, .. } = &events[0].1 {
                assigned.push(insurer_id.0);
            }
        }
        // Should cycle 1,2,3,1,2,3
        assert_eq!(assigned, vec![1, 2, 3, 1, 2, 3]);
    }

    #[test]
    fn on_submission_arrived_empty_insurers_returns_empty() {
        let mut market = Market::new();
        let events = market.on_submission_arrived(
            Day(0),
            SubmissionId(1),
            InsuredId(1),
            small_risk(),
            &[],
        );
        assert!(events.is_empty());
    }

    // ── on_quote_issued ────────────────────────────────────────────────────────

    #[test]
    fn on_quote_issued_binds_policy_and_schedules_expiry() {
        let mut market = Market::new();
        let sid = SubmissionId(1);
        market.pending.insert(sid, PendingSubmission { insured_id: InsuredId(1), risk: small_risk() });
        let events = market.on_quote_issued(Day(10), sid, InsurerId(1), 50_000, Year(1));

        let has_bound = events.iter().any(|(_, e)| matches!(e, Event::PolicyBound { .. }));
        let has_expired = events.iter().any(|(_, e)| matches!(e, Event::PolicyExpired { .. }));
        assert!(has_bound, "expected PolicyBound");
        assert!(has_expired, "expected PolicyExpired");
    }

    #[test]
    fn policy_expires_360_days_after_bind() {
        let mut market = Market::new();
        let sid = SubmissionId(1);
        market.pending.insert(sid, PendingSubmission { insured_id: InsuredId(1), risk: small_risk() });
        let events = market.on_quote_issued(Day(10), sid, InsurerId(1), 50_000, Year(1));

        let bind_day = events
            .iter()
            .find_map(|(d, e)| if matches!(e, Event::PolicyBound { .. }) { Some(*d) } else { None })
            .unwrap();
        let expire_day = events
            .iter()
            .find_map(|(d, e)| if matches!(e, Event::PolicyExpired { .. }) { Some(*d) } else { None })
            .unwrap();
        assert_eq!(expire_day.0, bind_day.0 + 360, "expiry must be 360 days after bind");
    }

    #[test]
    fn on_quote_issued_accumulates_ytd_premiums() {
        let mut market = Market::new();
        market.pending.insert(
            SubmissionId(1),
            PendingSubmission { insured_id: InsuredId(1), risk: small_risk() },
        );
        market.on_quote_issued(Day(0), SubmissionId(1), InsurerId(1), 80_000, Year(1));
        assert_eq!(market.ytd_premiums, 80_000);
    }

    // ── on_policy_expired ─────────────────────────────────────────────────────

    #[test]
    fn on_policy_expired_removes_policy() {
        let mut market = Market::new();
        let pid = bind_policy(&mut market, 1, 1);
        assert!(market.policies.contains_key(&pid));
        market.on_policy_expired(pid);
        assert!(!market.policies.contains_key(&pid), "policy must be removed on expiry");
    }

    #[test]
    fn on_policy_expired_clears_active_policy_map() {
        let mut market = Market::new();
        let pid = bind_policy(&mut market, 1, 1);
        assert!(market.insured_active_policies.contains_key(&InsuredId(1)));
        market.on_policy_expired(pid);
        assert!(
            !market.insured_active_policies.contains_key(&InsuredId(1)),
            "active policy map must be cleared on expiry"
        );
    }

    // ── on_loss_event ─────────────────────────────────────────────────────────

    #[test]
    fn on_loss_event_emits_insured_loss_per_policy() {
        let mut market = Market::new();
        bind_policy(&mut market, 1, 1);
        bind_policy(&mut market, 2, 2);

        let events = market.on_loss_event(Day(100), Peril::WindstormAtlantic, &full_damage_models(), &mut rng());
        assert_eq!(events.len(), 2, "one InsuredLoss per matching policy");
        for (_, e) in &events {
            assert!(matches!(e, Event::InsuredLoss { peril: Peril::WindstormAtlantic, .. }));
        }
    }

    #[test]
    fn on_loss_event_skips_non_matching_peril() {
        let mut market = Market::new();
        bind_policy(&mut market, 1, 1);
        // Attritional is in perils_covered but no damage model for it here.
        let models: HashMap<Peril, DamageFractionModel> = HashMap::new();
        let events = market.on_loss_event(Day(100), Peril::WindstormAtlantic, &models, &mut rng());
        assert!(events.is_empty(), "no events when damage model is missing");
    }

    #[test]
    fn on_loss_event_ground_up_loss_le_sum_insured() {
        let mut market = Market::new();
        bind_policy(&mut market, 1, 1);
        let events =
            market.on_loss_event(Day(100), Peril::WindstormAtlantic, &full_damage_models(), &mut rng());
        for (_, e) in &events {
            if let Event::InsuredLoss { ground_up_loss, .. } = e {
                assert!(
                    *ground_up_loss <= SMALL_ASSET_VALUE,
                    "gul {ground_up_loss} > sum_insured {SMALL_ASSET_VALUE}"
                );
            }
        }
    }

    // ── on_insured_loss ───────────────────────────────────────────────────────

    #[test]
    fn on_insured_loss_returns_claim_settled() {
        let mut market = Market::new();
        let pid = bind_policy(&mut market, 1, 1);
        let events = market.on_insured_loss(Day(10), pid, 100_000, Peril::WindstormAtlantic);
        assert_eq!(events.len(), 1);
        assert!(matches!(events[0].1, Event::ClaimSettled { amount: 100_000, .. }));
    }

    #[test]
    fn on_insured_loss_accumulates_ytd_claims() {
        let mut market = Market::new();
        let pid = bind_policy(&mut market, 1, 1);
        market.on_insured_loss(Day(10), pid, 50_000, Peril::Attritional);
        market.on_insured_loss(Day(20), pid, 30_000, Peril::Attritional);
        // Total is capped by remaining_asset_value after first hit, but both ≤ sum_insured.
        assert!(market.ytd_claims > 0);
    }

    #[test]
    fn aggregate_annual_gul_capped_at_sum_insured() {
        let mut market = Market::new();
        let pid = bind_policy(&mut market, 1, 1);
        let half = SMALL_ASSET_VALUE / 2 + 1;
        let e1 = market.on_insured_loss(Day(10), pid, half, Peril::WindstormAtlantic);
        let e2 = market.on_insured_loss(Day(20), pid, half, Peril::WindstormAtlantic);

        let total: u64 = e1.iter().chain(e2.iter()).filter_map(|(_, e)| {
            if let Event::ClaimSettled { amount, .. } = e { Some(*amount) } else { None }
        }).sum();

        assert_eq!(
            total, SMALL_ASSET_VALUE,
            "aggregate annual GUL must not exceed sum_insured"
        );
    }

    #[test]
    fn on_insured_loss_unknown_policy_produces_no_event() {
        let mut market = Market::new();
        let events =
            market.on_insured_loss(Day(0), PolicyId(999), 100_000, Peril::Attritional);
        assert!(events.is_empty(), "unknown policy_id must produce no events");
    }

    // ── loss ratio ─────────────────────────────────────────────────────────────

    #[test]
    fn loss_ratio_computed_correctly() {
        let mut market = Market::new();
        let pid = bind_policy(&mut market, 1, 1); // adds 100_000 to ytd_premiums
        market.on_insured_loss(Day(10), pid, 50_000, Peril::Attritional);
        let lr = market.loss_ratio();
        assert!(
            (lr - 0.5).abs() < 1e-6,
            "loss_ratio={lr:.4}, expected 0.5"
        );
    }

    #[test]
    fn reset_ytd_clears_accumulators() {
        let mut market = Market::new();
        let pid = bind_policy(&mut market, 1, 1);
        market.on_insured_loss(Day(10), pid, 50_000, Peril::Attritional);
        market.reset_ytd();
        assert_eq!(market.ytd_premiums, 0);
        assert_eq!(market.ytd_claims, 0);
        assert_eq!(market.loss_ratio(), 0.0);
    }
}
