use std::collections::HashMap;

use crate::events::{DeclineReason, Event, Peril, Risk};
use crate::types::{Day, InsuredId, InsurerId, PolicyId, SubmissionId};

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
    /// YTD attritional claims (cents); accumulated by on_claim_settled (Attritional only);
    /// reset at YearEnd. Used to update attritional_elf via EWMA. Cat claims are excluded.
    year_attritional_claims: u64,
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
    /// How strongly the underwriter reacts to own prior-year loss ratio deviating from target.
    /// 0.0 = through-cycle (no reaction); 0.5 = cycle trader (aggressive repricing).
    cycle_sensitivity: f64,
    /// Gross premium written this year (cents); accumulated by on_policy_bound; reset at YearEnd.
    year_premium: u64,
    /// Total claims paid this year (att + cat, cents); accumulated by on_claim_settled; reset at YearEnd.
    year_total_claims: u64,
    /// Own loss ratio from the prior year: year_total_claims / year_premium.
    /// Initialised to target_loss_ratio so no cycle adjustment fires in year 1.
    prior_year_loss_ratio: f64,
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
        max_cat_aggregate: Option<u64>,
        max_line_size: Option<u64>,
        cycle_sensitivity: f64,
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
            year_attritional_claims: 0,
            year_exposure: 0,
            cat_aggregate: 0,
            max_cat_aggregate,
            max_line_size,
            cat_policy_map: HashMap::new(),
            cycle_sensitivity,
            year_premium: 0,
            year_total_claims: 0,
            prior_year_loss_ratio: target_loss_ratio,
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
        self.year_premium += premium;
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

    /// Underwriter channel: Step 1 — own-CR cycle adjustment above ATP floor.
    /// cycle_adj = max(0, cycle_sensitivity × (prior_year_loss_ratio − target_loss_ratio))
    /// uw_factor = profit_loading + cycle_adj
    /// premium   = ATP × (1 + uw_factor)
    /// The floor on cycle_adj enforces the MS3 minimum AvT invariant: premiums never fall
    /// below ATP × (1 + profit_loading) regardless of prior-year loss ratio.
    fn underwriter_premium(&self, risk: &Risk) -> u64 {
        let atp = self.actuarial_price(risk);
        let cycle_adj = (self.cycle_sensitivity
            * (self.prior_year_loss_ratio - self.target_loss_ratio))
            .max(0.0);
        let uw_factor = self.profit_loading + cycle_adj;
        (atp as f64 * (1.0 + uw_factor)).round() as u64
    }

    /// Deduct a settled claim from capital (floored at zero).
    /// Only attritional claims are accumulated for the EWMA; cat claims are excluded
    /// because cat_elf is anchored and not updated from experience.
    /// Returns `InsurerInsolvent` on the first crossing to zero; empty otherwise.
    pub fn on_claim_settled(&mut self, day: Day, amount: u64, peril: Peril) -> Vec<(Day, Event)> {
        let payable = amount.min(self.capital.max(0) as u64);
        self.capital -= payable as i64; // floors at 0 naturally
        if peril == Peril::Attritional {
            self.year_attritional_claims += payable;
        }
        self.year_total_claims += payable;

        if self.capital == 0 && !self.insolvent {
            self.insolvent = true;
            vec![(day, Event::InsurerInsolvent { insurer_id: self.id })]
        } else {
            vec![]
        }
    }

    /// Update attritional_elf via EWMA from this year's realized attritional burning cost,
    /// then reset YTD accumulators. cat_elf is never updated. No-op if no exposure written.
    pub fn on_year_end(&mut self) {
        if self.year_exposure > 0 {
            let realized_att_lf =
                self.year_attritional_claims as f64 / self.year_exposure as f64;
            self.attritional_elf = self.ewma_credibility * realized_att_lf
                + (1.0 - self.ewma_credibility) * self.attritional_elf;
        }
        if self.year_premium > 0 {
            self.prior_year_loss_ratio =
                self.year_total_claims as f64 / self.year_premium as f64;
        }
        self.year_attritional_claims = 0;
        self.year_exposure = 0;
        self.year_total_claims = 0;
        self.year_premium = 0;
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
        // attritional_elf=0.239, cat_elf=0.0, profit_loading=0.0, cycle_sensitivity=0.0 → premium = ATP (tests unchanged)
        Insurer::new(id, capital, 0.239, 0.0, 0.70, 0.3, 0.0, 0.0, None, None, 0.0)
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
        let mut ins = Insurer::new(InsurerId(1), initial_capital, 0.239, 0.0, 0.55, 0.3, 0.0, 0.0, None, None, 0.0);
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
        let (_, event) = first_event(ins.on_lead_quote_requested(Day(0), SubmissionId(1), InsuredId(1), &risk));
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
        let (_, event) = first_event(ins.on_lead_quote_requested(Day(0), SubmissionId(1), InsuredId(1), &risk));
        if let Event::LeadQuoteIssued { atp, premium, .. } = event {
            assert_eq!(premium, atp, "with profit_loading=0.0, premium must equal ATP");
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
    fn premium_equals_atp_times_loading() {
        // make_insurer uses attritional_elf=0.239, cat_elf=0.0, profit_loading=0.0.
        // premium = (0.239 + 0.0) × sum_insured / target_LR × (1 + 0.0) = ATP.
        let ins = make_insurer(InsurerId(1), 0);
        let risk = small_risk();
        let expected = (0.239 * ASSET_VALUE as f64 / 0.70).round() as u64;
        let (_, event) = first_event(ins.on_lead_quote_requested(Day(0), SubmissionId(1), InsuredId(1), &risk));
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
        let ins = Insurer::new(InsurerId(1), 0, 0.239, 0.0, 0.70, 0.3, 0.0, 0.0, None, Some(ASSET_VALUE - 1), 0.0);
        let risk = cat_risk(); // sum_insured = ASSET_VALUE > max_line_size
        let (_, event) = first_event(ins.on_lead_quote_requested(Day(0), SubmissionId(1), InsuredId(1), &risk));
        assert!(
            matches!(event, Event::LeadQuoteDeclined { reason: DeclineReason::MaxLineSizeExceeded, .. }),
            "expected LeadQuoteDeclined(MaxLineSizeExceeded), got {event:?}"
        );
    }

    #[test]
    fn max_cat_aggregate_breached_emits_declined() {
        let ins = Insurer::new(InsurerId(1), 0, 0.239, 0.0, 0.70, 0.3, 0.0, 0.0, Some(0), None, 0.0);
        let risk = cat_risk(); // cat_aggregate(0) + sum_insured > max_cat_aggregate(0)
        let (_, event) = first_event(ins.on_lead_quote_requested(Day(0), SubmissionId(1), InsuredId(1), &risk));
        assert!(
            matches!(event, Event::LeadQuoteDeclined { reason: DeclineReason::MaxCatAggregateBreached, .. }),
            "expected LeadQuoteDeclined(MaxCatAggregateBreached), got {event:?}"
        );
    }

    #[test]
    fn within_limits_after_partial_fill_emits_quote_issued() {
        let mut ins = Insurer::new(InsurerId(1), 0, 0.239, 0.0, 0.70, 0.3, 0.0, 0.0, Some(2 * ASSET_VALUE), None, 0.0);
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
        let mut ins = make_insurer(InsurerId(1), ASSET_VALUE as i64 * 10);
        let atp_before = quote_atp(&ins);
        ins.on_policy_bound(PolicyId(1), ASSET_VALUE, 0, &[Peril::Attritional]);
        let _ = ins.on_claim_settled(Day(0), ASSET_VALUE, Peril::Attritional);
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
        let mut ins = make_insurer(InsurerId(1), ASSET_VALUE as i64 * 10);
        ins.on_policy_bound(PolicyId(1), ASSET_VALUE, 0, &[Peril::Attritional]);
        let _ = ins.on_claim_settled(Day(0), ASSET_VALUE / 2, Peril::Attritional);
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
        let mut ins = make_insurer(InsurerId(1), ASSET_VALUE as i64 * 10);
        ins.on_policy_bound(PolicyId(1), ASSET_VALUE, 0, &[Peril::Attritional]);
        let _ = ins.on_claim_settled(Day(0), ASSET_VALUE, Peril::Attritional);
        ins.on_year_end(); // ELF updated, counters reset
        let atp_year1 = quote_atp(&ins);
        ins.on_year_end(); // no new data → noop
        assert_eq!(quote_atp(&ins), atp_year1, "second on_year_end with no data must be a noop");
    }

    #[test]
    fn ewma_compounds_over_multiple_years() {
        // Two consecutive high-loss years should push ELF higher than one.
        let mut ins = make_insurer(InsurerId(1), ASSET_VALUE as i64 * 10);
        ins.on_policy_bound(PolicyId(1), ASSET_VALUE, 0, &[Peril::Attritional]);
        let _ = ins.on_claim_settled(Day(0), ASSET_VALUE, Peril::Attritional);
        ins.on_year_end();
        let atp_after_year1 = quote_atp(&ins);

        ins.on_policy_bound(PolicyId(2), ASSET_VALUE, 0, &[Peril::Attritional]);
        let _ = ins.on_claim_settled(Day(0), ASSET_VALUE, Peril::Attritional);
        ins.on_year_end();
        let atp_after_year2 = quote_atp(&ins);

        assert!(atp_after_year2 > atp_after_year1, "consecutive bad years must compound ELF upward");
    }

    #[test]
    fn on_policy_bound_credits_net_premium_to_capital() {
        // expense_ratio=0.25 → net = 75% of gross premium.
        let mut ins = Insurer::new(InsurerId(1), 1_000_000, 0.239, 0.0, 0.55, 0.3, 0.25, 0.0, None, None, 0.0);
        let gross_premium = 400_000u64;
        ins.on_policy_bound(PolicyId(1), ASSET_VALUE, gross_premium, &[Peril::Attritional]);
        let expected_net = (gross_premium as f64 * 0.75).round() as i64;
        assert_eq!(
            ins.capital,
            1_000_000 + expected_net,
            "capital must increase by net premium (gross × (1 − expense_ratio))"
        );
    }

    // ── Underwriter cycle adjustment ──────────────────────────────────────────

    /// Helper: get premium from a LeadQuoteIssued for a standard att-only risk.
    fn quote_premium(ins: &Insurer) -> u64 {
        let risk = Risk {
            sum_insured: ASSET_VALUE,
            territory: "US-SE".to_string(),
            perils_covered: vec![Peril::Attritional],
        };
        let events = ins.on_lead_quote_requested(Day(0), SubmissionId(1), InsuredId(1), &risk);
        let (_, event) = events.into_iter().next().unwrap();
        if let Event::LeadQuoteIssued { premium, .. } = event { premium } else { panic!("expected LeadQuoteIssued") }
    }

    #[test]
    fn cycle_adj_raises_premium_after_bad_year() {
        // Compare two identical insurers that differ only in cycle_sensitivity (0.0 vs 0.3)
        // after the same bad year. The sensitive insurer must quote higher.
        // Both see the same EWMA update so the difference is purely from cycle_adj.
        let gross_premium = 500_000u64;

        let mut sensitive = Insurer::new(InsurerId(1), ASSET_VALUE as i64 * 10, 0.239, 0.0, 0.70, 0.3, 0.0, 0.05, None, None, 0.3);
        let mut flat = Insurer::new(InsurerId(2), ASSET_VALUE as i64 * 10, 0.239, 0.0, 0.70, 0.3, 0.0, 0.05, None, None, 0.0);

        for ins in [&mut sensitive, &mut flat] {
            ins.on_policy_bound(PolicyId(1), ASSET_VALUE, gross_premium, &[Peril::Attritional]);
            let _ = ins.on_claim_settled(Day(0), gross_premium * 2, Peril::Attritional);
            ins.on_year_end(); // prior_year_LR = 2.0 > target 0.70
        }

        let p_sensitive = quote_premium(&sensitive);
        let p_flat = quote_premium(&flat);
        assert!(p_sensitive > p_flat, "insurer with cycle_sensitivity=0.3 must quote higher after bad year: {p_sensitive} vs {p_flat}");
    }

    #[test]
    fn cycle_adj_floors_at_profit_loading_after_good_year() {
        // A benign year (LR < target) must not push premium below ATP × (1 + profit_loading).
        let mut ins = Insurer::new(InsurerId(1), ASSET_VALUE as i64 * 10, 0.239, 0.0, 0.70, 0.3, 0.0, 0.05, None, None, 0.5);
        // Simulate a benign year: bind one policy, no claims → LR = 0.
        let gross_premium = 500_000u64;
        ins.on_policy_bound(PolicyId(1), ASSET_VALUE, gross_premium, &[Peril::Attritional]);
        // no claims
        ins.on_year_end(); // prior_year_LR = 0.0 < target 0.70

        let premium = quote_premium(&ins);
        let atp = quote_atp(&ins);
        let floor = (atp as f64 * 1.05).round() as u64;
        assert_eq!(premium, floor, "premium must equal ATP × (1 + profit_loading) after a benign year; got {premium} vs floor {floor}");
    }

    #[test]
    fn zero_cycle_sensitivity_is_unchanged_by_prior_year_lr() {
        // With cycle_sensitivity=0.0, prior_year_LR has no effect regardless of its value.
        let mut ins = Insurer::new(InsurerId(1), ASSET_VALUE as i64 * 10, 0.239, 0.0, 0.70, 0.3, 0.0, 0.05, None, None, 0.0);
        let baseline = quote_premium(&ins);

        // Simulate a terrible year.
        let gross_premium = 500_000u64;
        ins.on_policy_bound(PolicyId(1), ASSET_VALUE, gross_premium, &[Peril::Attritional]);
        let _ = ins.on_claim_settled(Day(0), gross_premium * 5, Peril::Attritional);
        ins.on_year_end();

        let after_bad_year = quote_premium(&ins);
        // ATP may change via EWMA, but the cycle loading must not add anything extra.
        // Verify premium == ATP × (1 + profit_loading) exactly.
        let atp = quote_atp(&ins);
        let expected = (atp as f64 * 1.05).round() as u64;
        assert_eq!(after_bad_year, expected, "zero cycle_sensitivity must leave premium = ATP × (1 + profit_loading) after any year; got {after_bad_year} vs {expected}");
        let _ = baseline; // used implicitly
    }
}
