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

pub struct Market {
    next_policy_id: u64,
    /// Policies created by QuoteAccepted but not yet activated (PolicyBound not yet fired).
    pending_policies: HashMap<PolicyId, BoundPolicy>,
    /// Active policies (after PolicyBound fires) — eligible for loss routing.
    pub policies: HashMap<PolicyId, BoundPolicy>,
    /// insured_id → active PolicyId for this year.
    pub insured_active_policies: HashMap<InsuredId, PolicyId>,
    /// Per-(policy, year) remaining insurable asset value.
    /// Initialized to sum_insured on first hit; decremented to prevent aggregate GUL > sum_insured.
    remaining_asset_value: HashMap<(PolicyId, Year), u64>,
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
            pending_policies: HashMap::new(),
            policies: HashMap::new(),
            insured_active_policies: HashMap::new(),
            remaining_asset_value: HashMap::new(),
        }
    }

    /// Insured has accepted a quote. Create the policy record (not yet loss-eligible) and
    /// schedule `PolicyBound` at `day+1` and `PolicyExpired` at `day+361`.
    pub fn on_quote_accepted(
        &mut self,
        day: Day,
        submission_id: SubmissionId,
        insured_id: InsuredId,
        insurer_id: InsurerId,
        premium: u64,
        risk: Risk,
        year: Year,
    ) -> Vec<(Day, Event)> {
        let policy_id = PolicyId(self.next_policy_id);
        self.next_policy_id += 1;

        self.pending_policies.insert(
            policy_id,
            BoundPolicy {
                policy_id,
                submission_id,
                insured_id,
                insurer_id,
                risk,
                premium,
                bound_year: year,
            },
        );

        let bind_day = day.offset(1);
        let expire_day = day.offset(361);

        vec![
            (
                bind_day,
                Event::PolicyBound { policy_id, submission_id, insured_id, insurer_id, premium },
            ),
            (expire_day, Event::PolicyExpired { policy_id }),
        ]
    }

    /// PolicyBound has fired: activate the policy so it is eligible for loss routing.
    pub fn on_policy_bound(&mut self, policy_id: PolicyId) {
        if let Some(policy) = self.pending_policies.remove(&policy_id) {
            self.insured_active_policies.insert(policy.insured_id, policy_id);
            self.policies.insert(policy_id, policy);
        }
    }

    /// Insured rejected the quote — no-op in this model.
    pub fn on_quote_rejected(&mut self, _submission_id: SubmissionId) {}

    /// Remove a policy when its PolicyExpired event fires.
    pub fn on_policy_expired(&mut self, policy_id: PolicyId) {
        if let Some(policy) = self.policies.remove(&policy_id) {
            self.insured_active_policies.retain(|_, &mut pid| pid != policy_id);
            drop(policy); // silence dead-code warning on fields
        }
    }

    /// A catastrophe loss event has fired. Emit InsuredLoss for every active policy
    /// that covers this peril. Full coverage: ground_up_loss = damage_fraction × sum_insured.
    ///
    /// A single damage fraction is drawn once for the entire event and applied to every
    /// affected policy. This reflects the physical reality: a cat event's intensity field
    /// (wind speed, ground motion) is a property of the occurrence, not of individual assets.
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
        let df = model.sample(rng);
        self.policies
            .values()
            .filter(|p| p.risk.perils_covered.contains(&peril))
            .filter_map(|policy| {
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

        vec![(
            day,
            Event::ClaimSettled { policy_id, insurer_id, amount: effective_gul, peril },
        )]
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

    /// Helper: create an accepted + activated policy. Returns the PolicyId.
    fn bind_policy(market: &mut Market, submission_id: u64, insured_id: u64) -> PolicyId {
        let sid = SubmissionId(submission_id);
        let iid = InsuredId(insured_id);
        let insurer_id = InsurerId(1);
        let events = market.on_quote_accepted(
            Day(0),
            sid,
            iid,
            insurer_id,
            100_000,
            small_risk(),
            Year(1),
        );
        let policy_id = events
            .iter()
            .find_map(|(_, e)| match e {
                Event::PolicyBound { policy_id, .. } => Some(*policy_id),
                _ => None,
            })
            .expect("expected PolicyBound");
        market.on_policy_bound(policy_id);
        policy_id
    }

    // ── on_quote_accepted ─────────────────────────────────────────────────────

    #[test]
    fn on_quote_accepted_creates_policy_record() {
        let mut market = Market::new();
        let events = market.on_quote_accepted(
            Day(0),
            SubmissionId(1),
            InsuredId(1),
            InsurerId(1),
            50_000,
            small_risk(),
            Year(1),
        );
        let policy_id = events.iter().find_map(|(_, e)| match e {
            Event::PolicyBound { policy_id, .. } => Some(*policy_id),
            _ => None,
        });
        assert!(policy_id.is_some(), "PolicyBound must be scheduled");
        // Policy should be in pending (not yet active).
        let pid = policy_id.unwrap();
        assert!(market.pending_policies.contains_key(&pid));
        assert!(!market.policies.contains_key(&pid));
    }

    #[test]
    fn on_quote_accepted_schedules_policy_bound_plus_one() {
        let mut market = Market::new();
        let events = market.on_quote_accepted(
            Day(10),
            SubmissionId(1),
            InsuredId(1),
            InsurerId(1),
            50_000,
            small_risk(),
            Year(1),
        );
        let bind_day = events
            .iter()
            .find_map(|(d, e)| if matches!(e, Event::PolicyBound { .. }) { Some(*d) } else { None })
            .unwrap();
        assert_eq!(bind_day, Day(11), "PolicyBound must fire at QuoteAccepted.day + 1");
    }

    #[test]
    fn policy_expires_360_days_after_policy_bound() {
        let mut market = Market::new();
        let events = market.on_quote_accepted(
            Day(10),
            SubmissionId(1),
            InsuredId(1),
            InsurerId(1),
            50_000,
            small_risk(),
            Year(1),
        );
        let bind_day = events
            .iter()
            .find_map(|(d, e)| if matches!(e, Event::PolicyBound { .. }) { Some(*d) } else { None })
            .unwrap();
        let expire_day = events
            .iter()
            .find_map(|(d, e)| if matches!(e, Event::PolicyExpired { .. }) { Some(*d) } else { None })
            .unwrap();
        assert_eq!(
            expire_day.0,
            bind_day.0 + 360,
            "expiry must be 360 days after PolicyBound"
        );
    }

    // ── on_policy_bound ───────────────────────────────────────────────────────

    #[test]
    fn on_policy_bound_registers_active_policy() {
        let mut market = Market::new();
        let events = market.on_quote_accepted(
            Day(0),
            SubmissionId(1),
            InsuredId(1),
            InsurerId(1),
            50_000,
            small_risk(),
            Year(1),
        );
        let policy_id = events.iter().find_map(|(_, e)| match e {
            Event::PolicyBound { policy_id, .. } => Some(*policy_id),
            _ => None,
        }).unwrap();

        // Before on_policy_bound: not in active policies.
        assert!(!market.policies.contains_key(&policy_id));
        assert!(!market.insured_active_policies.contains_key(&InsuredId(1)));

        market.on_policy_bound(policy_id);

        // After on_policy_bound: active.
        assert!(market.policies.contains_key(&policy_id));
        assert_eq!(market.insured_active_policies[&InsuredId(1)], policy_id);
        // No longer pending.
        assert!(!market.pending_policies.contains_key(&policy_id));
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

    /// Damage fraction must be drawn once per cat event and shared across all
    /// affected policies. Two identical policies in the same event must receive
    /// the same ground_up_loss. This test fails with per-policy draws.
    #[test]
    fn cat_loss_uses_shared_damage_fraction() {
        let mut market = Market::new();
        bind_policy(&mut market, 1, 1);
        bind_policy(&mut market, 2, 2);
        // Both policies have SMALL_ASSET_VALUE. Use a variable model (not the
        // degenerate Pareto(1,2) that always clips to 1.0).
        let models: HashMap<Peril, DamageFractionModel> = [(
            Peril::WindstormAtlantic,
            DamageFractionModel::LogNormal { mu: -3.0, sigma: 1.0 },
        )]
        .into_iter()
        .collect();
        let events = market.on_loss_event(Day(100), Peril::WindstormAtlantic, &models, &mut rng());
        assert_eq!(events.len(), 2);
        let guls: Vec<u64> = events
            .iter()
            .filter_map(|(_, e)| {
                if let Event::InsuredLoss { ground_up_loss, .. } = e { Some(*ground_up_loss) }
                else { None }
            })
            .collect();
        assert_eq!(
            guls[0], guls[1],
            "all policies in the same cat event must share the damage fraction"
        );
    }

    #[test]
    fn on_loss_event_emits_insured_loss_per_policy() {
        let mut market = Market::new();
        bind_policy(&mut market, 1, 1);
        bind_policy(&mut market, 2, 2);

        let events =
            market.on_loss_event(Day(100), Peril::WindstormAtlantic, &full_damage_models(), &mut rng());
        assert_eq!(events.len(), 2, "one InsuredLoss per matching active policy");
        for (_, e) in &events {
            assert!(matches!(e, Event::InsuredLoss { peril: Peril::WindstormAtlantic, .. }));
        }
    }

    #[test]
    fn on_loss_event_only_hits_active_policies() {
        let mut market = Market::new();
        // Create a policy via on_quote_accepted but do NOT call on_policy_bound.
        market.on_quote_accepted(
            Day(0),
            SubmissionId(1),
            InsuredId(1),
            InsurerId(1),
            50_000,
            small_risk(),
            Year(1),
        );
        // Policy is pending, not active — should not appear in loss events.
        let events = market.on_loss_event(
            Day(100),
            Peril::WindstormAtlantic,
            &full_damage_models(),
            &mut rng(),
        );
        assert!(events.is_empty(), "pending (unbound) policy must not be loss-eligible");
    }

    #[test]
    fn on_loss_event_skips_non_matching_peril() {
        let mut market = Market::new();
        bind_policy(&mut market, 1, 1);
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
    fn aggregate_annual_gul_capped_at_sum_insured() {
        let mut market = Market::new();
        let pid = bind_policy(&mut market, 1, 1);
        let half = SMALL_ASSET_VALUE / 2 + 1;
        let e1 = market.on_insured_loss(Day(10), pid, half, Peril::WindstormAtlantic);
        let e2 = market.on_insured_loss(Day(20), pid, half, Peril::WindstormAtlantic);

        let total: u64 = e1
            .iter()
            .chain(e2.iter())
            .filter_map(|(_, e)| {
                if let Event::ClaimSettled { amount, .. } = e { Some(*amount) } else { None }
            })
            .sum();

        assert_eq!(total, SMALL_ASSET_VALUE, "aggregate annual GUL must not exceed sum_insured");
    }

    #[test]
    fn on_insured_loss_unknown_policy_produces_no_event() {
        let mut market = Market::new();
        let events = market.on_insured_loss(Day(0), PolicyId(999), 100_000, Peril::Attritional);
        assert!(events.is_empty(), "unknown policy_id must produce no events");
    }

    // ── on_quote_rejected ─────────────────────────────────────────────────────

    #[test]
    fn on_quote_rejected_is_noop() {
        let mut market = Market::new();
        market.on_quote_rejected(SubmissionId(99)); // must not panic
        assert!(market.policies.is_empty());
    }
}
