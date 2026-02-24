use std::collections::HashMap;

use crate::{
    events::{Event, Peril, SimEvent},
    types::{InsurerId, PolicyId, SubmissionId},
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

    /// Dominant peril by GUL share: "Cat" >60%, "Mixed" 30–60%, "Attritional" <30%.
    pub fn dominant_peril(&self) -> &'static str {
        let total = self.attr_gul + self.cat_gul;
        if total == 0 {
            return "Attritional";
        }
        let cat_pct = self.cat_gul as f64 / total as f64;
        if cat_pct > 0.60 {
            "Cat"
        } else if cat_pct >= 0.30 {
            "Mixed"
        } else {
            "Attritional"
        }
    }
}

/// A mechanics invariant violation detected in the event stream.
#[derive(Debug)]
pub enum MechanicsViolation {
    /// PolicyBound did not arrive exactly 2 days after LeadQuoteRequested.
    DayOffsetChain { submission_id: u64, detail: String },
    /// InsuredLoss arrived before the policy was bound (any peril).
    LossBeforeBound { policy_id: u64, loss_day: u64, bound_day: u64 },
    /// Attritional InsuredLoss arrived on the bound day or earlier (must be strictly after).
    AttrNotStrictlyPostBound { policy_id: u64, loss_day: u64, bound_day: u64 },
    /// PolicyExpired did not fire at QuoteAccepted_day + 361.
    PolicyExpiredTiming { policy_id: u64, expected: u64, actual: u64 },
    /// ClaimSettled arrived after the policy had expired.
    ClaimAfterExpiry { policy_id: u64, claim_day: u64, expiry_day: u64 },
    /// InsuredLoss ground_up_loss exceeds the policy sum_insured (damage fraction > 1.0).
    CatFractionInconsistent { peril: String, day: u64, detail: String },
}

fn day_to_year(day: u64) -> u32 {
    (day / 360 + 1) as u32
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

    for sim_event in events {
        let day = sim_event.day.0;
        let year = day_to_year(day);

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
            Event::InsuredLoss { peril, ground_up_loss, .. } => {
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
    let mut bound_day: HashMap<PolicyId, u64> = HashMap::new();
    let mut policy_from_sub: HashMap<SubmissionId, PolicyId> = HashMap::new();
    let mut expiry_day: HashMap<PolicyId, u64> = HashMap::new();
    let mut sum_insured_by_policy: HashMap<PolicyId, u64> = HashMap::new();

    // First pass: index LeadQuoteRequested, QuoteAccepted, PolicyBound, PolicyExpired.
    for ev in events {
        let day = ev.day.0;
        match &ev.event {
            Event::LeadQuoteRequested { submission_id, .. } => {
                lqr_day.entry(*submission_id).or_insert(day);
            }
            Event::QuoteAccepted { submission_id, .. } => {
                qa_day.insert(*submission_id, day);
            }
            Event::PolicyBound { policy_id, submission_id, sum_insured, .. } => {
                bound_day.insert(*policy_id, day);
                policy_from_sub.insert(*submission_id, *policy_id);
                sum_insured_by_policy.insert(*policy_id, *sum_insured);

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
            Event::InsuredLoss { policy_id, peril, ground_up_loss, .. } => {
                if let Some(&bd) = bound_day.get(policy_id) {
                    // Invariant 2 — LossBeforeBound: loss_day must not be before bound_day.
                    if day < bd {
                        violations.push(MechanicsViolation::LossBeforeBound {
                            policy_id: policy_id.0,
                            loss_day: day,
                            bound_day: bd,
                        });
                    }
                    // Invariant 3 — AttrNotStrictlyPostBound: attritional loss must be strictly after bound.
                    if matches!(peril, Peril::Attritional) && day <= bd {
                        violations.push(MechanicsViolation::AttrNotStrictlyPostBound {
                            policy_id: policy_id.0,
                            loss_day: day,
                            bound_day: bd,
                        });
                    }
                }
                // Invariant 6 — CatFractionInconsistent: ground_up_loss must not exceed sum_insured.
                if matches!(peril, Peril::WindstormAtlantic) {
                    if let Some(&si) = sum_insured_by_policy.get(policy_id) {
                        if *ground_up_loss > si {
                            violations.push(MechanicsViolation::CatFractionInconsistent {
                                peril: "WindstormAtlantic".to_string(),
                                day,
                                detail: format!(
                                    "policy {} gul {} > sum_insured {}",
                                    policy_id.0, ground_up_loss, si
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
    fn test_dominant_peril_cat() {
        let events = vec![
            sim_start(),
            sim_ev(
                50,
                Event::InsuredLoss {
                    policy_id: PolicyId(1),
                    insured_id: InsuredId(1),
                    peril: Peril::WindstormAtlantic,
                    ground_up_loss: 700,
                },
            ),
            sim_ev(
                50,
                Event::InsuredLoss {
                    policy_id: PolicyId(1),
                    insured_id: InsuredId(1),
                    peril: Peril::Attritional,
                    ground_up_loss: 300,
                },
            ),
            sim_ev(359, Event::YearEnd { year: Year(1) }),
        ];
        let (_, stats) = analyse(&events, &empty_capitals(), 0.344);
        assert_eq!(stats[0].dominant_peril(), "Cat");
    }

    #[test]
    fn test_dominant_peril_attritional() {
        let events = vec![
            sim_start(),
            sim_ev(
                50,
                Event::InsuredLoss {
                    policy_id: PolicyId(1),
                    insured_id: InsuredId(1),
                    peril: Peril::Attritional,
                    ground_up_loss: 1_000,
                },
            ),
            sim_ev(359, Event::YearEnd { year: Year(1) }),
        ];
        let (_, stats) = analyse(&events, &empty_capitals(), 0.344);
        assert_eq!(stats[0].dominant_peril(), "Attritional");
    }

    #[test]
    fn test_dominant_peril_mixed() {
        let events = vec![
            sim_start(),
            // cat = 450, attr = 550 → cat_pct = 45% → Mixed
            sim_ev(
                50,
                Event::InsuredLoss {
                    policy_id: PolicyId(1),
                    insured_id: InsuredId(1),
                    peril: Peril::WindstormAtlantic,
                    ground_up_loss: 450,
                },
            ),
            sim_ev(
                50,
                Event::InsuredLoss {
                    policy_id: PolicyId(1),
                    insured_id: InsuredId(1),
                    peril: Peril::Attritional,
                    ground_up_loss: 550,
                },
            ),
            sim_ev(359, Event::YearEnd { year: Year(1) }),
        ];
        let (_, stats) = analyse(&events, &empty_capitals(), 0.344);
        assert_eq!(stats[0].dominant_peril(), "Mixed");
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
        // PolicyBound at day 3, InsuredLoss for that policy at day 2 (before bound).
        let base_day = 0u64;
        let submission_id = SubmissionId(1);
        let policy_id = PolicyId(1);

        let mut events = valid_chain_events(submission_id, policy_id, base_day);
        // Insert an InsuredLoss at day 2 (before PolicyBound at day 3).
        events.push(sim_ev(
            base_day + 2,
            Event::InsuredLoss {
                policy_id,
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
}
