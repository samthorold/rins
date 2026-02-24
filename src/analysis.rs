use std::collections::{HashMap, HashSet};

use crate::{
    events::{Event, Peril, SimEvent},
    types::{InsuredId, InsurerId, PolicyId, SubmissionId},
};

/// Per-year aggregate statistics derived from the event stream.
#[derive(Debug, Clone)]
pub struct YearStats {
    pub year: u32,
    /// Sum of PolicyBound.premium in the year (cents).
    pub bound_premium: u64,
    /// Sum of PolicyBound.sum_insured in the year (cents).
    pub sum_insured: u64,
    /// Sum of ClaimSettled.amount in the year (cents).
    pub claims: u64,
    /// Sum of InsuredLoss.ground_up_loss where peril = Attritional (cents).
    pub attr_gul: u64,
    /// Sum of InsuredLoss.ground_up_loss where peril = WindstormAtlantic (cents).
    pub cat_gul: u64,
    /// Sum of last-known remaining_capital per insurer at year-end (cents).
    pub total_capital: u64,
    /// Count of InsurerInsolvent events in the year.
    pub insolvent_count: u32,
    /// Count of SubmissionDropped events in the year (all insurers declined).
    pub dropped_count: u32,
    /// Sum of unique-insured sum_insured from CoverageRequested in the year (cents).
    pub total_assets: u64,
    /// Count of WindstormAtlantic LossEvent firings in the year.
    pub cat_event_count: u32,
    /// Count of InsurerEntered events in the year.
    pub entrant_count: u32,
}

impl YearStats {
    fn zero(year: u32) -> Self {
        Self {
            year,
            bound_premium: 0,
            sum_insured: 0,
            claims: 0,
            attr_gul: 0,
            cat_gul: 0,
            total_capital: 0,
            insolvent_count: 0,
            dropped_count: 0,
            total_assets: 0,
            cat_event_count: 0,
            entrant_count: 0,
        }
    }

    /// Pure loss ratio: total claims / total bound premium. Zero if no premium.
    pub fn loss_ratio(&self) -> f64 {
        if self.bound_premium == 0 {
            0.0
        } else {
            self.claims as f64 / self.bound_premium as f64
        }
    }

    /// Market-wide rate on line: bound premium / sum insured. Zero if no exposure.
    pub fn rate_on_line(&self) -> f64 {
        if self.sum_insured == 0 {
            0.0
        } else {
            self.bound_premium as f64 / self.sum_insured as f64
        }
    }

    /// Combined ratio: loss ratio + expense ratio. Below 1.0 = underwriting profit.
    pub fn combined_ratio(&self, expense_ratio: f64) -> f64 {
        self.loss_ratio() + expense_ratio
    }

}

/// A mechanics invariant violation detected in the event stream.
#[derive(Debug)]
pub enum MechanicsViolation {
    /// PolicyBound did not arrive exactly 2 days after LeadQuoteRequested.
    DayOffsetChain { submission_id: u64, detail: String },
    /// AssetDamage arrived before the insured's first CoverageRequested (any peril).
    LossBeforeBound { insured_id: u64, loss_day: u64, bound_day: u64 },
    /// Attritional AssetDamage arrived on or before the insured's CoverageRequested day.
    AttrNotStrictlyPostBound { insured_id: u64, loss_day: u64, bound_day: u64 },
    /// PolicyExpired did not fire at QuoteAccepted_day + 361.
    PolicyExpiredTiming { policy_id: u64, expected: u64, actual: u64 },
    /// ClaimSettled arrived after the policy had expired.
    ClaimAfterExpiry { policy_id: u64, claim_day: u64, expiry_day: u64 },
    /// AssetDamage ground_up_loss exceeds the insured sum_insured (damage fraction > 1.0).
    CatFractionInconsistent { peril: String, day: u64, detail: String },
}


impl std::fmt::Display for MechanicsViolation {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::DayOffsetChain { submission_id, detail } => {
                write!(f, "DayOffsetChain sub={submission_id}: {detail}")
            }
            Self::LossBeforeBound { insured_id, loss_day, bound_day } => {
                write!(f, "LossBeforeBound insured={insured_id}: loss_day={loss_day} bound_day={bound_day}")
            }
            Self::AttrNotStrictlyPostBound { insured_id, loss_day, bound_day } => {
                write!(f, "AttrNotStrictlyPostBound insured={insured_id}: loss_day={loss_day} bound_day={bound_day}")
            }
            Self::PolicyExpiredTiming { policy_id, expected, actual } => {
                write!(f, "PolicyExpiredTiming policy={policy_id}: expected={expected} actual={actual}")
            }
            Self::ClaimAfterExpiry { policy_id, claim_day, expiry_day } => {
                write!(f, "ClaimAfterExpiry policy={policy_id}: claim_day={claim_day} expiry_day={expiry_day}")
            }
            Self::CatFractionInconsistent { peril, day, detail } => {
                write!(f, "CatFractionInconsistent peril={peril} day={day}: {detail}")
            }
        }
    }
}

/// Compute per-year statistics from a typed event slice.
///
/// `initial_capitals` seeds each insurer's capital before any ClaimSettled is seen.
/// Warmup years are read from the SimulationStart event; years ≤ warmup_years are excluded
/// from the returned Vec.
///
/// `_expense_ratio` is accepted for API symmetry; callers use `YearStats::combined_ratio`
/// to apply it when rendering output.
pub fn analyse(
    events: &[SimEvent],
    initial_capitals: &HashMap<InsurerId, u64>,
    _expense_ratio: f64,
) -> (u32, Vec<YearStats>) {
    let warmup_years = events
        .iter()
        .find_map(|e| {
            if let Event::SimulationStart { warmup_years, .. } = &e.event {
                Some(*warmup_years)
            } else {
                None
            }
        })
        .unwrap_or(0);

    let mut stats: HashMap<u32, YearStats> = HashMap::new();
    let mut last_capital: HashMap<InsurerId, u64> = initial_capitals.clone();
    let mut assets_seen: HashMap<u32, HashSet<InsuredId>> = HashMap::new();

    for sim_event in events {
        let year = sim_event.day.year().0;

        match &sim_event.event {
            Event::PolicyBound { premium, sum_insured, .. } => {
                let s = stats.entry(year).or_insert_with(|| YearStats::zero(year));
                s.bound_premium += premium;
                s.sum_insured += sum_insured;
            }
            Event::ClaimSettled { insurer_id, amount, remaining_capital, .. } => {
                last_capital.insert(*insurer_id, *remaining_capital);
                let s = stats.entry(year).or_insert_with(|| YearStats::zero(year));
                s.claims += amount;
            }
            Event::AssetDamage { peril, ground_up_loss, .. } => {
                let s = stats.entry(year).or_insert_with(|| YearStats::zero(year));
                match peril {
                    Peril::Attritional => s.attr_gul += ground_up_loss,
                    Peril::WindstormAtlantic => s.cat_gul += ground_up_loss,
                }
            }
            Event::InsurerInsolvent { .. } => {
                let s = stats.entry(year).or_insert_with(|| YearStats::zero(year));
                s.insolvent_count += 1;
            }
            Event::SubmissionDropped { .. } => {
                let s = stats.entry(year).or_insert_with(|| YearStats::zero(year));
                s.dropped_count += 1;
            }
            Event::LossEvent { peril: Peril::WindstormAtlantic, .. } => {
                let s = stats.entry(year).or_insert_with(|| YearStats::zero(year));
                s.cat_event_count += 1;
            }
            Event::InsurerEntered { insurer_id, initial_capital, .. } => {
                last_capital.insert(*insurer_id, *initial_capital);
                let s = stats.entry(year).or_insert_with(|| YearStats::zero(year));
                s.entrant_count += 1;
            }
            Event::CoverageRequested { insured_id, risk } => {
                let seen = assets_seen.entry(year).or_default();
                if seen.insert(*insured_id) {
                    let s = stats.entry(year).or_insert_with(|| YearStats::zero(year));
                    s.total_assets += risk.sum_insured;
                }
            }
            Event::YearEnd { year: y } => {
                // Snapshot total capital at year boundary.
                let total_cap: u64 = last_capital.values().sum();
                let s = stats.entry(y.0).or_insert_with(|| YearStats::zero(y.0));
                s.total_capital = total_cap;
            }
            _ => {}
        }
    }

    let mut result: Vec<YearStats> =
        stats.into_values().filter(|s| s.year > warmup_years).collect();
    result.sort_by_key(|s| s.year);
    (warmup_years, result)
}

/// Check all 6 mechanics invariants. Returns one item per violation found.
pub fn verify_mechanics(events: &[SimEvent]) -> Vec<MechanicsViolation> {
    let mut violations: Vec<MechanicsViolation> = Vec::new();

    // Per-submission tracking for the quoting chain and expiry timing.
    let mut lqr_day: HashMap<SubmissionId, u64> = HashMap::new();
    let mut qa_day: HashMap<SubmissionId, u64> = HashMap::new();

    // Per-policy tracking.
    let mut policy_from_sub: HashMap<SubmissionId, PolicyId> = HashMap::new();
    let mut expiry_day: HashMap<PolicyId, u64> = HashMap::new();

    // Per-insured tracking: first CoverageRequested day + sum_insured.
    let mut insured_cr_day: HashMap<InsuredId, u64> = HashMap::new();
    let mut insured_sum_insured: HashMap<InsuredId, u64> = HashMap::new();

    // First pass: index LeadQuoteRequested, QuoteAccepted, PolicyBound, PolicyExpired,
    // and CoverageRequested (for loss timing checks).
    for ev in events {
        let day = ev.day.0;
        match &ev.event {
            Event::CoverageRequested { insured_id, risk } => {
                insured_cr_day.entry(*insured_id).or_insert(day);
                insured_sum_insured.entry(*insured_id).or_insert(risk.sum_insured);
            }
            Event::LeadQuoteRequested { submission_id, .. } => {
                lqr_day.entry(*submission_id).or_insert(day);
            }
            Event::QuoteAccepted { submission_id, .. } => {
                qa_day.insert(*submission_id, day);
            }
            Event::PolicyBound { policy_id, submission_id, .. } => {
                policy_from_sub.insert(*submission_id, *policy_id);

                // Invariant 1 — DayOffsetChain: PolicyBound must be lqr_day + 2.
                if let Some(&lqr) = lqr_day.get(submission_id) {
                    let expected = lqr + 2;
                    if day != expected {
                        violations.push(MechanicsViolation::DayOffsetChain {
                            submission_id: submission_id.0,
                            detail: format!(
                                "PolicyBound at day {day}, expected {expected} (LeadQuoteRequested at {lqr})"
                            ),
                        });
                    }
                }
            }
            Event::PolicyExpired { policy_id } => {
                expiry_day.insert(*policy_id, day);
            }
            _ => {}
        }
    }

    // Check PolicyExpiredTiming: expected = qa_day + 361.
    for (sub_id, pid) in &policy_from_sub {
        if let (Some(&qa), Some(&actual)) = (qa_day.get(sub_id), expiry_day.get(pid)) {
            let expected = qa + 361;
            if actual != expected {
                violations.push(MechanicsViolation::PolicyExpiredTiming {
                    policy_id: pid.0,
                    expected,
                    actual,
                });
            }
        }
    }

    // Second pass: check loss and claim timing.
    for ev in events {
        let day = ev.day.0;
        match &ev.event {
            Event::AssetDamage { insured_id, peril, ground_up_loss } => {
                if let Some(&cr_day) = insured_cr_day.get(insured_id) {
                    // Invariant 2 — LossBeforeBound: AssetDamage must not fire before the
                    // insured's first CoverageRequested (losses are scheduled from that day).
                    if day < cr_day {
                        violations.push(MechanicsViolation::LossBeforeBound {
                            insured_id: insured_id.0,
                            loss_day: day,
                            bound_day: cr_day,
                        });
                    }
                    // Invariant 3 — AttrNotStrictlyPostBound: attritional loss must be strictly
                    // after CoverageRequested day (scheduled in (from_day, year_end]).
                    if matches!(peril, Peril::Attritional) && day <= cr_day {
                        violations.push(MechanicsViolation::AttrNotStrictlyPostBound {
                            insured_id: insured_id.0,
                            loss_day: day,
                            bound_day: cr_day,
                        });
                    }
                }
                // Invariant 6 — CatFractionInconsistent: ground_up_loss must not exceed sum_insured.
                if matches!(peril, Peril::WindstormAtlantic) {
                    if let Some(&si) = insured_sum_insured.get(insured_id) {
                        if *ground_up_loss > si {
                            violations.push(MechanicsViolation::CatFractionInconsistent {
                                peril: "WindstormAtlantic".to_string(),
                                day,
                                detail: format!(
                                    "insured {} gul {} > sum_insured {}",
                                    insured_id.0, ground_up_loss, si
                                ),
                            });
                        }
                    }
                }
            }
            Event::ClaimSettled { policy_id, .. } => {
                // Invariant 5 — ClaimAfterExpiry: claim must not arrive after policy expiry.
                if let Some(&exp) = expiry_day.get(policy_id) {
                    if day > exp {
                        violations.push(MechanicsViolation::ClaimAfterExpiry {
                            policy_id: policy_id.0,
                            claim_day: day,
                            expiry_day: exp,
                        });
                    }
                }
            }
            _ => {}
        }
    }

    violations
}

/// A structural integrity violation detected in the event stream.
///
/// These are universal truths that must hold for any valid simulation run:
/// claim amounts, routing consistency, and bind-flow completeness.
#[derive(Debug)]
pub enum IntegrityViolation {
    // From verify_claims.py
    GulExceedsSumInsured { policy_id: u64, day: u64, peril: String, gul: u64, sum_insured: u64 },
    AggregateClaimExceedsSumInsured { policy_id: u64, year: u32, aggregate: u64, sum_insured: u64 },
    ClaimWithoutMatchingLoss { policy_id: u64, day: u64 },
    // From verify_insolvency.py
    ClaimAmountZero { policy_id: u64, day: u64 },
    ClaimInsurerMismatch { policy_id: u64, day: u64, claim_insurer: u64, bound_insurer: u64 },
    // From verify_panel_integrity.py
    QuoteAcceptedWithoutPolicyBound { submission_id: u64, accepted_day: u64 },
    PolicyBoundInsurerMismatch { submission_id: u64, policy_id: u64, bound_insurer: u64, accepted_insurer: u64 },
    DuplicatePolicyBound { policy_id: u64 },
    PolicyExpiredWithoutBound { policy_id: u64 },
    /// Inv 16 — LeadQuoteRequested with no insurer response.
    LeadQuoteOrphanRequest { submission_id: u64, insurer_id: u64, day: u64 },
    /// Inv 17 — (submission_id, insurer_id) received more than one insurer response.
    LeadQuoteDuplicateResponse { submission_id: u64, insurer_id: u64, count: u32 },
    /// Inv 18 — LeadQuoteIssued or LeadQuoteDeclined without a prior LeadQuoteRequested.
    LeadQuoteOrphanResponse { submission_id: u64, insurer_id: u64, day: u64, kind: String },
}

impl std::fmt::Display for IntegrityViolation {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::GulExceedsSumInsured { policy_id, day, peril, gul, sum_insured } => {
                write!(f, "GulExceedsSumInsured policy={policy_id} day={day} peril={peril} gul={gul} sum_insured={sum_insured}")
            }
            Self::AggregateClaimExceedsSumInsured { policy_id, year, aggregate, sum_insured } => {
                write!(f, "AggregateClaimExceedsSumInsured policy={policy_id} year={year} aggregate={aggregate} sum_insured={sum_insured}")
            }
            Self::ClaimWithoutMatchingLoss { policy_id, day } => {
                write!(f, "ClaimWithoutMatchingLoss policy={policy_id} day={day}")
            }
            Self::ClaimAmountZero { policy_id, day } => {
                write!(f, "ClaimAmountZero policy={policy_id} day={day}")
            }
            Self::ClaimInsurerMismatch { policy_id, day, claim_insurer, bound_insurer } => {
                write!(f, "ClaimInsurerMismatch policy={policy_id} day={day} claim_insurer={claim_insurer} bound_insurer={bound_insurer}")
            }
            Self::QuoteAcceptedWithoutPolicyBound { submission_id, accepted_day } => {
                write!(f, "QuoteAcceptedWithoutPolicyBound sub={submission_id} accepted_day={accepted_day}")
            }
            Self::PolicyBoundInsurerMismatch { submission_id, policy_id, bound_insurer, accepted_insurer } => {
                write!(f, "PolicyBoundInsurerMismatch sub={submission_id} policy={policy_id} bound_insurer={bound_insurer} accepted_insurer={accepted_insurer}")
            }
            Self::DuplicatePolicyBound { policy_id } => {
                write!(f, "DuplicatePolicyBound policy={policy_id}")
            }
            Self::PolicyExpiredWithoutBound { policy_id } => {
                write!(f, "PolicyExpiredWithoutBound policy={policy_id}")
            }
            Self::LeadQuoteOrphanRequest { submission_id, insurer_id, day } => {
                write!(f, "LeadQuoteOrphanRequest sub={submission_id} insurer={insurer_id} day={day}")
            }
            Self::LeadQuoteDuplicateResponse { submission_id, insurer_id, count } => {
                write!(f, "LeadQuoteDuplicateResponse sub={submission_id} insurer={insurer_id} count={count}")
            }
            Self::LeadQuoteOrphanResponse { submission_id, insurer_id, day, kind } => {
                write!(f, "LeadQuoteOrphanResponse sub={submission_id} insurer={insurer_id} day={day} kind={kind}")
            }
        }
    }
}

/// Check all 12 structural integrity invariants. Returns one item per violation found.
pub fn verify_integrity(events: &[SimEvent]) -> Vec<IntegrityViolation> {
    // ── Index pass ────────────────────────────────────────────────────────────
    let mut max_day: u64 = 0;
    let mut policy_sum_insured: HashMap<PolicyId, u64> = HashMap::new();
    let mut policy_insurer: HashMap<PolicyId, InsurerId> = HashMap::new();
    let mut policy_insured: HashMap<PolicyId, InsuredId> = HashMap::new();
    let mut insured_sum_insured: HashMap<InsuredId, u64> = HashMap::new();
    let mut sub_insurer_quoted: HashMap<SubmissionId, InsurerId> = HashMap::new();
    let mut sub_accepted_day: HashMap<SubmissionId, u64> = HashMap::new();
    let mut sub_policy: HashMap<SubmissionId, PolicyId> = HashMap::new();
    let mut policy_bind_count: HashMap<PolicyId, u32> = HashMap::new();
    let mut bound_policies: HashSet<PolicyId> = HashSet::new();
    let mut loss_keys: HashSet<(u64, InsuredId)> = HashSet::new();
    let mut claim_agg: HashMap<(PolicyId, u32), u64> = HashMap::new();
    let mut claim_settled_list: Vec<(u64, PolicyId, InsurerId, u64)> = Vec::new();
    // Quoting flow tracking for Inv 16–18.
    let mut lead_requested: HashMap<(SubmissionId, InsurerId), u64> = HashMap::new();
    let mut lead_responses: HashMap<(SubmissionId, InsurerId), u32> = HashMap::new();
    let mut orphan_responses: Vec<(SubmissionId, InsurerId, u64, String)> = Vec::new();

    for ev in events {
        let day = ev.day.0;
        if day > max_day {
            max_day = day;
        }
        match &ev.event {
            Event::CoverageRequested { insured_id, risk } => {
                insured_sum_insured.entry(*insured_id).or_insert(risk.sum_insured);
            }
            Event::QuoteAccepted { submission_id, insurer_id, .. } => {
                sub_accepted_day.insert(*submission_id, day);
                // Track the insurer whose quote was accepted — this is the correct reference for
                // PolicyBoundInsurerMismatch. With multi-insurer solicitation, multiple
                // LeadQuoteIssued events share a submission_id; only QuoteAccepted identifies the
                // selected insurer unambiguously.
                sub_insurer_quoted.insert(*submission_id, *insurer_id);
            }
            Event::PolicyBound { policy_id, submission_id, insurer_id, insured_id, sum_insured, .. } => {
                policy_sum_insured.insert(*policy_id, *sum_insured);
                policy_insurer.insert(*policy_id, *insurer_id);
                policy_insured.insert(*policy_id, *insured_id);
                sub_policy.insert(*submission_id, *policy_id);
                *policy_bind_count.entry(*policy_id).or_insert(0) += 1;
                bound_policies.insert(*policy_id);
            }
            Event::AssetDamage { insured_id, .. } => {
                loss_keys.insert((day, *insured_id));
            }
            Event::ClaimSettled { policy_id, insurer_id, amount, .. } => {
                let year = ev.day.year().0;
                *claim_agg.entry((*policy_id, year)).or_insert(0) += amount;
                claim_settled_list.push((day, *policy_id, *insurer_id, *amount));
            }
            Event::LeadQuoteRequested { submission_id, insurer_id, .. } => {
                lead_requested.entry((*submission_id, *insurer_id)).or_insert(day);
            }
            Event::LeadQuoteIssued { submission_id, insurer_id, .. } => {
                if !lead_requested.contains_key(&(*submission_id, *insurer_id)) {
                    orphan_responses.push((*submission_id, *insurer_id, day, "LeadQuoteIssued".to_string()));
                }
                *lead_responses.entry((*submission_id, *insurer_id)).or_insert(0) += 1;
            }
            Event::LeadQuoteDeclined { submission_id, insurer_id, .. } => {
                if !lead_requested.contains_key(&(*submission_id, *insurer_id)) {
                    orphan_responses.push((*submission_id, *insurer_id, day, "LeadQuoteDeclined".to_string()));
                }
                *lead_responses.entry((*submission_id, *insurer_id)).or_insert(0) += 1;
            }
            _ => {}
        }
    }

    let mut violations: Vec<IntegrityViolation> = Vec::new();

    // ── Claims (3) ────────────────────────────────────────────────────────────

    // Check 1: GulExceedsSumInsured — gul must not exceed sum_insured for any peril.
    for ev in events {
        if let Event::AssetDamage { insured_id, peril, ground_up_loss } = &ev.event {
            if let Some(&si) = insured_sum_insured.get(insured_id) {
                if *ground_up_loss > si {
                    violations.push(IntegrityViolation::GulExceedsSumInsured {
                        policy_id: insured_id.0, // field repurposed as insured_id for backwards compat
                        day: ev.day.0,
                        peril: format!("{peril:?}"),
                        gul: *ground_up_loss,
                        sum_insured: si,
                    });
                }
            }
        }
    }

    // Check 2: AggregateClaimExceedsSumInsured — sum of claims per (policy, year) ≤ sum_insured.
    for ((policy_id, year), &agg) in &claim_agg {
        if let Some(&si) = policy_sum_insured.get(policy_id) {
            if agg > si {
                violations.push(IntegrityViolation::AggregateClaimExceedsSumInsured {
                    policy_id: policy_id.0,
                    year: *year,
                    aggregate: agg,
                    sum_insured: si,
                });
            }
        }
    }

    // Check 3 (Claims), 4 (Routing), 5 (Routing) — iterate ClaimSettled.
    for &(day, policy_id, insurer_id, amount) in &claim_settled_list {
        // ClaimWithoutMatchingLoss: every ClaimSettled must have a matching AssetDamage.
        // AssetDamage is keyed by (day, insured_id); look up insured_id via policy_insured.
        let has_matching_loss = policy_insured
            .get(&policy_id)
            .map(|insured_id| loss_keys.contains(&(day, *insured_id)))
            .unwrap_or(false);
        if !has_matching_loss {
            violations.push(IntegrityViolation::ClaimWithoutMatchingLoss {
                policy_id: policy_id.0,
                day,
            });
        }
        // ClaimAmountZero: claim amount must be positive.
        if amount == 0 {
            violations.push(IntegrityViolation::ClaimAmountZero {
                policy_id: policy_id.0,
                day,
            });
        }
        // ClaimInsurerMismatch: claim must be paid by the insurer who bound the policy.
        if let Some(&bound_insurer) = policy_insurer.get(&policy_id) {
            if insurer_id != bound_insurer {
                violations.push(IntegrityViolation::ClaimInsurerMismatch {
                    policy_id: policy_id.0,
                    day,
                    claim_insurer: insurer_id.0,
                    bound_insurer: bound_insurer.0,
                });
            }
        }
    }

    // ── Bind Flow (4) ─────────────────────────────────────────────────────────

    // Check 6: QuoteAcceptedWithoutPolicyBound — every non-final-day accepted quote binds.
    for (&sub_id, &acc_day) in &sub_accepted_day {
        if acc_day < max_day && !sub_policy.contains_key(&sub_id) {
            violations.push(IntegrityViolation::QuoteAcceptedWithoutPolicyBound {
                submission_id: sub_id.0,
                accepted_day: acc_day,
            });
        }
    }

    // Check 7: PolicyBoundInsurerMismatch — bound insurer must match the insurer who quoted.
    for (&sub_id, &policy_id) in &sub_policy {
        if let (Some(&quoted), Some(&bound)) =
            (sub_insurer_quoted.get(&sub_id), policy_insurer.get(&policy_id))
        {
            if quoted != bound {
                violations.push(IntegrityViolation::PolicyBoundInsurerMismatch {
                    submission_id: sub_id.0,
                    policy_id: policy_id.0,
                    bound_insurer: bound.0,
                    accepted_insurer: quoted.0,
                });
            }
        }
    }

    // Check 8: DuplicatePolicyBound — each policy_id must bind exactly once.
    for (&policy_id, &count) in &policy_bind_count {
        if count > 1 {
            violations.push(IntegrityViolation::DuplicatePolicyBound {
                policy_id: policy_id.0,
            });
        }
    }

    // Check 9: PolicyExpiredWithoutBound — every PolicyExpired must reference a bound policy.
    for ev in events {
        if let Event::PolicyExpired { policy_id } = &ev.event {
            if !bound_policies.contains(policy_id) {
                violations.push(IntegrityViolation::PolicyExpiredWithoutBound {
                    policy_id: policy_id.0,
                });
            }
        }
    }

    // ── Quoting Flow (3) ──────────────────────────────────────────────────────

    // Check 10 (Inv 16): LeadQuoteOrphanRequest — every request must have a response.
    for (&(sub_id, ins_id), &req_day) in &lead_requested {
        if !lead_responses.contains_key(&(sub_id, ins_id)) {
            violations.push(IntegrityViolation::LeadQuoteOrphanRequest {
                submission_id: sub_id.0,
                insurer_id: ins_id.0,
                day: req_day,
            });
        }
    }

    // Check 11 (Inv 17): LeadQuoteDuplicateResponse — at most one response per (sub, ins).
    for (&(sub_id, ins_id), &count) in &lead_responses {
        if count > 1 {
            violations.push(IntegrityViolation::LeadQuoteDuplicateResponse {
                submission_id: sub_id.0,
                insurer_id: ins_id.0,
                count,
            });
        }
    }

    // Check 12 (Inv 18): LeadQuoteOrphanResponse — every response needs a prior request.
    for (sub_id, ins_id, orphan_day, kind) in orphan_responses {
        violations.push(IntegrityViolation::LeadQuoteOrphanResponse {
            submission_id: sub_id.0,
            insurer_id: ins_id.0,
            day: orphan_day,
            kind,
        });
    }

    violations
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        events::{Event, Peril, Risk, SimEvent},
        types::{Day, InsuredId, InsurerId, PolicyId, SubmissionId, Year},
    };

    fn sim_ev(day: u64, event: Event) -> SimEvent {
        SimEvent { day: Day(day), event }
    }

    fn dummy_risk() -> Risk {
        Risk {
            sum_insured: 1_000,
            territory: "US-SE".to_string(),
            perils_covered: vec![Peril::WindstormAtlantic, Peril::Attritional],
        }
    }

    fn sim_start() -> SimEvent {
        sim_ev(0, Event::SimulationStart { year_start: Year(1), warmup_years: 0, analysis_years: 1 })
    }

    fn empty_capitals() -> HashMap<InsurerId, u64> {
        HashMap::new()
    }

    // ── YearStats unit tests ──────────────────────────────────────────────────

    #[test]
    fn test_quiet_year_zero_claims() {
        let events = vec![
            sim_start(),
            sim_ev(
                10,
                Event::PolicyBound {
                    policy_id: PolicyId(1),
                    submission_id: SubmissionId(1),
                    insured_id: InsuredId(1),
                    insurer_id: InsurerId(1),
                    premium: 100,
                    sum_insured: 1_000,
                    total_cat_exposure: 1_000,
                },
            ),
            sim_ev(359, Event::YearEnd { year: Year(1) }),
        ];
        let (_, stats) = analyse(&events, &empty_capitals(), 0.344);
        assert_eq!(stats.len(), 1);
        assert_eq!(stats[0].year, 1);
        assert_eq!(stats[0].claims, 0);
        assert!((stats[0].loss_ratio() - 0.0).abs() < f64::EPSILON);
    }

    #[test]
    fn test_loss_ratio_exact() {
        let events = vec![
            sim_start(),
            sim_ev(
                10,
                Event::PolicyBound {
                    policy_id: PolicyId(1),
                    submission_id: SubmissionId(1),
                    insured_id: InsuredId(1),
                    insurer_id: InsurerId(1),
                    premium: 100,
                    sum_insured: 1_000,
                    total_cat_exposure: 1_000,
                },
            ),
            sim_ev(
                50,
                Event::ClaimSettled {
                    policy_id: PolicyId(1),
                    insurer_id: InsurerId(1),
                    amount: 50,
                    peril: Peril::WindstormAtlantic,
                    remaining_capital: 950,
                },
            ),
            sim_ev(359, Event::YearEnd { year: Year(1) }),
        ];
        let (_, stats) = analyse(&events, &empty_capitals(), 0.344);
        assert_eq!(stats.len(), 1);
        assert!((stats[0].loss_ratio() - 0.5).abs() < 1e-10);
    }

    #[test]
    fn test_rate_on_line_exact() {
        let events = vec![
            sim_start(),
            sim_ev(
                10,
                Event::PolicyBound {
                    policy_id: PolicyId(1),
                    submission_id: SubmissionId(1),
                    insured_id: InsuredId(1),
                    insurer_id: InsurerId(1),
                    premium: 100,
                    sum_insured: 1_000,
                    total_cat_exposure: 1_000,
                },
            ),
            sim_ev(359, Event::YearEnd { year: Year(1) }),
        ];
        let (_, stats) = analyse(&events, &empty_capitals(), 0.344);
        assert!((stats[0].rate_on_line() - 0.10).abs() < 1e-10);
    }

    #[test]
    fn test_cat_event_count() {
        // Two LossEvent(WindstormAtlantic) in year 1 → cat_event_count = 2.
        // Attritional AssetDamage must not increment cat_event_count.
        let events = vec![
            sim_start(),
            sim_ev(50, Event::LossEvent { event_id: 1, peril: Peril::WindstormAtlantic }),
            sim_ev(80, Event::LossEvent { event_id: 2, peril: Peril::WindstormAtlantic }),
            sim_ev(
                80,
                Event::AssetDamage {
                    insured_id: InsuredId(1),
                    peril: Peril::Attritional,
                    ground_up_loss: 500,
                },
            ),
            sim_ev(359, Event::YearEnd { year: Year(1) }),
        ];
        let (_, stats) = analyse(&events, &empty_capitals(), 0.344);
        assert_eq!(stats[0].cat_event_count, 2);
    }

    #[test]
    fn test_capital_carry_forward() {
        // ClaimSettled in year 1 reduces capital to 800.
        // Year 2 has no claims → total_capital should still be 800 (carried forward).
        let mut initials = HashMap::new();
        initials.insert(InsurerId(1), 1_000u64);

        let events = vec![
            sim_start(),
            sim_ev(
                50,
                Event::ClaimSettled {
                    policy_id: PolicyId(1),
                    insurer_id: InsurerId(1),
                    amount: 200,
                    peril: Peril::WindstormAtlantic,
                    remaining_capital: 800,
                },
            ),
            sim_ev(359, Event::YearEnd { year: Year(1) }),
            sim_ev(719, Event::YearEnd { year: Year(2) }),
        ];
        let (_, stats) = analyse(&events, &initials, 0.344);
        // Both years should appear (warmup_years=0).
        let y1 = stats.iter().find(|s| s.year == 1).expect("year 1 missing");
        let y2 = stats.iter().find(|s| s.year == 2).expect("year 2 missing");
        assert_eq!(y1.total_capital, 800);
        assert_eq!(y2.total_capital, 800, "capital must carry forward when no new claims");
    }

    #[test]
    fn test_insolvent_counted_per_year() {
        let events = vec![
            sim_start(),
            // Year 1 and 2 via YearEnd.
            sim_ev(359, Event::YearEnd { year: Year(1) }),
            sim_ev(719, Event::YearEnd { year: Year(2) }),
            // InsurerInsolvent in year 3 (day 720..1079).
            sim_ev(800, Event::InsurerInsolvent { insurer_id: InsurerId(1) }),
            sim_ev(1079, Event::YearEnd { year: Year(3) }),
            // Year 4 — no insolvent events.
            sim_ev(1439, Event::YearEnd { year: Year(4) }),
        ];
        let (_, stats) = analyse(&events, &empty_capitals(), 0.344);
        let y3 = stats.iter().find(|s| s.year == 3).expect("year 3 missing");
        let y4 = stats.iter().find(|s| s.year == 4).expect("year 4 missing");
        assert_eq!(y3.insolvent_count, 1);
        assert_eq!(y4.insolvent_count, 0);
    }

    #[test]
    fn test_warmup_years_excluded() {
        // SimulationStart with warmup_years=2 → years 1 and 2 must be absent.
        let events = vec![
            sim_ev(
                0,
                Event::SimulationStart {
                    year_start: Year(1),
                    warmup_years: 2,
                    analysis_years: 2,
                },
            ),
            sim_ev(359, Event::YearEnd { year: Year(1) }),
            sim_ev(719, Event::YearEnd { year: Year(2) }),
            sim_ev(
                800,
                Event::PolicyBound {
                    policy_id: PolicyId(1),
                    submission_id: SubmissionId(1),
                    insured_id: InsuredId(1),
                    insurer_id: InsurerId(1),
                    premium: 100,
                    sum_insured: 1_000,
                    total_cat_exposure: 1_000,
                },
            ),
            sim_ev(1079, Event::YearEnd { year: Year(3) }),
        ];
        let (warmup, stats) = analyse(&events, &empty_capitals(), 0.344);
        assert_eq!(warmup, 2);
        assert!(stats.iter().all(|s| s.year > 2), "warmup years must be excluded");
        assert!(stats.iter().any(|s| s.year == 3), "year 3 must be present");
    }

    // ── Mechanics invariant tests ─────────────────────────────────────────────

    /// Build a valid quoting chain (CoverageRequested → PolicyBound = 3 days).
    fn valid_chain_events(
        submission_id: SubmissionId,
        policy_id: PolicyId,
        base_day: u64,
    ) -> Vec<SimEvent> {
        vec![
            sim_ev(
                base_day,
                Event::CoverageRequested { insured_id: InsuredId(1), risk: dummy_risk() },
            ),
            sim_ev(
                base_day + 1,
                Event::LeadQuoteRequested {
                    submission_id,
                    insured_id: InsuredId(1),
                    insurer_id: InsurerId(1),
                    risk: dummy_risk(),
                },
            ),
            sim_ev(
                base_day + 1,
                Event::LeadQuoteIssued {
                    submission_id,
                    insured_id: InsuredId(1),
                    insurer_id: InsurerId(1),
                    atp: 100,
                    premium: 105,
                    cat_exposure_at_quote: 0,
                },
            ),
            sim_ev(
                base_day + 2,
                Event::QuotePresented {
                    submission_id,
                    insured_id: InsuredId(1),
                    insurer_id: InsurerId(1),
                    premium: 105,
                },
            ),
            sim_ev(
                base_day + 2,
                Event::QuoteAccepted {
                    submission_id,
                    insured_id: InsuredId(1),
                    insurer_id: InsurerId(1),
                    premium: 105,
                },
            ),
            sim_ev(
                base_day + 3,
                Event::PolicyBound {
                    policy_id,
                    submission_id,
                    insured_id: InsuredId(1),
                    insurer_id: InsurerId(1),
                    premium: 105,
                    sum_insured: 1_000,
                    total_cat_exposure: 1_000,
                },
            ),
            // PolicyExpired = QuoteAccepted_day + 361 = (base+2) + 361 = base+363
            sim_ev(base_day + 363, Event::PolicyExpired { policy_id }),
        ]
    }

    #[test]
    fn test_mechanics_offset_pass() {
        let events = valid_chain_events(SubmissionId(1), PolicyId(1), 0);
        let violations = verify_mechanics(&events);
        assert!(
            violations.is_empty(),
            "valid chain must produce no violations, got: {violations:?}"
        );
    }

    #[test]
    fn test_mechanics_offset_fail() {
        // PolicyBound arrives one day early (base+2 instead of base+3).
        let base_day = 0u64;
        let submission_id = SubmissionId(1);
        let policy_id = PolicyId(1);

        let mut events = valid_chain_events(submission_id, policy_id, base_day);
        // Replace the PolicyBound event with one that is one day early.
        let pb_idx = events
            .iter()
            .position(|e| matches!(e.event, Event::PolicyBound { .. }))
            .expect("PolicyBound missing");
        let early_bound = Event::PolicyBound {
            policy_id,
            submission_id,
            insured_id: InsuredId(1),
            insurer_id: InsurerId(1),
            premium: 105,
            sum_insured: 1_000,
            total_cat_exposure: 1_000,
        };
        events[pb_idx] = sim_ev(base_day + 2, early_bound); // one day early

        let violations = verify_mechanics(&events);
        assert!(
            violations.iter().any(|v| matches!(v, MechanicsViolation::DayOffsetChain { .. })),
            "expected DayOffsetChain violation, got: {violations:?}"
        );
    }

    #[test]
    fn test_mechanics_loss_before_bound() {
        // CoverageRequested at day 5; AssetDamage at day 4 (before CoverageRequested).
        let base_day = 5u64; // so days 0–4 are "before insured appears"
        let submission_id = SubmissionId(1);
        let policy_id = PolicyId(1);

        let mut events = valid_chain_events(submission_id, policy_id, base_day);
        // Insert an AssetDamage at day 4 (before CoverageRequested at day 5).
        events.push(sim_ev(
            base_day - 1,
            Event::AssetDamage {
                insured_id: InsuredId(1),
                peril: Peril::WindstormAtlantic,
                ground_up_loss: 100,
            },
        ));

        let violations = verify_mechanics(&events);
        assert!(
            violations.iter().any(|v| matches!(v, MechanicsViolation::LossBeforeBound { .. })),
            "expected LossBeforeBound violation, got: {violations:?}"
        );
    }

    // ── Integration tests ─────────────────────────────────────────────────────

    fn small_test_config(seed: u64) -> crate::config::SimulationConfig {
        use crate::config::{AttritionalConfig, CatConfig, InsurerConfig, SimulationConfig};
        SimulationConfig {
            seed,
            years: 5,
            warmup_years: 0,
            insurers: (1u64..=2)
                .map(|i| InsurerConfig {
                    id: InsurerId(i),
                    initial_capital: 10_000_000_000, // 100M USD in cents
                    attritional_elf: 0.030,
                    cat_elf: 0.033,
                    target_loss_ratio: 0.80,
                    ewma_credibility: 0.3,
                    profit_loading: 0.05,
                    expense_ratio: 0.344,
                    net_line_capacity: None,
                    solvency_capital_fraction: None,
                    pml_damage_fraction_override: None,
                })
                .collect(),
            n_insureds: 20,
            attritional: AttritionalConfig { annual_rate: 2.0, mu: -4.7, sigma: 1.0 },
            catastrophe: CatConfig {
                annual_frequency: 0.5,
                pareto_scale: 0.04,
                pareto_shape: 2.5,
            },
            quotes_per_submission: None,
            max_rate_on_line: 1.0,
        }
    }

    #[test]
    fn integrity_holds_small_config() {
        use crate::simulation::Simulation;
        for seed in [1u64, 2, 3] {
            let config = small_test_config(seed);
            let mut sim = Simulation::from_config(config);
            sim.start();
            sim.run();
            let mech = verify_mechanics(&sim.log);
            assert!(mech.is_empty(), "seed {seed}: mechanics violations: {mech:?}");
            let integ = verify_integrity(&sim.log);
            assert!(integ.is_empty(), "seed {seed}: integrity violations: {integ:?}");
        }
    }

    // ── Quoting flow invariant tests (Inv 16–18) ─────────────────────────────

    #[test]
    fn test_integrity_quoting_orphan_request() {
        // LeadQuoteRequested with no following response → LeadQuoteOrphanRequest.
        let events = vec![sim_ev(
            1,
            Event::LeadQuoteRequested {
                submission_id: SubmissionId(1),
                insured_id: InsuredId(1),
                insurer_id: InsurerId(1),
                risk: dummy_risk(),
            },
        )];
        let violations = verify_integrity(&events);
        assert!(
            violations.iter().any(|v| matches!(v, IntegrityViolation::LeadQuoteOrphanRequest { .. })),
            "expected LeadQuoteOrphanRequest violation, got: {violations:?}"
        );
    }

    #[test]
    fn test_integrity_quoting_duplicate_response() {
        // Two LeadQuoteIssued for the same (sub, ins) pair → LeadQuoteDuplicateResponse.
        let events = vec![
            sim_ev(
                1,
                Event::LeadQuoteRequested {
                    submission_id: SubmissionId(1),
                    insured_id: InsuredId(1),
                    insurer_id: InsurerId(1),
                    risk: dummy_risk(),
                },
            ),
            sim_ev(
                1,
                Event::LeadQuoteIssued {
                    submission_id: SubmissionId(1),
                    insured_id: InsuredId(1),
                    insurer_id: InsurerId(1),
                    atp: 100,
                    premium: 105,
                    cat_exposure_at_quote: 0,
                },
            ),
            sim_ev(
                2,
                Event::LeadQuoteIssued {
                    submission_id: SubmissionId(1),
                    insured_id: InsuredId(1),
                    insurer_id: InsurerId(1),
                    atp: 100,
                    premium: 105,
                    cat_exposure_at_quote: 0,
                },
            ),
        ];
        let violations = verify_integrity(&events);
        assert!(
            violations.iter().any(|v| matches!(v, IntegrityViolation::LeadQuoteDuplicateResponse { .. })),
            "expected LeadQuoteDuplicateResponse violation, got: {violations:?}"
        );
    }

    #[test]
    fn test_integrity_quoting_orphan_response() {
        // LeadQuoteIssued with no prior LeadQuoteRequested → LeadQuoteOrphanResponse.
        let events = vec![sim_ev(
            1,
            Event::LeadQuoteIssued {
                submission_id: SubmissionId(1),
                insured_id: InsuredId(1),
                insurer_id: InsurerId(1),
                atp: 100,
                premium: 105,
                cat_exposure_at_quote: 0,
            },
        )];
        let violations = verify_integrity(&events);
        assert!(
            violations.iter().any(|v| matches!(v, IntegrityViolation::LeadQuoteOrphanResponse { .. })),
            "expected LeadQuoteOrphanResponse violation, got: {violations:?}"
        );
    }

    #[test]
    #[ignore]
    fn integrity_holds_canonical_multi_seed() {
        use crate::simulation::Simulation;
        for seed in [42u64, 43, 44, 45, 46] {
            let mut config = crate::config::SimulationConfig::canonical();
            config.seed = seed;
            let mut sim = Simulation::from_config(config);
            sim.start();
            sim.run();
            let mech = verify_mechanics(&sim.log);
            assert!(mech.is_empty(), "seed {seed}: mechanics violations: {mech:?}");
            let integ = verify_integrity(&sim.log);
            assert!(integ.is_empty(), "seed {seed}: integrity violations: {integ:?}");
        }
    }
}
