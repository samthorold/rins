use std::collections::HashMap;

use crate::events::{DeclineReason, Event, Peril, Risk};
use crate::types::{Day, InsuredId, InsurerId, PolicyId, SubmissionId};

/// A single insurer in the minimal property market.
/// Writes 100% of each risk it quotes (lead-only, no follow market).
/// Capital is endowed once at construction and persists year-over-year; premiums add, claims deduct.
pub struct Insurer {
    pub id: InsurerId,
    /// Current capital (signed to allow negative without panicking).
    pub capital: i64,
    /// Actuarial channel: live E[annual_loss] / sum_insured, updated each YearEnd via EWMA.
    expected_loss_fraction: f64,
    /// Actuarial channel: ATP = expected_loss_fraction / target_loss_ratio.
    target_loss_ratio: f64,
    /// EWMA credibility weight α: new_elf = α × realized_lf + (1-α) × old_elf.
    ewma_credibility: f64,
    /// Fraction of gross premium consumed by acquisition costs + overhead.
    expense_ratio: f64,
    /// YTD settled claims (cents); accumulated by on_claim_settled; reset at YearEnd.
    year_claims: u64,
    /// YTD written exposure (sum_insured, cents); accumulated by on_policy_bound; reset at YearEnd.
    year_exposure: u64,
    /// Exposure management: live WindstormAtlantic aggregate sum_insured.
    pub cat_aggregate: u64,
    /// Max WindstormAtlantic aggregate (None = unlimited).
    pub max_cat_aggregate: Option<u64>,
    /// Max sum_insured on any single risk (None = unlimited).
    pub max_line_size: Option<u64>,
    /// Map from policy_id to its WindstormAtlantic sum_insured, for release on expiry.
    cat_policy_map: HashMap<PolicyId, u64>,
}

impl Insurer {
    pub fn new(
        id: InsurerId,
        initial_capital: i64,
        expected_loss_fraction: f64,
        target_loss_ratio: f64,
        ewma_credibility: f64,
        expense_ratio: f64,
        max_cat_aggregate: Option<u64>,
        max_line_size: Option<u64>,
    ) -> Self {
        Insurer {
            id,
            capital: initial_capital,
            expected_loss_fraction,
            target_loss_ratio,
            ewma_credibility,
            expense_ratio,
            year_claims: 0,
            year_exposure: 0,
            cat_aggregate: 0,
            max_cat_aggregate,
            max_line_size,
            cat_policy_map: HashMap::new(),
        }
    }

    /// Called at each YearStart. Capital is NOT reset — it persists from prior year.
    pub fn on_year_start(&mut self) {}

    /// Price and issue a lead quote for a risk, or decline if an exposure limit is breached.
    /// Returns a single `LeadQuoteIssued` or `LeadQuoteDeclined` event.
    pub fn on_lead_quote_requested(
        &self,
        day: Day,
        submission_id: SubmissionId,
        insured_id: InsuredId,
        risk: &Risk,
    ) -> Vec<(Day, Event)> {
        if let Some(max_line) = self.max_line_size
            && risk.sum_insured > max_line
        {
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
        if let Some(max_agg) = self.max_cat_aggregate
            && risk.perils_covered.contains(&Peril::WindstormAtlantic)
            && self.cat_aggregate + risk.sum_insured > max_agg
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
        let atp = self.actuarial_price(risk);
        let premium = self.underwriter_premium(risk);
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
        self.year_exposure += sum_insured;
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

    /// Actuarial channel: E[annual_loss] / target_loss_ratio.
    /// Future: replace expected_loss_fraction with EWMA from observed burning cost.
    fn actuarial_price(&self, risk: &Risk) -> u64 {
        (self.expected_loss_fraction * risk.sum_insured as f64 / self.target_loss_ratio).round() as u64
    }

    /// Underwriter channel: Step 0 — technical pricing baseline, premium = ATP.
    /// Future: apply cycle indicator, relationship score, lead-quote anchoring
    /// as a multiplicative factor on ATP (premium = ATP × underwriter_factor).
    fn underwriter_premium(&self, risk: &Risk) -> u64 {
        self.actuarial_price(risk)
    }

    /// Deduct a settled claim from capital and accumulate YTD claims for EWMA.
    pub fn on_claim_settled(&mut self, amount: u64) {
        self.capital -= amount as i64;
        self.year_claims += amount;
    }

    /// Update expected_loss_fraction via EWMA from this year's realized burning cost,
    /// then reset YTD accumulators. No-op if no exposure was written this year.
    pub fn on_year_end(&mut self) {
        if self.year_exposure > 0 {
            let realized_lf = self.year_claims as f64 / self.year_exposure as f64;
            self.expected_loss_fraction = self.ewma_credibility * realized_lf
                + (1.0 - self.ewma_credibility) * self.expected_loss_fraction;
        }
        self.year_claims = 0;
        self.year_exposure = 0;
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
        Insurer::new(id, capital, 0.239, 0.70, 0.3, 0.0, None, None)
    }

    /// Helper: quote and return the ATP for a standard small_risk().
    fn quote_atp(ins: &Insurer) -> u64 {
        let risk = Risk {
            sum_insured: ASSET_VALUE,
            territory: "US-SE".to_string(),
            perils_covered: vec![Peril::Attritional],
        };
        let events = ins.on_lead_quote_requested(Day(0), SubmissionId(1), InsuredId(1), &risk);
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
        ins.on_claim_settled(300_000);
        assert_eq!(ins.capital, 700_000);
    }

    #[test]
    fn on_claim_settled_can_go_negative() {
        let mut ins = make_insurer(InsurerId(1), 100);
        ins.on_claim_settled(1_000_000);
        assert!(ins.capital < 0, "capital should go negative without panicking");
    }

    fn first_event(events: Vec<(Day, Event)>) -> (Day, Event) {
        events.into_iter().next().unwrap()
    }

    #[test]
    fn on_lead_quote_requested_always_quotes() {
        let ins = make_insurer(InsurerId(1), 1_000_000_000);
        let risk = small_risk();
        let (_, event) = first_event(ins.on_lead_quote_requested(Day(0), SubmissionId(1), InsuredId(1), &risk));
        assert!(
            matches!(event, Event::LeadQuoteIssued { .. }),
            "insurer must always issue a lead quote, got {event:?}"
        );
    }

    #[test]
    fn premium_equals_atp() {
        let ins = make_insurer(InsurerId(1), 1_000_000_000);
        let risk = small_risk();
        let (_, event) = first_event(ins.on_lead_quote_requested(Day(0), SubmissionId(1), InsuredId(1), &risk));
        if let Event::LeadQuoteIssued { atp, premium, .. } = event {
            assert_eq!(premium, atp, "Step 0 technical pricing: premium must equal ATP");
        }
    }

    #[test]
    fn lead_quote_issued_carries_insured_id() {
        let ins = make_insurer(InsurerId(1), 1_000_000_000);
        let risk = small_risk();
        let (_, event) =
            first_event(ins.on_lead_quote_requested(Day(0), SubmissionId(5), InsuredId(42), &risk));
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
            first_event(ins.on_lead_quote_requested(Day(0), SubmissionId(1), InsuredId(1), &small));
        let (_, e_large) =
            first_event(ins.on_lead_quote_requested(Day(0), SubmissionId(2), InsuredId(2), &large));
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
        let (_, event) = first_event(ins.on_lead_quote_requested(Day(0), SubmissionId(1), InsuredId(1), &risk));
        if let Event::LeadQuoteIssued { premium, .. } = event {
            assert!(premium > 0, "premium must be positive for a non-trivial risk");
        }
    }

    #[test]
    fn atp_equals_expected_loss_over_target_ratio() {
        let ins = make_insurer(InsurerId(1), 0);
        let risk = small_risk();
        let expected = (0.239 * ASSET_VALUE as f64 / 0.70).round() as u64;
        let (_, event) = first_event(ins.on_lead_quote_requested(Day(0), SubmissionId(1), InsuredId(1), &risk));
        if let Event::LeadQuoteIssued { atp, .. } = event {
            assert_eq!(atp, expected, "ATP must equal expected_loss_fraction × sum_insured / target_loss_ratio");
        } else {
            panic!("expected LeadQuoteIssued");
        }
    }

    #[test]
    fn premium_equals_expected_loss_fraction_over_target_ratio() {
        // Step 0 technical pricing: premium = ATP = expected_loss_fraction × sum_insured / target_loss_ratio.
        let ins = make_insurer(InsurerId(1), 0);
        let risk = small_risk();
        let expected = (0.239 * ASSET_VALUE as f64 / 0.70).round() as u64;
        let (_, event) = first_event(ins.on_lead_quote_requested(Day(0), SubmissionId(1), InsuredId(1), &risk));
        if let Event::LeadQuoteIssued { premium, .. } = event {
            assert_eq!(premium, expected, "premium must equal expected_loss_fraction × sum_insured / target_loss_ratio");
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
        let (_, event) = first_event(ins.on_lead_quote_requested(Day(0), SubmissionId(2), InsuredId(2), &risk));
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
        let (_, event) = first_event(ins.on_lead_quote_requested(Day(0), SubmissionId(2), InsuredId(2), &risk));
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
        let ins = Insurer::new(InsurerId(1), 0, 0.239, 0.70, 0.3, 0.0, None, Some(ASSET_VALUE - 1));
        let risk = cat_risk(); // sum_insured = ASSET_VALUE > max_line_size
        let (_, event) = first_event(ins.on_lead_quote_requested(Day(0), SubmissionId(1), InsuredId(1), &risk));
        assert!(
            matches!(event, Event::LeadQuoteDeclined { reason: DeclineReason::MaxLineSizeExceeded, .. }),
            "expected LeadQuoteDeclined(MaxLineSizeExceeded), got {event:?}"
        );
    }

    #[test]
    fn max_cat_aggregate_breached_emits_declined() {
        let ins = Insurer::new(InsurerId(1), 0, 0.239, 0.70, 0.3, 0.0, Some(0), None);
        let risk = cat_risk(); // cat_aggregate(0) + sum_insured > max_cat_aggregate(0)
        let (_, event) = first_event(ins.on_lead_quote_requested(Day(0), SubmissionId(1), InsuredId(1), &risk));
        assert!(
            matches!(event, Event::LeadQuoteDeclined { reason: DeclineReason::MaxCatAggregateBreached, .. }),
            "expected LeadQuoteDeclined(MaxCatAggregateBreached), got {event:?}"
        );
    }

    #[test]
    fn within_limits_after_partial_fill_emits_quote_issued() {
        let mut ins = Insurer::new(InsurerId(1), 0, 0.239, 0.70, 0.3, 0.0, Some(2 * ASSET_VALUE), None);
        ins.on_policy_bound(PolicyId(1), ASSET_VALUE, 0, &[Peril::WindstormAtlantic]);
        // cat_aggregate = ASSET_VALUE, max = 2×ASSET_VALUE → still room for one more
        let risk = cat_risk();
        let (_, event) = first_event(ins.on_lead_quote_requested(Day(0), SubmissionId(2), InsuredId(2), &risk));
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
        let mut ins = make_insurer(InsurerId(1), 0);
        let atp_before = quote_atp(&ins);
        ins.on_policy_bound(PolicyId(1), ASSET_VALUE, 0, &[Peril::Attritional]);
        ins.on_claim_settled(ASSET_VALUE);
        ins.on_year_end();
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
        ins.on_year_end();
        let atp_after = quote_atp(&ins);
        assert!(atp_after < atp_before, "ATP must fall after a 0% LF year: {atp_after} vs {atp_before}");
    }

    #[test]
    fn ewma_formula_matches_exact_calculation() {
        // α=0.3, realized LF = 0.5 (claim = ASSET_VALUE/2, exposure = ASSET_VALUE).
        // New ELF = 0.3 × 0.5 + 0.7 × 0.239 = 0.15 + 0.1673 = 0.3173.
        let mut ins = make_insurer(InsurerId(1), 0);
        ins.on_policy_bound(PolicyId(1), ASSET_VALUE, 0, &[Peril::Attritional]);
        ins.on_claim_settled(ASSET_VALUE / 2);
        ins.on_year_end();
        let expected_elf = 0.3 * 0.5 + 0.7 * 0.239;
        let expected_atp = (expected_elf * ASSET_VALUE as f64 / 0.70).round() as u64;
        assert_eq!(quote_atp(&ins), expected_atp, "EWMA must match α × realized + (1-α) × prior");
    }

    #[test]
    fn on_year_end_with_no_exposure_leaves_atp_unchanged() {
        let mut ins = make_insurer(InsurerId(1), 0);
        let atp_before = quote_atp(&ins);
        ins.on_year_end(); // no policies bound, no claims
        assert_eq!(quote_atp(&ins), atp_before, "ATP must not change if no exposure was written");
    }

    #[test]
    fn on_year_end_resets_so_second_call_without_new_data_is_noop() {
        // After on_year_end resets counters, a second on_year_end with no new
        // policies or claims must leave ATP unchanged.
        let mut ins = make_insurer(InsurerId(1), 0);
        ins.on_policy_bound(PolicyId(1), ASSET_VALUE, 0, &[Peril::Attritional]);
        ins.on_claim_settled(ASSET_VALUE);
        ins.on_year_end(); // ELF updated, counters reset
        let atp_year1 = quote_atp(&ins);
        ins.on_year_end(); // no new data → noop
        assert_eq!(quote_atp(&ins), atp_year1, "second on_year_end with no data must be a noop");
    }

    #[test]
    fn ewma_compounds_over_multiple_years() {
        // Two consecutive high-loss years should push ELF higher than one.
        let mut ins = make_insurer(InsurerId(1), 0);
        ins.on_policy_bound(PolicyId(1), ASSET_VALUE, 0, &[Peril::Attritional]);
        ins.on_claim_settled(ASSET_VALUE);
        ins.on_year_end();
        let atp_after_year1 = quote_atp(&ins);

        ins.on_policy_bound(PolicyId(2), ASSET_VALUE, 0, &[Peril::Attritional]);
        ins.on_claim_settled(ASSET_VALUE);
        ins.on_year_end();
        let atp_after_year2 = quote_atp(&ins);

        assert!(atp_after_year2 > atp_after_year1, "consecutive bad years must compound ELF upward");
    }

    #[test]
    fn on_policy_bound_credits_net_premium_to_capital() {
        // expense_ratio=0.25 → net = 75% of gross premium.
        let mut ins = Insurer::new(InsurerId(1), 1_000_000, 0.239, 0.55, 0.3, 0.25, None, None);
        let gross_premium = 400_000u64;
        ins.on_policy_bound(PolicyId(1), ASSET_VALUE, gross_premium, &[Peril::Attritional]);
        let expected_net = (gross_premium as f64 * 0.75).round() as i64;
        assert_eq!(
            ins.capital,
            1_000_000 + expected_net,
            "capital must increase by net premium (gross × (1 − expense_ratio))"
        );
    }
}
