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
    /// The day the matching `PolicyExpired` event fires (= bound_day + 360).
    /// Used by `on_loss_event` to guard against the DES race where a `LossEvent`
    /// and `PolicyExpired` share the same day but the loss fires first.
    pub expire_day: Day,
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
    /// insured_id → (territory, sum_insured). Populated via register_insured() at CoverageRequested time.
    /// Used by on_loss_event to emit AssetDamage only for insureds in the struck territory.
    pub insured_registry: HashMap<InsuredId, (String, u64)>,
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
            insured_registry: HashMap::new(),
        }
    }

    /// Register an insured in the market registry. Called at `CoverageRequested` time.
    /// Idempotent — only the first call for each `insured_id` takes effect.
    pub fn register_insured(&mut self, insured_id: InsuredId, territory: &str, sum_insured: u64) {
        self.insured_registry.entry(insured_id).or_insert((territory.to_string(), sum_insured));
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

        let bind_day = day.offset(1);
        let expire_day = day.offset(361);
        let sum_insured = risk.sum_insured;

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
                expire_day,
            },
        );

        vec![
            (
                bind_day,
                Event::PolicyBound {
                    policy_id,
                    submission_id,
                    insured_id,
                    insurer_id,
                    premium,
                    sum_insured,
                    total_cat_exposure: 0, // back-filled by simulation after insurer.on_policy_bound
                },
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

    /// Insured rejected the quote. No market state changes required; renewal is
    /// scheduled by the simulation dispatcher after this call returns.
    pub fn on_quote_rejected(&mut self, _submission_id: SubmissionId) {}

    /// Remove a policy when its PolicyExpired event fires.
    pub fn on_policy_expired(&mut self, policy_id: PolicyId) {
        if let Some(policy) = self.policies.remove(&policy_id) {
            self.insured_active_policies.retain(|_, &mut pid| pid != policy_id);
            drop(policy); // silence dead-code warning on fields
        }
    }

    /// A catastrophe loss event has fired. Emit `AssetDamage` for every registered
    /// insured **in the matching territory**.
    ///
    /// A single damage fraction is drawn once for the entire event and applied to every
    /// affected insured. This reflects the physical reality: a cat event's intensity field
    /// (wind speed, ground motion) is a property of the occurrence, not of individual assets.
    /// Routing to `ClaimSettled` happens downstream in `on_asset_damage`.
    pub fn on_loss_event(
        &self,
        day: Day,
        peril: Peril,
        territory: &str,
        damage_models: &HashMap<Peril, DamageFractionModel>,
        rng: &mut impl Rng,
    ) -> Vec<(Day, Event)> {
        let Some(model) = damage_models.get(&peril) else {
            return vec![];
        };
        let df = model.sample(rng);
        self.insured_registry
            .iter()
            .filter(|(_, (t, _))| t.as_str() == territory)
            .filter_map(|(&insured_id, &(_, sum_insured))| {
                let gul = (df * sum_insured as f64) as u64;
                if gul == 0 {
                    return None;
                }
                Some((day, Event::AssetDamage { insured_id, peril, ground_up_loss: gul }))
            })
            .collect()
    }

    /// An `AssetDamage` event has fired for an insured. Routes to `ClaimSettled` only
    /// when the insured holds an active policy that covers the peril.
    /// Uninsured insureds (no active policy, policy expired, or peril not covered) generate
    /// no claim — the loss is counted in analysis but not passed to any insurer.
    pub fn on_asset_damage(
        &mut self,
        day: Day,
        insured_id: InsuredId,
        ground_up_loss: u64,
        peril: Peril,
    ) -> Vec<(Day, Event)> {
        // No active policy → uninsured; no claim.
        let Some(&policy_id) = self.insured_active_policies.get(&insured_id) else {
            return vec![];
        };
        let policy = match self.policies.get(&policy_id) {
            Some(p) => p,
            None => return vec![],
        };
        // expire_day race guard: policy covers [bound_day, expire_day).
        if day >= policy.expire_day {
            return vec![];
        }
        if !policy.risk.perils_covered.contains(&peril) {
            return vec![];
        }
        let insurer_id = policy.insurer_id;
        let sum_insured = policy.risk.sum_insured;

        let year = day.year();
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
            Event::ClaimSettled {
                policy_id,
                insurer_id,
                amount: effective_gul,
                peril,
                remaining_capital: 0, // back-filled by simulation after insurer.on_claim_settled
            },
        )]
    }

}

#[cfg(test)]
mod tests {
    use rand::SeedableRng;
    use rand_chacha::ChaCha20Rng;

    use super::*;
    use crate::config::ASSET_VALUE;
    use crate::perils::DamageFractionModel;

    fn rng() -> ChaCha20Rng {
        ChaCha20Rng::seed_from_u64(42)
    }

    fn small_risk() -> Risk {
        Risk {
            sum_insured: ASSET_VALUE,
            territory: "US-SE".to_string(),
            perils_covered: vec![Peril::WindstormAtlantic, Peril::Attritional],
        }
    }

    fn full_damage_models() -> HashMap<Peril, DamageFractionModel> {
        // Pareto(scale=1.0, shape=2.0) always produces values ≥ 1.0, clipped to 1.0.
        [(Peril::WindstormAtlantic, DamageFractionModel::Pareto { scale: 1.0, shape: 2.0, cap: 1.0 })]
            .into_iter()
            .collect()
    }

    /// Helper: register an insured, create an accepted + activated policy. Returns the PolicyId.
    fn bind_policy(market: &mut Market, submission_id: u64, insured_id: u64) -> PolicyId {
        let sid = SubmissionId(submission_id);
        let iid = InsuredId(insured_id);
        let insurer_id = InsurerId(1);
        // Register insured so on_loss_event emits AssetDamage for them.
        market.register_insured(iid, "US-SE", ASSET_VALUE);
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
    /// registered insureds. Two identical insureds in the same event must receive
    /// the same ground_up_loss. This test fails with per-insured draws.
    #[test]
    fn cat_loss_uses_shared_damage_fraction() {
        let mut market = Market::new();
        bind_policy(&mut market, 1, 1);
        bind_policy(&mut market, 2, 2);
        // Both insureds have ASSET_VALUE. Use a variable model (not the
        // degenerate Pareto(1,2) that always clips to 1.0).
        let models: HashMap<Peril, DamageFractionModel> = [(
            Peril::WindstormAtlantic,
            DamageFractionModel::LogNormal { mu: -3.0, sigma: 1.0 },
        )]
        .into_iter()
        .collect();
        let events = market.on_loss_event(Day(100), Peril::WindstormAtlantic, "US-SE", &models, &mut rng());
        assert_eq!(events.len(), 2);
        let guls: Vec<u64> = events
            .iter()
            .filter_map(|(_, e)| {
                if let Event::AssetDamage { ground_up_loss, .. } = e { Some(*ground_up_loss) }
                else { None }
            })
            .collect();
        assert_eq!(
            guls[0], guls[1],
            "all insureds in the same cat event must share the damage fraction"
        );
    }

    #[test]
    fn on_loss_event_emits_asset_damage_per_registered_insured() {
        let mut market = Market::new();
        bind_policy(&mut market, 1, 1);
        bind_policy(&mut market, 2, 2);

        let events =
            market.on_loss_event(Day(100), Peril::WindstormAtlantic, "US-SE", &full_damage_models(), &mut rng());
        assert_eq!(events.len(), 2, "one AssetDamage per registered insured");
        for (_, e) in &events {
            assert!(matches!(e, Event::AssetDamage { peril: Peril::WindstormAtlantic, .. }));
        }
    }

    #[test]
    fn on_loss_event_emits_asset_damage_regardless_of_expiry() {
        // on_loss_event fires for all registered insureds; expiry guard lives in on_asset_damage.
        let mut market = Market::new();
        // bind_policy uses Day(0), so expire_day = Day(361).
        bind_policy(&mut market, 1, 1);
        // Loss on expiry day: on_loss_event still emits AssetDamage (expiry is checked later).
        let events =
            market.on_loss_event(Day(361), Peril::WindstormAtlantic, "US-SE", &full_damage_models(), &mut rng());
        assert_eq!(events.len(), 1, "on_loss_event emits AssetDamage even on expiry day");
    }

    #[test]
    fn on_asset_damage_skips_claim_on_expiry_day() {
        // on_asset_damage guards: policy covers [bound_day, expire_day).
        // Loss on expire_day must not produce a ClaimSettled.
        let mut market = Market::new();
        bind_policy(&mut market, 1, 1); // bound at Day(1), expires at Day(361)
        let events =
            market.on_asset_damage(Day(361), InsuredId(1), ASSET_VALUE, Peril::WindstormAtlantic);
        assert!(events.is_empty(), "claim on expiry day must be skipped");

        // Loss one day before expiry must still produce a claim.
        let events =
            market.on_asset_damage(Day(360), InsuredId(1), ASSET_VALUE, Peril::WindstormAtlantic);
        assert_eq!(events.len(), 1, "claim one day before expiry must be emitted");
    }

    #[test]
    fn on_loss_event_no_events_for_unregistered_insured() {
        // Insured created a pending policy (quote accepted) but never called register_insured.
        let mut market = Market::new();
        market.on_quote_accepted(
            Day(0),
            SubmissionId(1),
            InsuredId(1),
            InsurerId(1),
            50_000,
            small_risk(),
            Year(1),
        );
        // Insured not registered → no AssetDamage.
        let events = market.on_loss_event(
            Day(100),
            Peril::WindstormAtlantic,
            "US-SE",
            &full_damage_models(),
            &mut rng(),
        );
        assert!(events.is_empty(), "unregistered insured must not receive AssetDamage");
    }

    #[test]
    fn on_loss_event_skips_non_matching_peril() {
        let mut market = Market::new();
        bind_policy(&mut market, 1, 1);
        let models: HashMap<Peril, DamageFractionModel> = HashMap::new();
        let events = market.on_loss_event(Day(100), Peril::WindstormAtlantic, "US-SE", &models, &mut rng());
        assert!(events.is_empty(), "no events when damage model is missing");
    }

    #[test]
    fn on_loss_event_ground_up_loss_le_sum_insured() {
        let mut market = Market::new();
        bind_policy(&mut market, 1, 1);
        let events =
            market.on_loss_event(Day(100), Peril::WindstormAtlantic, "US-SE", &full_damage_models(), &mut rng());
        for (_, e) in &events {
            if let Event::AssetDamage { ground_up_loss, .. } = e {
                assert!(
                    *ground_up_loss <= ASSET_VALUE,
                    "gul {ground_up_loss} > sum_insured {ASSET_VALUE}"
                );
            }
        }
    }

    /// Two insureds in US-SE with different sum_insured values. Using a model that
    /// always produces df=1.0 (Pareto scale=1.0), GUL must equal each insured's own SI.
    /// This confirms the shared damage fraction scales proportionally with sum_insured.
    #[test]
    fn cat_gul_proportional_to_sum_insured_same_territory() {
        let mut market = Market::new();
        let si_small = ASSET_VALUE;
        let si_large = ASSET_VALUE * 2;
        // register_insured directly — on_loss_event only needs insured_registry.
        market.register_insured(InsuredId(1), "US-SE", si_small);
        market.register_insured(InsuredId(2), "US-SE", si_large);

        // full_damage_models: Pareto(scale=1.0) → df always clips to 1.0.
        let events = market.on_loss_event(
            Day(100),
            Peril::WindstormAtlantic,
            "US-SE",
            &full_damage_models(),
            &mut rng(),
        );
        assert_eq!(events.len(), 2);
        let guls: HashMap<InsuredId, u64> = events
            .iter()
            .filter_map(|(_, e)| {
                if let Event::AssetDamage { insured_id, ground_up_loss, .. } = e {
                    Some((*insured_id, *ground_up_loss))
                } else {
                    None
                }
            })
            .collect();
        assert_eq!(
            guls[&InsuredId(1)], si_small,
            "small insured GUL must equal SI_small when df=1.0"
        );
        assert_eq!(
            guls[&InsuredId(2)], si_large,
            "large insured GUL must equal SI_large when df=1.0"
        );
    }

    /// A LossEvent striking a territory with no registered insureds must emit nothing.
    #[test]
    fn loss_event_to_empty_territory_emits_nothing() {
        let mut market = Market::new();
        market.register_insured(InsuredId(1), "US-SE", ASSET_VALUE);
        // Strike US-Gulf — no insureds there.
        let events = market.on_loss_event(
            Day(100),
            Peril::WindstormAtlantic,
            "US-Gulf",
            &full_damage_models(),
            &mut rng(),
        );
        assert!(
            events.is_empty(),
            "no AssetDamage when struck territory has no registered insureds"
        );
    }

    /// Three insureds in three different territories. Striking each territory in
    /// turn must produce exactly one AssetDamage for the matching insured.
    #[test]
    fn loss_event_strikes_correct_subset_across_three_territories() {
        let mut market = Market::new();
        let iid_ne = InsuredId(10);
        let iid_se = InsuredId(11);
        let iid_gulf = InsuredId(12);
        market.register_insured(iid_ne, "US-NE", ASSET_VALUE);
        market.register_insured(iid_se, "US-SE", ASSET_VALUE);
        market.register_insured(iid_gulf, "US-Gulf", ASSET_VALUE);

        for (territory, expected_iid) in [
            ("US-SE", iid_se),
            ("US-Gulf", iid_gulf),
            ("US-NE", iid_ne),
        ] {
            let events = market.on_loss_event(
                Day(100),
                Peril::WindstormAtlantic,
                territory,
                &full_damage_models(),
                &mut rng(),
            );
            assert_eq!(events.len(), 1, "territory {territory}: expected exactly 1 AssetDamage");
            match &events[0].1 {
                Event::AssetDamage { insured_id, .. } => {
                    assert_eq!(
                        *insured_id, expected_iid,
                        "territory {territory}: wrong insured hit"
                    );
                }
                e => panic!("expected AssetDamage, got {e:?}"),
            }
        }
    }

    #[test]
    fn on_loss_event_only_hits_insureds_in_matching_territory() {
        // Insured A in US-SE; insured B in US-NE.
        // A LossEvent striking US-SE must only emit AssetDamage for insured A.
        let mut market = Market::new();
        let iid_a = InsuredId(10);
        let iid_b = InsuredId(11);
        market.register_insured(iid_a, "US-SE", ASSET_VALUE);
        market.register_insured(iid_b, "US-NE", ASSET_VALUE);

        let events = market.on_loss_event(
            Day(100),
            Peril::WindstormAtlantic,
            "US-SE",
            &full_damage_models(),
            &mut rng(),
        );

        assert_eq!(events.len(), 1, "only insured A (US-SE) should be hit");
        if let (_, Event::AssetDamage { insured_id, .. }) = &events[0] {
            assert_eq!(*insured_id, iid_a, "AssetDamage must target insured A");
        } else {
            panic!("expected AssetDamage event");
        }
    }

    // ── on_asset_damage ───────────────────────────────────────────────────────

    #[test]
    fn on_asset_damage_returns_claim_settled() {
        let mut market = Market::new();
        bind_policy(&mut market, 1, 1);
        let events = market.on_asset_damage(Day(10), InsuredId(1), 100_000, Peril::WindstormAtlantic);
        assert_eq!(events.len(), 1);
        assert!(matches!(events[0].1, Event::ClaimSettled { amount: 100_000, .. }));
    }

    #[test]
    fn aggregate_annual_gul_capped_at_sum_insured() {
        let mut market = Market::new();
        bind_policy(&mut market, 1, 1);
        let half = ASSET_VALUE / 2 + 1;
        let e1 = market.on_asset_damage(Day(10), InsuredId(1), half, Peril::WindstormAtlantic);
        let e2 = market.on_asset_damage(Day(20), InsuredId(1), half, Peril::WindstormAtlantic);

        let total: u64 = e1
            .iter()
            .chain(e2.iter())
            .filter_map(|(_, e)| {
                if let Event::ClaimSettled { amount, .. } = e { Some(*amount) } else { None }
            })
            .sum();

        assert_eq!(total, ASSET_VALUE, "aggregate annual GUL must not exceed sum_insured");
    }

    #[test]
    fn on_asset_damage_unknown_insured_produces_no_event() {
        let mut market = Market::new();
        let events = market.on_asset_damage(Day(0), InsuredId(999), 100_000, Peril::Attritional);
        assert!(events.is_empty(), "unknown insured_id must produce no events");
    }

    #[test]
    fn on_asset_damage_uninsured_returns_empty() {
        // Insured is registered but has no active policy (SubmissionDropped / unbound).
        let mut market = Market::new();
        market.register_insured(InsuredId(1), "US-SE", ASSET_VALUE);
        let events = market.on_asset_damage(Day(10), InsuredId(1), 100_000, Peril::WindstormAtlantic);
        assert!(events.is_empty(), "uninsured insured must not generate a ClaimSettled");
    }

    #[test]
    fn on_asset_damage_peril_not_covered_returns_empty() {
        // Policy covers only WindstormAtlantic; Attritional damage must not generate a claim.
        let mut market = Market::new();
        let iid = InsuredId(1);
        market.register_insured(iid, "US-SE", ASSET_VALUE);
        let cat_only_risk = Risk {
            sum_insured: ASSET_VALUE,
            territory: "US-SE".to_string(),
            perils_covered: vec![Peril::WindstormAtlantic],
        };
        let events = market.on_quote_accepted(
            Day(0), SubmissionId(1), iid, InsurerId(1), 100_000, cat_only_risk, Year(1),
        );
        let pid = events
            .iter()
            .find_map(|(_, e)| if let Event::PolicyBound { policy_id, .. } = e { Some(*policy_id) } else { None })
            .unwrap();
        market.on_policy_bound(pid);
        let events = market.on_asset_damage(Day(10), iid, 100_000, Peril::Attritional);
        assert!(events.is_empty(), "peril not covered by policy must not generate a claim");
    }

    // ── on_quote_rejected ─────────────────────────────────────────────────────

    #[test]
    fn on_quote_rejected_is_noop() {
        let mut market = Market::new();
        market.on_quote_rejected(SubmissionId(99)); // must not panic
        assert!(market.policies.is_empty());
    }
}
