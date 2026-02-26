use std::collections::{HashMap, VecDeque};

use crate::events::{DeclineReason, Event, Peril, Risk};
use crate::types::{Day, InsuredId, InsurerId, PolicyId, SubmissionId, YearAccumulator};

/// A single insurer in the minimal property market.
/// Writes 100% of each risk it quotes (lead-only, no follow market).
/// Capital is endowed once at construction and persists year-over-year; premiums add, claims deduct.
pub struct Insurer {
    pub id: InsurerId,
    /// Current capital (unsigned floor at zero; cannot go negative).
    pub capital: i64,
    /// Set to true the first time a claim drives capital to zero.
    /// An insolvent insurer declines all new quote requests but continues settling claims.
    pub insolvent: bool,
    /// Actuarial channel: E[attritional_loss] / sum_insured.
    /// Updated each YearEnd via EWMA from realized attritional burning cost.
    attritional_elf: f64,
    /// Actuarial channel: E[cat_loss] / sum_insured.
    /// Anchored — never updated from experience. Derived from the cat model.
    /// A quiet cat period is not evidence of a lower rate; EWMA would produce systematic
    /// soft-market erosion. Mirrors Lloyd's MS3 Technical Premium requirements.
    cat_elf: f64,
    /// Actuarial channel: ATP = (attritional_elf + cat_elf) / target_loss_ratio.
    target_loss_ratio: f64,
    /// EWMA credibility weight α: new_att_elf = α × realized_att_lf + (1-α) × old_att_elf.
    ewma_credibility: f64,
    /// Fraction of gross premium consumed by acquisition costs + overhead.
    expense_ratio: f64,
    /// Multiplicative loading above ATP: premium = ATP × (1 + profit_loading).
    profit_loading: f64,
    /// Year-to-date premium and claims accumulators; reset at each YearEnd.
    ytd: YearAccumulator,
    /// Exposure management: live WindstormAtlantic aggregate sum_insured.
    pub cat_aggregate: u64,
    /// Fraction of current capital committable to a single risk net line (None = unlimited).
    net_line_capacity: Option<f64>,
    /// Fraction of capital for the 1-in-200 cat scenario (None = unlimited).
    solvency_capital_fraction: Option<f64>,
    /// Pareto 1-in-200 damage fraction derived from cat model at construction.
    pml_damage_fraction_200: f64,
    /// Map from policy_id to its WindstormAtlantic sum_insured, for release on expiry.
    cat_policy_map: HashMap<PolicyId, u64>,
    /// Capital at construction — used to compute depletion ratio.
    initial_capital: i64,
    /// Sensitivity of capital-depletion adjustment: cap_depletion_adj = depletion × sensitivity.
    depletion_sensitivity: f64,
    /// Rolling per-insurer annual loss ratios (max 3 years) for own CR computation.
    own_loss_ratios: VecDeque<f64>,
    /// Number of years of own experience accumulated; drives credibility weight.
    own_years: u32,
}

impl Insurer {
    pub fn new(
        id: InsurerId,
        initial_capital: i64,
        attritional_elf: f64,
        cat_elf: f64,
        target_loss_ratio: f64,
        ewma_credibility: f64,
        expense_ratio: f64,
        profit_loading: f64,
        net_line_capacity: Option<f64>,
        solvency_capital_fraction: Option<f64>,
        pml_damage_fraction_200: f64,
        depletion_sensitivity: f64,
    ) -> Self {
        Insurer {
            id,
            capital: initial_capital,
            insolvent: false,
            attritional_elf,
            cat_elf,
            target_loss_ratio,
            ewma_credibility,
            expense_ratio,
            profit_loading,
            ytd: YearAccumulator::default(),
            cat_aggregate: 0,
            net_line_capacity,
            solvency_capital_fraction,
            pml_damage_fraction_200,
            cat_policy_map: HashMap::new(),
            initial_capital,
            depletion_sensitivity,
            own_loss_ratios: VecDeque::new(),
            own_years: 0,
        }
    }

    /// Called at each YearStart. Capital is NOT reset — it persists from prior year.
    pub fn on_year_start(&mut self) {}

    /// Price and issue a lead quote for a risk, or decline if an exposure limit is breached.
    /// Returns a single `LeadQuoteIssued` or `LeadQuoteDeclined` event.
    /// `market_ap_tp_factor`: coordinator-published AP/TP ratio; 1.0 = neutral.
    pub fn on_lead_quote_requested(
        &self,
        day: Day,
        submission_id: SubmissionId,
        insured_id: InsuredId,
        risk: &Risk,
        market_ap_tp_factor: f64,
    ) -> Vec<(Day, Event)> {
        if self.insolvent {
            return vec![(
                day,
                Event::LeadQuoteDeclined {
                    submission_id,
                    insured_id,
                    insurer_id: self.id,
                    reason: DeclineReason::Insolvent,
                },
            )];
        }
        if let Some(nlc) = self.net_line_capacity {
            let effective_line_limit = (nlc * self.capital.max(0) as f64) as u64;
            if risk.sum_insured > effective_line_limit {
                return vec![(
                    day,
                    Event::LeadQuoteDeclined {
                        submission_id,
                        insured_id,
                        insurer_id: self.id,
                        reason: DeclineReason::MaxLineSizeExceeded,
                    },
                )];
            }
        }
        if let Some(scf) = self.solvency_capital_fraction {
            let effective_cat_limit =
                (scf * self.capital.max(0) as f64 / self.pml_damage_fraction_200) as u64;
            if risk.perils_covered.contains(&Peril::WindstormAtlantic)
                && self.cat_aggregate + risk.sum_insured > effective_cat_limit
            {
                return vec![(
                    day,
                    Event::LeadQuoteDeclined {
                        submission_id,
                        insured_id,
                        insurer_id: self.id,
                        reason: DeclineReason::MaxCatAggregateBreached,
                    },
                )];
            }
        }
        let atp = self.actuarial_price(risk);
        let premium = self.underwriter_premium(risk, market_ap_tp_factor);
        let cat_exposure_at_quote = if risk.perils_covered.contains(&Peril::WindstormAtlantic) {
            self.cat_aggregate
        } else {
            0
        };
        vec![(
            day,
            Event::LeadQuoteIssued {
                submission_id,
                insured_id,
                insurer_id: self.id,
                atp,
                premium,
                cat_exposure_at_quote,
            },
        )]
    }

    /// A policy has been bound. Credit net premium to capital, accumulate written exposure for EWMA; update cat aggregate.
    pub fn on_policy_bound(
        &mut self,
        policy_id: PolicyId,
        sum_insured: u64,
        premium: u64,
        perils: &[Peril],
    ) {
        let net_premium = (premium as f64 * (1.0 - self.expense_ratio)).round() as i64;
        self.capital += net_premium;
        self.ytd.exposure += sum_insured;
        self.ytd.premium += premium;
        if perils.contains(&Peril::WindstormAtlantic) {
            self.cat_aggregate += sum_insured;
            self.cat_policy_map.insert(policy_id, sum_insured);
        }
    }

    /// A policy has expired. Release its WindstormAtlantic aggregate contribution.
    pub fn on_policy_expired(&mut self, policy_id: PolicyId) {
        if let Some(sum_insured) = self.cat_policy_map.remove(&policy_id) {
            self.cat_aggregate = self.cat_aggregate.saturating_sub(sum_insured);
        }
    }

    /// Actuarial channel: (attritional_elf + cat_elf) × sum_insured / target_loss_ratio.
    /// cat_elf is anchored; attritional_elf drifts via EWMA.
    fn actuarial_price(&self, risk: &Risk) -> u64 {
        let elf = self.attritional_elf + self.cat_elf;
        (elf * risk.sum_insured as f64 / self.target_loss_ratio).round() as u64
    }

    /// Blend market factor with per-insurer capital state and loss history.
    /// credibility = min(own_years / 5.0, 1.0); new entrant follows market exactly.
    fn own_ap_tp_factor(&self, market_factor: f64) -> f64 {
        let credibility = (self.own_years as f64 / 5.0).min(1.0);

        let depletion = if self.initial_capital > 0 {
            (1.0 - self.capital as f64 / self.initial_capital as f64).max(0.0)
        } else {
            0.0
        };
        let cap_depletion_adj = (depletion * self.depletion_sensitivity).clamp(0.0, 0.30);

        let own_cr_signal = if self.own_loss_ratios.is_empty() {
            0.0
        } else {
            let avg_lr = self.own_loss_ratios.iter().sum::<f64>() / self.own_loss_ratios.len() as f64;
            let avg_cr = avg_lr + self.expense_ratio;
            (avg_cr - 1.0).clamp(-0.25, 0.40)
        };

        let own_factor = (1.0 + own_cr_signal + cap_depletion_adj).clamp(0.90, 1.40);
        credibility * own_factor + (1.0 - credibility) * market_factor
    }

    /// Underwriter channel: TP × own_ap_tp_factor (blend of market signal and own state).
    /// TP = ATP × (1 + profit_loading) — the per-insurer Technical Premium.
    fn underwriter_premium(&self, risk: &Risk, market_ap_tp_factor: f64) -> u64 {
        let tp = self.actuarial_price(risk) as f64 * (1.0 + self.profit_loading);
        (tp * self.own_ap_tp_factor(market_ap_tp_factor)).round() as u64
    }

    /// Deduct a settled claim from capital (floored at zero).
    /// Only attritional claims are accumulated for the EWMA; cat claims are excluded
    /// because cat_elf is anchored and not updated from experience.
    /// Returns `InsurerInsolvent` on the first crossing to zero; empty otherwise.
    pub fn on_claim_settled(&mut self, day: Day, amount: u64, peril: Peril) -> Vec<(Day, Event)> {
        let payable = amount.min(self.capital.max(0) as u64);
        self.capital -= payable as i64; // floors at 0 naturally
        if peril == Peril::Attritional {
            self.ytd.attritional_claims += payable;
        }
        self.ytd.total_claims += payable;

        if self.capital == 0 && !self.insolvent {
            self.insolvent = true;
            vec![(day, Event::InsurerInsolvent { insurer_id: self.id })]
        } else {
            vec![]
        }
    }

    /// Update attritional_elf via EWMA from this year's realized attritional burning cost,
    /// then reset YTD accumulators. cat_elf is never updated. No-op if no exposure written.
    /// Also detects "zombie" state: capital > 0 but max_line < min_sum_insured — the insurer
    /// can no longer write any new business. Marks it insolvent and emits InsurerInsolvent.
    pub fn on_year_end(&mut self, day: Day, min_sum_insured: u64) -> Vec<(Day, Event)> {
        if self.ytd.exposure > 0 {
            let realized_att_lf = self.ytd.attritional_loss_fraction();
            self.attritional_elf = self.ewma_credibility * realized_att_lf
                + (1.0 - self.ewma_credibility) * self.attritional_elf;
        }
        // Accumulate per-insurer loss ratio for own CR pricing signal.
        if self.ytd.premium > 0 {
            let own_lr = self.ytd.total_claims as f64 / self.ytd.premium as f64;
            self.own_loss_ratios.push_back(own_lr);
            if self.own_loss_ratios.len() > 3 {
                self.own_loss_ratios.pop_front();
            }
        }
        self.own_years += 1;
        self.ytd.reset();

        // Zombie check: capital > 0 but max_line < min writeable policy size.
        // Functionally equivalent to insolvency — no new business can be written.
        if !self.insolvent {
            if let Some(nlc) = self.net_line_capacity {
                let max_line = (nlc * self.capital.max(0) as f64) as u64;
                if max_line < min_sum_insured {
                    self.insolvent = true;
                    return vec![(day, Event::InsurerInsolvent { insurer_id: self.id })];
                }
            }
        }
        vec![]
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::ASSET_VALUE;
    use crate::events::Peril;

    fn small_risk() -> Risk {
        Risk {
            sum_insured: ASSET_VALUE,
            territory: "US-SE".to_string(),
            perils_covered: vec![Peril::WindstormAtlantic, Peril::Attritional],
        }
    }

    fn make_insurer(id: InsurerId, capital: i64) -> Insurer {
        // attritional_elf=0.239, cat_elf=0.0, profit_loading=0.0, depletion_sensitivity=0.0
        // depletion_sensitivity=0.0 → no depletion effect; preserves all existing test behaviour.
        Insurer::new(id, capital, 0.239, 0.0, 0.70, 0.3, 0.0, 0.0, None, None, 0.252, 0.0)
    }

    /// Helper: quote and return the ATP for a standard small_risk().
    fn quote_atp(ins: &Insurer) -> u64 {
        let risk = Risk {
            sum_insured: ASSET_VALUE,
            territory: "US-SE".to_string(),
            perils_covered: vec![Peril::Attritional],
        };
        let events = ins.on_lead_quote_requested(Day(0), SubmissionId(1), InsuredId(1), &risk, 1.0);
        let (_, event) = events.into_iter().next().unwrap();
        if let Event::LeadQuoteIssued { atp, .. } = event { atp } else { panic!("expected LeadQuoteIssued") }
    }

    #[test]
    fn on_year_start_preserves_capital() {
        let mut ins = make_insurer(InsurerId(1), 1_000_000);
        ins.capital = 500_000; // depleted by claims
        ins.on_year_start();
        assert_eq!(ins.capital, 500_000, "on_year_start must not reset capital — it must persist from prior year");
    }

    #[test]
    fn on_claim_settled_reduces_capital() {
        let mut ins = make_insurer(InsurerId(1), 1_000_000);
        ins.on_claim_settled(Day(0), 300_000, Peril::Attritional);
        assert_eq!(ins.capital, 700_000);
    }

    #[test]
    fn on_claim_settled_floors_at_zero_and_emits_insolvent() {
        let mut ins = make_insurer(InsurerId(1), 100);
        let events = ins.on_claim_settled(Day(5), 1_000_000, Peril::Attritional);
        assert_eq!(ins.capital, 0, "capital must floor at zero");
        assert!(ins.insolvent, "insurer must be marked insolvent");
        assert_eq!(events.len(), 1, "must emit exactly one InsurerInsolvent event");
        assert!(
            matches!(events[0].1, Event::InsurerInsolvent { insurer_id } if insurer_id == InsurerId(1)),
            "event must be InsurerInsolvent with correct id"
        );
    }

    #[test]
    fn multiple_claims_exhaust_capital_and_insurer_becomes_insolvent() {
        // Two policy premiums partially offset initial capital, but further
        // claims exhaust it — capital must floor at zero and insurer must become insolvent.
        let initial_capital = 1_000_000i64;
        let gross_premium = 200_000u64;
        // expense_ratio=0.0 → net premium = gross premium
        let mut ins = Insurer::new(InsurerId(1), initial_capital, 0.239, 0.0, 0.55, 0.3, 0.0, 0.0, None, None, 0.252, 0.0);
        ins.on_policy_bound(PolicyId(1), ASSET_VALUE, gross_premium, &[Peril::Attritional]);
        ins.on_policy_bound(PolicyId(2), ASSET_VALUE, gross_premium, &[Peril::Attritional]);
        let total_net_premiums = (gross_premium * 2) as i64;
        let total_available = initial_capital + total_net_premiums;
        // Two claims that together exceed total available funds
        let claim = (total_available as u64 / 2) + 1;
        let _ = ins.on_claim_settled(Day(0), claim, Peril::Attritional);
        let _ = ins.on_claim_settled(Day(0), claim, Peril::Attritional);
        assert_eq!(
            ins.capital, 0,
            "capital must floor at zero after cumulative claims exceed available funds; got {}",
            ins.capital
        );
        assert!(ins.insolvent, "insurer must be marked insolvent after capital is exhausted");
    }

    fn first_event(events: Vec<(Day, Event)>) -> (Day, Event) {
        events.into_iter().next().unwrap()
    }

    #[test]
    fn on_lead_quote_requested_always_quotes() {
        let ins = make_insurer(InsurerId(1), 1_000_000_000);
        let risk = small_risk();
        let (_, event) = first_event(ins.on_lead_quote_requested(Day(0), SubmissionId(1), InsuredId(1), &risk, 1.0));
        assert!(
            matches!(event, Event::LeadQuoteIssued { .. }),
            "insurer must always issue a lead quote, got {event:?}"
        );
    }

    #[test]
    fn premium_equals_atp_when_profit_loading_zero() {
        // make_insurer uses profit_loading=0.0, so premium = ATP × 1.0 = ATP.
        let ins = make_insurer(InsurerId(1), 1_000_000_000);
        let risk = small_risk();
        let (_, event) = first_event(ins.on_lead_quote_requested(Day(0), SubmissionId(1), InsuredId(1), &risk, 1.0));
        if let Event::LeadQuoteIssued { atp, premium, .. } = event {
            assert_eq!(premium, atp, "with profit_loading=0.0, premium must equal ATP");
        }
    }

    #[test]
    fn lead_quote_issued_carries_insured_id() {
        let ins = make_insurer(InsurerId(1), 1_000_000_000);
        let risk = small_risk();
        let (_, event) =
            first_event(ins.on_lead_quote_requested(Day(0), SubmissionId(5), InsuredId(42), &risk, 1.0));
        if let Event::LeadQuoteIssued { insured_id, submission_id, insurer_id, .. } = event {
            assert_eq!(insured_id, InsuredId(42));
            assert_eq!(submission_id, SubmissionId(5));
            assert_eq!(insurer_id, InsurerId(1));
        } else {
            panic!("expected LeadQuoteIssued");
        }
    }

    #[test]
    fn premium_scales_with_sum_insured() {
        let ins = make_insurer(InsurerId(1), 0);
        let small = Risk {
            sum_insured: ASSET_VALUE,
            territory: "US-SE".to_string(),
            perils_covered: vec![Peril::Attritional],
        };
        let large = Risk {
            sum_insured: ASSET_VALUE * 10,
            territory: "US-SE".to_string(),
            perils_covered: vec![Peril::Attritional],
        };
        let (_, e_small) =
            first_event(ins.on_lead_quote_requested(Day(0), SubmissionId(1), InsuredId(1), &small, 1.0));
        let (_, e_large) =
            first_event(ins.on_lead_quote_requested(Day(0), SubmissionId(2), InsuredId(2), &large, 1.0));
        let p_small =
            if let Event::LeadQuoteIssued { premium, .. } = e_small { premium } else { 0 };
        let p_large =
            if let Event::LeadQuoteIssued { premium, .. } = e_large { premium } else { 0 };
        assert!(
            p_large > p_small,
            "larger sum_insured must produce larger premium: {p_large} vs {p_small}"
        );
    }

    #[test]
    fn quote_premium_is_positive_for_nonzero_risk() {
        let ins = make_insurer(InsurerId(1), 1_000_000_000);
        let risk = small_risk();
        let (_, event) = first_event(ins.on_lead_quote_requested(Day(0), SubmissionId(1), InsuredId(1), &risk, 1.0));
        if let Event::LeadQuoteIssued { premium, .. } = event {
            assert!(premium > 0, "premium must be positive for a non-trivial risk");
        }
    }

    #[test]
    fn atp_equals_expected_loss_over_target_ratio() {
        let ins = make_insurer(InsurerId(1), 0);
        let risk = small_risk();
        let expected = (0.239 * ASSET_VALUE as f64 / 0.70).round() as u64;
        let (_, event) = first_event(ins.on_lead_quote_requested(Day(0), SubmissionId(1), InsuredId(1), &risk, 1.0));
        if let Event::LeadQuoteIssued { atp, .. } = event {
            assert_eq!(atp, expected, "ATP must equal expected_loss_fraction × sum_insured / target_loss_ratio");
        } else {
            panic!("expected LeadQuoteIssued");
        }
    }

    #[test]
    fn premium_equals_atp_times_loading() {
        // make_insurer uses attritional_elf=0.239, cat_elf=0.0, profit_loading=0.0.
        // premium = (0.239 + 0.0) × sum_insured / target_LR × (1 + 0.0) = ATP.
        let ins = make_insurer(InsurerId(1), 0);
        let risk = small_risk();
        let expected = (0.239 * ASSET_VALUE as f64 / 0.70).round() as u64;
        let (_, event) = first_event(ins.on_lead_quote_requested(Day(0), SubmissionId(1), InsuredId(1), &risk, 1.0));
        if let Event::LeadQuoteIssued { premium, .. } = event {
            assert_eq!(premium, expected, "premium must equal (attritional_elf + cat_elf) × sum_insured / target_loss_ratio × (1 + profit_loading)");
        } else {
            panic!("expected LeadQuoteIssued");
        }
    }

    // ── Exposure management ───────────────────────────────────────────────────

    fn cat_risk() -> Risk {
        Risk {
            sum_insured: ASSET_VALUE,
            territory: "US-SE".to_string(),
            perils_covered: vec![Peril::WindstormAtlantic],
        }
    }

    fn att_only_risk() -> Risk {
        Risk {
            sum_insured: ASSET_VALUE,
            territory: "US-SE".to_string(),
            perils_covered: vec![Peril::Attritional],
        }
    }

    #[test]
    fn on_policy_bound_increments_cat_aggregate() {
        let mut ins = make_insurer(InsurerId(1), 0);
        ins.on_policy_bound(PolicyId(1), ASSET_VALUE, 0, &[Peril::WindstormAtlantic]);
        assert_eq!(ins.cat_aggregate, ASSET_VALUE, "cat_aggregate must equal sum_insured after binding one cat policy");
    }

    #[test]
    fn on_policy_expired_releases_cat_aggregate() {
        let mut ins = make_insurer(InsurerId(1), 0);
        ins.on_policy_bound(PolicyId(1), ASSET_VALUE, 0, &[Peril::WindstormAtlantic]);
        assert_eq!(ins.cat_aggregate, ASSET_VALUE);
        ins.on_policy_expired(PolicyId(1));
        assert_eq!(ins.cat_aggregate, 0, "cat_aggregate must return to 0 after policy expiry");
    }

    #[test]
    fn non_cat_policy_does_not_affect_cat_aggregate() {
        let mut ins = make_insurer(InsurerId(1), 0);
        ins.on_policy_bound(PolicyId(1), ASSET_VALUE, 0, &[Peril::Attritional]);
        assert_eq!(ins.cat_aggregate, 0, "attritional-only policy must not affect cat_aggregate");
    }

    #[test]
    fn cat_exposure_at_quote_reflects_aggregate() {
        let mut ins = make_insurer(InsurerId(1), 0);
        // Bind a cat policy first.
        ins.on_policy_bound(PolicyId(1), ASSET_VALUE, 0, &[Peril::WindstormAtlantic]);

        // Quote a second cat risk — exposure_at_quote should reflect the already-bound aggregate.
        let risk = cat_risk();
        let (_, event) = first_event(ins.on_lead_quote_requested(Day(0), SubmissionId(2), InsuredId(2), &risk, 1.0));
        if let Event::LeadQuoteIssued { cat_exposure_at_quote, .. } = event {
            assert_eq!(
                cat_exposure_at_quote, ASSET_VALUE,
                "cat_exposure_at_quote must equal the already-bound cat aggregate"
            );
        } else {
            panic!("expected LeadQuoteIssued");
        }
    }

    #[test]
    fn cat_exposure_at_quote_is_zero_for_non_cat_risk() {
        let mut ins = make_insurer(InsurerId(1), 0);
        ins.on_policy_bound(PolicyId(1), ASSET_VALUE, 0, &[Peril::WindstormAtlantic]);

        let risk = att_only_risk();
        let (_, event) = first_event(ins.on_lead_quote_requested(Day(0), SubmissionId(2), InsuredId(2), &risk, 1.0));
        if let Event::LeadQuoteIssued { cat_exposure_at_quote, .. } = event {
            assert_eq!(
                cat_exposure_at_quote, 0,
                "cat_exposure_at_quote must be 0 for a risk that doesn't cover WindstormAtlantic"
            );
        } else {
            panic!("expected LeadQuoteIssued");
        }
    }

    // ── Exposure limit enforcement ────────────────────────────────────────────

    #[test]
    fn max_line_size_exceeded_emits_declined() {
        // capital=0 → effective_line = 0.30 × 0 = 0 < ASSET_VALUE → declines MaxLineSizeExceeded.
        let ins = Insurer::new(InsurerId(1), 0, 0.239, 0.0, 0.70, 0.3, 0.0, 0.0, Some(0.30), Some(0.30), 0.252, 0.0);
        let risk = cat_risk(); // sum_insured = ASSET_VALUE > effective_line_limit (0)
        let (_, event) = first_event(ins.on_lead_quote_requested(Day(0), SubmissionId(1), InsuredId(1), &risk, 1.0));
        assert!(
            matches!(event, Event::LeadQuoteDeclined { reason: DeclineReason::MaxLineSizeExceeded, .. }),
            "expected LeadQuoteDeclined(MaxLineSizeExceeded), got {event:?}"
        );
    }

    #[test]
    fn max_cat_aggregate_breached_emits_declined() {
        // net_line_capacity=None skips the line check; capital=0 → effective_cat = 0 → declines MaxCatAggregateBreached.
        let ins = Insurer::new(InsurerId(1), 0, 0.239, 0.0, 0.70, 0.3, 0.0, 0.0, None, Some(0.30), 0.252, 0.0);
        let risk = cat_risk(); // cat_aggregate(0) + sum_insured > effective_cat_limit(0)
        let (_, event) = first_event(ins.on_lead_quote_requested(Day(0), SubmissionId(1), InsuredId(1), &risk, 1.0));
        assert!(
            matches!(event, Event::LeadQuoteDeclined { reason: DeclineReason::MaxCatAggregateBreached, .. }),
            "expected LeadQuoteDeclined(MaxCatAggregateBreached), got {event:?}"
        );
    }

    #[test]
    fn within_limits_after_partial_fill_emits_quote_issued() {
        // capital=200M USD; effective_cat = 0.30 × 20B / 0.252 ≈ 23.8B > 2×ASSET_VALUE=10B → room for second policy.
        let mut ins = Insurer::new(InsurerId(1), 20_000_000_000, 0.239, 0.0, 0.70, 0.3, 0.0, 0.0, None, Some(0.30), 0.252, 0.0);
        ins.on_policy_bound(PolicyId(1), ASSET_VALUE, 0, &[Peril::WindstormAtlantic]);
        // cat_aggregate = ASSET_VALUE; effective_cat ≈ 23.8B → still room for one more
        let risk = cat_risk();
        let (_, event) = first_event(ins.on_lead_quote_requested(Day(0), SubmissionId(2), InsuredId(2), &risk, 1.0));
        assert!(
            matches!(event, Event::LeadQuoteIssued { .. }),
            "still within limit — must emit LeadQuoteIssued, got {event:?}"
        );
    }

    // ── EWMA experience update ────────────────────────────────────────────────

    #[test]
    fn on_year_end_raises_atp_after_high_loss_year() {
        // Bind one policy; settle a claim equal to 100% of sum_insured.
        // Realized LF = 1.0 >> prior ELF = 0.239 → ATP must increase.
        let mut ins = make_insurer(InsurerId(1), ASSET_VALUE as i64 * 10);
        let atp_before = quote_atp(&ins);
        ins.on_policy_bound(PolicyId(1), ASSET_VALUE, 0, &[Peril::Attritional]);
        let _ = ins.on_claim_settled(Day(0), ASSET_VALUE, Peril::Attritional);
        let _ = ins.on_year_end(Day(0), ASSET_VALUE);
        let atp_after = quote_atp(&ins);
        assert!(atp_after > atp_before, "ATP must rise after a 100% LF year: {atp_after} vs {atp_before}");
    }

    #[test]
    fn on_year_end_lowers_atp_after_benign_year() {
        // Bind one policy; no claims. Realized LF = 0 < prior ELF = 0.239 → ATP must fall.
        let mut ins = make_insurer(InsurerId(1), 0);
        let atp_before = quote_atp(&ins);
        ins.on_policy_bound(PolicyId(1), ASSET_VALUE, 0, &[Peril::Attritional]);
        // no claims
        let _ = ins.on_year_end(Day(0), ASSET_VALUE);
        let atp_after = quote_atp(&ins);
        assert!(atp_after < atp_before, "ATP must fall after a 0% LF year: {atp_after} vs {atp_before}");
    }

    #[test]
    fn ewma_formula_matches_exact_calculation() {
        // α=0.3, realized LF = 0.5 (claim = ASSET_VALUE/2, exposure = ASSET_VALUE).
        // New ELF = 0.3 × 0.5 + 0.7 × 0.239 = 0.15 + 0.1673 = 0.3173.
        let mut ins = make_insurer(InsurerId(1), ASSET_VALUE as i64 * 10);
        ins.on_policy_bound(PolicyId(1), ASSET_VALUE, 0, &[Peril::Attritional]);
        let _ = ins.on_claim_settled(Day(0), ASSET_VALUE / 2, Peril::Attritional);
        let _ = ins.on_year_end(Day(0), ASSET_VALUE);
        let expected_elf = 0.3 * 0.5 + 0.7 * 0.239;
        let expected_atp = (expected_elf * ASSET_VALUE as f64 / 0.70).round() as u64;
        assert_eq!(quote_atp(&ins), expected_atp, "EWMA must match α × realized + (1-α) × prior");
    }

    #[test]
    fn on_year_end_with_no_exposure_leaves_atp_unchanged() {
        let mut ins = make_insurer(InsurerId(1), 0);
        let atp_before = quote_atp(&ins);
        let _ = ins.on_year_end(Day(0), ASSET_VALUE); // no policies bound, no claims
        assert_eq!(quote_atp(&ins), atp_before, "ATP must not change if no exposure was written");
    }

    #[test]
    fn on_year_end_resets_so_second_call_without_new_data_is_noop() {
        // After on_year_end resets counters, a second on_year_end with no new
        // policies or claims must leave ATP unchanged.
        let mut ins = make_insurer(InsurerId(1), ASSET_VALUE as i64 * 10);
        ins.on_policy_bound(PolicyId(1), ASSET_VALUE, 0, &[Peril::Attritional]);
        let _ = ins.on_claim_settled(Day(0), ASSET_VALUE, Peril::Attritional);
        let _ = ins.on_year_end(Day(0), ASSET_VALUE); // ELF updated, counters reset
        let atp_year1 = quote_atp(&ins);
        let _ = ins.on_year_end(Day(0), ASSET_VALUE); // no new data → noop
        assert_eq!(quote_atp(&ins), atp_year1, "second on_year_end with no data must be a noop");
    }

    #[test]
    fn ewma_compounds_over_multiple_years() {
        // Two consecutive high-loss years should push ELF higher than one.
        let mut ins = make_insurer(InsurerId(1), ASSET_VALUE as i64 * 10);
        ins.on_policy_bound(PolicyId(1), ASSET_VALUE, 0, &[Peril::Attritional]);
        let _ = ins.on_claim_settled(Day(0), ASSET_VALUE, Peril::Attritional);
        let _ = ins.on_year_end(Day(0), ASSET_VALUE);
        let atp_after_year1 = quote_atp(&ins);

        ins.on_policy_bound(PolicyId(2), ASSET_VALUE, 0, &[Peril::Attritional]);
        let _ = ins.on_claim_settled(Day(0), ASSET_VALUE, Peril::Attritional);
        let _ = ins.on_year_end(Day(0), ASSET_VALUE);
        let atp_after_year2 = quote_atp(&ins);

        assert!(atp_after_year2 > atp_after_year1, "consecutive bad years must compound ELF upward");
    }

    #[test]
    fn on_policy_bound_credits_net_premium_to_capital() {
        // expense_ratio=0.25 → net = 75% of gross premium.
        let mut ins = Insurer::new(InsurerId(1), 1_000_000, 0.239, 0.0, 0.55, 0.3, 0.25, 0.0, None, None, 0.252, 0.0);
        let gross_premium = 400_000u64;
        ins.on_policy_bound(PolicyId(1), ASSET_VALUE, gross_premium, &[Peril::Attritional]);
        let expected_net = (gross_premium as f64 * 0.75).round() as i64;
        assert_eq!(
            ins.capital,
            1_000_000 + expected_net,
            "capital must increase by net premium (gross × (1 − expense_ratio))"
        );
    }

    // ── Zombie insurer detection ──────────────────────────────────────────────

    #[test]
    fn zombie_insurer_marked_insolvent_at_year_end() {
        // capital = 80M USD → max_line = 0.30 × 80M = 24M < ASSET_VALUE (25M) → zombie
        let capital_cents = 8_000_000_000i64; // 80M USD
        let mut ins = Insurer::new(
            InsurerId(1), capital_cents,
            0.239, 0.0, 0.70, 0.3, 0.0, 0.0,
            Some(0.30), None, 0.252, 0.0,
        );
        let events = ins.on_year_end(Day(360), ASSET_VALUE);
        assert!(ins.insolvent, "zombie insurer must be marked insolvent");
        assert_eq!(events.len(), 1);
        assert!(matches!(
            events[0].1,
            Event::InsurerInsolvent { insurer_id } if insurer_id == InsurerId(1)
        ));
    }

    #[test]
    fn insurer_at_max_line_threshold_not_marked_insolvent() {
        // capital = ceil(ASSET_VALUE / 0.30) cents → max_line = 0.30 × capital ≥ ASSET_VALUE → not zombie
        let capital_cents = (ASSET_VALUE as f64 / 0.30).ceil() as i64;
        let mut ins = Insurer::new(
            InsurerId(1), capital_cents,
            0.239, 0.0, 0.70, 0.3, 0.0, 0.0,
            Some(0.30), None, 0.252, 0.0,
        );
        let events = ins.on_year_end(Day(360), ASSET_VALUE);
        assert!(!ins.insolvent, "insurer at threshold must not be marked insolvent");
        assert!(events.is_empty());
    }

    // ── Heterogeneous experience divergence ───────────────────────────────────

    #[test]
    fn two_insurers_diverge_in_atp_after_asymmetric_attritional_loss() {
        // Both start identical. ins_a incurs a 100% attritional loss; ins_b has a benign year.
        // After on_year_end the EWMA update must push ins_a's ATP above ins_b's.
        let capital = ASSET_VALUE as i64 * 10;
        let mut ins_a = make_insurer(InsurerId(1), capital);
        let mut ins_b = make_insurer(InsurerId(2), capital);

        ins_a.on_policy_bound(PolicyId(1), ASSET_VALUE, 0, &[Peril::Attritional]);
        ins_b.on_policy_bound(PolicyId(2), ASSET_VALUE, 0, &[Peril::Attritional]);

        // ins_a: 100% loss; ins_b: no claims
        let _ = ins_a.on_claim_settled(Day(0), ASSET_VALUE, Peril::Attritional);

        let _ = ins_a.on_year_end(Day(360), ASSET_VALUE);
        let _ = ins_b.on_year_end(Day(360), ASSET_VALUE);

        let atp_a = quote_atp(&ins_a);
        let atp_b = quote_atp(&ins_b);
        assert!(
            atp_a > atp_b,
            "ins_a (100% loss year) must have higher ATP than ins_b (benign year): {atp_a} vs {atp_b}"
        );
    }

    #[test]
    fn two_insurers_diverge_in_capacity_after_asymmetric_cat_loss() {
        // Both start at 15M USD capital with net_line_capacity=0.30.
        // ins_a is drained to ~5M USD → max_line = 0.30 × 5M = 1.5M < 25M sum_insured → declines.
        // ins_b is untouched → max_line = 0.30 × 15M = 4.5M < 25M → also declined?
        // Use larger capital so ins_b can still write the risk.
        // ins_b capital = 100M USD → max_line = 30M > ASSET_VALUE ✓
        // ins_a drained to 5M USD → max_line = 1.5M < ASSET_VALUE → MaxLineSizeExceeded
        let capital_b = 10_000_000_000i64; // 100M USD in cents
        let capital_a = 10_000_000_000i64;

        let mut ins_a = Insurer::new(InsurerId(1), capital_a, 0.239, 0.0, 0.70, 0.3, 0.0, 0.0, Some(0.30), Some(0.30), 0.252, 0.0);
        let ins_b = Insurer::new(InsurerId(2), capital_b, 0.239, 0.0, 0.70, 0.3, 0.0, 0.0, Some(0.30), Some(0.30), 0.252, 0.0);

        ins_a.on_policy_bound(PolicyId(1), ASSET_VALUE, 0, &[Peril::WindstormAtlantic]);

        // Drain ins_a to ~5M USD (500_000_000 cents) via cat claims
        let drain = capital_a - 500_000_000;
        let _ = ins_a.on_claim_settled(Day(10), drain as u64, Peril::WindstormAtlantic);
        assert!(ins_a.capital < 600_000_000, "ins_a must be nearly depleted: {}", ins_a.capital);

        // Submit identical 25M USD cat risk to both
        let risk = cat_risk();
        let (_, event_a) = first_event(ins_a.on_lead_quote_requested(Day(20), SubmissionId(1), InsuredId(1), &risk, 1.0));
        let (_, event_b) = first_event(ins_b.on_lead_quote_requested(Day(20), SubmissionId(2), InsuredId(2), &risk, 1.0));

        assert!(
            matches!(event_a, Event::LeadQuoteDeclined { reason: DeclineReason::MaxLineSizeExceeded, .. }),
            "depleted ins_a must decline with MaxLineSizeExceeded, got {event_a:?}"
        );
        assert!(
            matches!(event_b, Event::LeadQuoteIssued { .. }),
            "well-capitalised ins_b must issue a quote, got {event_b:?}"
        );
    }

    #[test]
    fn atp_divergence_grows_over_multiple_years() {
        // ins_a incurs 100% attritional loss each year; ins_b has benign years.
        // The ATP gap must widen year-over-year as EWMA credibility accumulates.
        let capital = ASSET_VALUE as i64 * 10;
        let mut ins_a = make_insurer(InsurerId(1), capital);
        let mut ins_b = make_insurer(InsurerId(2), capital);

        let mut gap_yr1 = 0i64;

        for year in 0..3u64 {
            let pid_a = PolicyId(year * 2 + 1);
            let pid_b = PolicyId(year * 2 + 2);

            ins_a.on_policy_bound(pid_a, ASSET_VALUE, 0, &[Peril::Attritional]);
            ins_b.on_policy_bound(pid_b, ASSET_VALUE, 0, &[Peril::Attritional]);

            let _ = ins_a.on_claim_settled(Day(0), ASSET_VALUE, Peril::Attritional);
            // ins_b: no claims

            let _ = ins_a.on_year_end(Day(360), ASSET_VALUE);
            let _ = ins_b.on_year_end(Day(360), ASSET_VALUE);

            let gap = quote_atp(&ins_a) as i64 - quote_atp(&ins_b) as i64;
            if year == 0 {
                gap_yr1 = gap;
            } else if year == 2 {
                assert!(
                    gap > gap_yr1,
                    "ATP gap after 3 years ({gap}) must exceed gap after year 1 ({gap_yr1}) — divergence compounds"
                );
            }
        }
    }

    // ── Per-insurer capital-state pricing ─────────────────────────────────────

    /// Helper: quote and return the premium (not ATP) for a standard attritional risk.
    fn quote_premium(ins: &Insurer, market_factor: f64) -> u64 {
        let risk = Risk {
            sum_insured: ASSET_VALUE,
            territory: "US-SE".to_string(),
            perils_covered: vec![Peril::Attritional],
        };
        let events = ins.on_lead_quote_requested(Day(0), SubmissionId(1), InsuredId(1), &risk, market_factor);
        let (_, event) = events.into_iter().next().unwrap();
        if let Event::LeadQuoteIssued { premium, .. } = event {
            premium
        } else {
            panic!("expected LeadQuoteIssued, got {event:?}")
        }
    }

    #[test]
    fn new_insurer_uses_market_factor_when_no_experience() {
        // own_years=0 → credibility=0 → insurer_ap_tp = market_factor exactly.
        // depletion_sensitivity=1.0; capital=initial → no depletion adj.
        let ins = Insurer::new(InsurerId(1), 1_000_000, 0.239, 0.0, 0.70, 0.3, 0.0, 0.0, None, None, 0.252, 1.0);
        let market_factor = 1.20;
        let premium = quote_premium(&ins, market_factor);

        // TP = ATP × (1 + profit_loading=0) = ATP; expected = ATP × 1.20
        let atp = (0.239 * ASSET_VALUE as f64 / 0.70).round() as u64;
        let expected = (atp as f64 * market_factor).round() as u64;
        assert_eq!(premium, expected,
            "new entrant (own_years=0) must follow market factor exactly: got {premium}, expected {expected}");
    }

    #[test]
    fn depleted_insurer_quotes_above_market_with_full_credibility() {
        // 30% depletion, own_years=5 (credibility=1.0), no loss history → own_cr_signal=0.0
        // cap_depletion_adj = clamp(0.30 × 1.0, 0, 0.30) = 0.30
        // own_factor = clamp(1.0 + 0.0 + 0.30, 0.90, 1.40) = 1.30
        // insurer_ap_tp = 1.0 × 1.30 + 0.0 × market_factor = 1.30
        let initial_capital = 1_000_000i64;
        let current_capital = 700_000i64; // 30% depletion
        let mut ins = Insurer::new(InsurerId(1), initial_capital, 0.239, 0.0, 0.70, 0.3, 0.0, 0.0, None, None, 0.252, 1.0);
        ins.capital = current_capital;
        ins.own_years = 5; // full credibility

        let premium = quote_premium(&ins, 1.0);
        let atp = (0.239 * ASSET_VALUE as f64 / 0.70).round() as u64;
        let expected = (atp as f64 * 1.30).round() as u64;
        assert_eq!(premium, expected,
            "depleted insurer with full credibility must quote at own_factor=1.30: got {premium}, expected {expected}");
    }

    #[test]
    fn own_cr_signal_elevated_after_loss_year_raises_own_factor() {
        // No capital depletion (capital=initial); credibility=1.0 (own_years=5).
        // Bind one policy, settle 200% claim relative to premium → LR=2.0.
        // own_cr_signal = clamp((2.0 + 0.0) − 1.0, −0.25, 0.40) = 0.40 (expense_ratio=0.0)
        // own_factor = clamp(1.0 + 0.40 + 0.0, 0.90, 1.40) = 1.40
        // insurer_ap_tp = 1.0 × 1.40 = 1.40 > neutral market_factor=1.0
        let capital = 100_000_000i64;
        let mut ins = Insurer::new(InsurerId(1), capital, 0.239, 0.0, 0.70, 0.3, 0.0, 0.0, None, None, 0.252, 1.0);
        ins.own_years = 5;

        // Record a very high-loss year: premium=P, claims=2P → LR=2.0
        let premium = 1_000_000u64;
        ins.on_policy_bound(PolicyId(1), ASSET_VALUE, premium, &[Peril::Attritional]);
        let _ = ins.on_claim_settled(Day(10), premium * 2, Peril::Attritional);
        let _ = ins.on_year_end(Day(360), ASSET_VALUE);

        // TP is computed from the *current* (post-EWMA) ATP. own_factor=1.40 > 1.0,
        // so premium = current_ATP × 1.40 > current_ATP × 1.0 = TP.
        let current_atp = quote_atp(&ins);
        let premium_quoted = quote_premium(&ins, 1.0);
        assert!(
            premium_quoted > current_atp,
            "elevated own CR signal must push premium above TP×1.0 (neutral): got {premium_quoted}, TP={current_atp}"
        );
    }

    #[test]
    fn on_year_end_increments_own_years_and_pushes_lr() {
        // After one YearEnd with premium written, own_years goes from 0 to 1.
        // A new insurer with own_years=0 follows market exactly;
        // after YearEnd (own_years=1, credibility=0.2), the blend shifts toward own_factor.
        let capital = 100_000_000i64;
        let mut ins = Insurer::new(InsurerId(1), capital, 0.239, 0.0, 0.70, 0.3, 0.0, 0.0, None, None, 0.252, 1.0);

        // Before YearEnd: own_years=0
        assert_eq!(ins.own_years, 0);

        // Bind and push a high-loss year so own_factor will differ from market
        let premium = 1_000_000u64;
        ins.on_policy_bound(PolicyId(1), ASSET_VALUE, premium, &[Peril::Attritional]);
        let _ = ins.on_claim_settled(Day(10), premium * 4, Peril::Attritional);
        let _ = ins.on_year_end(Day(360), ASSET_VALUE);

        assert_eq!(ins.own_years, 1, "own_years must increment to 1 after one YearEnd");
        assert_eq!(ins.own_loss_ratios.len(), 1, "one LR must be recorded");

        // With own_years=1 credibility=0.2; own_factor elevated (high LR).
        // At market_factor=0.90 (soft), premium must still be > market's pure TP × 0.90
        // because own experience is bad. With market=1.40 (hard), premium could be ≤ own factor.
        // Just assert own_years was incremented and LR was recorded (structural test).
    }

    #[test]
    fn partial_credibility_blends_own_and_market_factors() {
        // own_years=2 → credibility=0.4
        // Record LR=2.0 (claims=2×premium, expense_ratio=0.0 → CR=2.0)
        // own_cr_signal = clamp(2.0 − 1.0, −0.25, 0.40) = 0.40 (capped)
        // own_factor = clamp(1.0 + 0.40 + 0.0, 0.90, 1.40) = 1.40 (capped)
        // market_factor = 0.90
        // insurer_ap_tp = 0.4 × 1.40 + 0.6 × 0.90 = 0.56 + 0.54 = 1.10
        let capital = 100_000_000i64;
        let mut ins = Insurer::new(InsurerId(1), capital, 0.239, 0.0, 0.70, 0.3, 0.0, 0.0, None, None, 0.252, 1.0);
        ins.own_years = 2;

        // Record one high-loss year: LR=2.0
        let premium = 1_000_000u64;
        ins.on_policy_bound(PolicyId(1), ASSET_VALUE, premium, &[Peril::Attritional]);
        let _ = ins.on_claim_settled(Day(10), premium * 2, Peril::Attritional);
        // Manually push LR into buffer without triggering another on_year_end increment
        // Use on_year_end which also increments own_years; compensate by pre-setting own_years=1
        ins.own_years = 1; // will become 2 after on_year_end
        let _ = ins.on_year_end(Day(360), ASSET_VALUE);
        assert_eq!(ins.own_years, 2, "own_years should be 2 after one more YearEnd");

        // Use post-EWMA ATP for the expected value; EWMA updated attritional_elf during on_year_end.
        // The blend factor should be 0.4×1.40 + 0.6×0.90 = 1.10 regardless of the ATP level.
        let current_atp = quote_atp(&ins);
        let expected = (current_atp as f64 * 1.10).round() as u64;
        let premium_quoted = quote_premium(&ins, 0.90);
        assert_eq!(premium_quoted, expected,
            "partial credibility blend: 0.4×1.40 + 0.6×0.90 = 1.10; got {premium_quoted}, expected {expected}");
    }

}
