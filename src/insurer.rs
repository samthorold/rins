use std::collections::HashMap;

use crate::events::{Event, Peril, Risk};
use crate::types::{Day, InsuredId, InsurerId, PolicyId, SubmissionId};

/// A single insurer in the minimal property market.
/// Writes 100% of each risk it quotes (lead-only, no follow market).
/// Capital is re-endowed at each YearStart — no insolvency in this model.
pub struct Insurer {
    pub id: InsurerId,
    /// Current capital (signed to allow negative without panicking).
    pub capital: i64,
    pub initial_capital: i64,
    /// Actuarial channel: E[annual_loss] / sum_insured across all perils.
    expected_loss_fraction: f64,
    /// Actuarial channel: ATP = expected_loss_fraction / target_loss_ratio.
    target_loss_ratio: f64,
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
        max_cat_aggregate: Option<u64>,
        max_line_size: Option<u64>,
    ) -> Self {
        Insurer {
            id,
            capital: initial_capital,
            initial_capital,
            expected_loss_fraction,
            target_loss_ratio,
            cat_aggregate: 0,
            max_cat_aggregate,
            max_line_size,
            cat_policy_map: HashMap::new(),
        }
    }

    /// Reset capital to initial_capital at the start of each year.
    pub fn on_year_start(&mut self) {
        self.capital = self.initial_capital;
    }

    /// Price and issue a lead quote for a risk. Always quotes (no capacity checks).
    /// Logs both the actuarial technical price (ATP) and the underwriter premium.
    /// Future: check max_cat_aggregate / max_line_size and return LeadQuoteRejected if breached.
    pub fn on_lead_quote_requested(
        &self,
        day: Day,
        submission_id: SubmissionId,
        insured_id: InsuredId,
        risk: &Risk,
    ) -> (Day, Event) {
        let atp = self.actuarial_price(risk);
        let premium = self.underwriter_premium(risk);
        let cat_exposure_at_quote = if risk.perils_covered.contains(&Peril::WindstormAtlantic) {
            self.cat_aggregate
        } else {
            0
        };
        (
            day,
            Event::LeadQuoteIssued {
                submission_id,
                insured_id,
                insurer_id: self.id,
                atp,
                premium,
                cat_exposure_at_quote,
            },
        )
    }

    /// A policy has been bound. Update WindstormAtlantic aggregate if the risk covers cat.
    pub fn on_policy_bound(&mut self, policy_id: PolicyId, sum_insured: u64, perils: &[Peril]) {
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

    /// Deduct a settled claim from capital (can go negative — no insolvency logic yet).
    pub fn on_claim_settled(&mut self, amount: u64) {
        self.capital -= amount as i64;
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
        Insurer::new(id, capital, 0.239, 0.70, None, None)
    }

    #[test]
    fn on_year_start_resets_capital() {
        let mut ins = make_insurer(InsurerId(1), 1_000_000);
        ins.capital = 500_000; // depleted
        ins.on_year_start();
        assert_eq!(ins.capital, ins.initial_capital);
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

    #[test]
    fn on_lead_quote_requested_always_quotes() {
        let ins = make_insurer(InsurerId(1), 1_000_000_000);
        let risk = small_risk();
        let (_, event) = ins.on_lead_quote_requested(Day(0), SubmissionId(1), InsuredId(1), &risk);
        assert!(
            matches!(event, Event::LeadQuoteIssued { .. }),
            "insurer must always issue a lead quote, got {event:?}"
        );
    }

    #[test]
    fn premium_equals_atp() {
        let ins = make_insurer(InsurerId(1), 1_000_000_000);
        let risk = small_risk();
        let (_, event) = ins.on_lead_quote_requested(Day(0), SubmissionId(1), InsuredId(1), &risk);
        if let Event::LeadQuoteIssued { atp, premium, .. } = event {
            assert_eq!(premium, atp, "Step 0 technical pricing: premium must equal ATP");
        }
    }

    #[test]
    fn lead_quote_issued_carries_insured_id() {
        let ins = make_insurer(InsurerId(1), 1_000_000_000);
        let risk = small_risk();
        let (_, event) =
            ins.on_lead_quote_requested(Day(0), SubmissionId(5), InsuredId(42), &risk);
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
            ins.on_lead_quote_requested(Day(0), SubmissionId(1), InsuredId(1), &small);
        let (_, e_large) =
            ins.on_lead_quote_requested(Day(0), SubmissionId(2), InsuredId(2), &large);
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
        let (_, event) = ins.on_lead_quote_requested(Day(0), SubmissionId(1), InsuredId(1), &risk);
        if let Event::LeadQuoteIssued { premium, .. } = event {
            assert!(premium > 0, "premium must be positive for a non-trivial risk");
        }
    }

    #[test]
    fn atp_equals_expected_loss_over_target_ratio() {
        let ins = make_insurer(InsurerId(1), 0);
        let risk = small_risk();
        let expected = (0.239 * ASSET_VALUE as f64 / 0.70).round() as u64;
        let (_, event) = ins.on_lead_quote_requested(Day(0), SubmissionId(1), InsuredId(1), &risk);
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
        let (_, event) = ins.on_lead_quote_requested(Day(0), SubmissionId(1), InsuredId(1), &risk);
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
        ins.on_policy_bound(PolicyId(1), ASSET_VALUE, &[Peril::WindstormAtlantic]);
        assert_eq!(ins.cat_aggregate, ASSET_VALUE, "cat_aggregate must equal sum_insured after binding one cat policy");
    }

    #[test]
    fn on_policy_expired_releases_cat_aggregate() {
        let mut ins = make_insurer(InsurerId(1), 0);
        ins.on_policy_bound(PolicyId(1), ASSET_VALUE, &[Peril::WindstormAtlantic]);
        assert_eq!(ins.cat_aggregate, ASSET_VALUE);
        ins.on_policy_expired(PolicyId(1));
        assert_eq!(ins.cat_aggregate, 0, "cat_aggregate must return to 0 after policy expiry");
    }

    #[test]
    fn non_cat_policy_does_not_affect_cat_aggregate() {
        let mut ins = make_insurer(InsurerId(1), 0);
        ins.on_policy_bound(PolicyId(1), ASSET_VALUE, &[Peril::Attritional]);
        assert_eq!(ins.cat_aggregate, 0, "attritional-only policy must not affect cat_aggregate");
    }

    #[test]
    fn cat_exposure_at_quote_reflects_aggregate() {
        let mut ins = make_insurer(InsurerId(1), 0);
        // Bind a cat policy first.
        ins.on_policy_bound(PolicyId(1), ASSET_VALUE, &[Peril::WindstormAtlantic]);

        // Quote a second cat risk — exposure_at_quote should reflect the already-bound aggregate.
        let risk = cat_risk();
        let (_, event) = ins.on_lead_quote_requested(Day(0), SubmissionId(2), InsuredId(2), &risk);
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
        ins.on_policy_bound(PolicyId(1), ASSET_VALUE, &[Peril::WindstormAtlantic]);

        let risk = att_only_risk();
        let (_, event) = ins.on_lead_quote_requested(Day(0), SubmissionId(2), InsuredId(2), &risk);
        if let Event::LeadQuoteIssued { cat_exposure_at_quote, .. } = event {
            assert_eq!(
                cat_exposure_at_quote, 0,
                "cat_exposure_at_quote must be 0 for a risk that doesn't cover WindstormAtlantic"
            );
        } else {
            panic!("expected LeadQuoteIssued");
        }
    }
}
