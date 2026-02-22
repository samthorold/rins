use crate::events::{Event, Risk};
use crate::types::{Day, InsuredId, InsurerId, SubmissionId};

/// A single insurer in the minimal property market.
/// Writes 100% of each risk it quotes (lead-only, no follow market).
/// Capital is re-endowed at each YearStart — no insolvency in this model.
pub struct Insurer {
    pub id: InsurerId,
    /// Current capital (signed to allow negative without panicking).
    pub capital: i64,
    pub initial_capital: i64,
    /// Premium as a fraction of sum_insured (e.g. 0.02 = 2% rate on line).
    pub rate: f64,
}

impl Insurer {
    pub fn new(id: InsurerId, initial_capital: i64, rate: f64) -> Self {
        Insurer { id, capital: initial_capital, initial_capital, rate }
    }

    /// Reset capital to initial_capital at the start of each year.
    pub fn on_year_start(&mut self) {
        self.capital = self.initial_capital;
    }

    /// Price and issue a lead quote for a risk. Always quotes (no capacity checks).
    /// Premium = rate × sum_insured.
    pub fn on_lead_quote_requested(
        &self,
        day: Day,
        submission_id: SubmissionId,
        insured_id: InsuredId,
        risk: &Risk,
    ) -> (Day, Event) {
        let premium = (self.rate * risk.sum_insured as f64).round() as u64;
        (day, Event::LeadQuoteIssued { submission_id, insured_id, insurer_id: self.id, premium })
    }

    /// Deduct a settled claim from capital (can go negative — no insolvency logic yet).
    pub fn on_claim_settled(&mut self, amount: u64) {
        self.capital -= amount as i64;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::SMALL_ASSET_VALUE;
    use crate::events::Peril;

    fn small_risk() -> Risk {
        Risk {
            sum_insured: SMALL_ASSET_VALUE,
            territory: "US-SE".to_string(),
            perils_covered: vec![Peril::WindstormAtlantic, Peril::Attritional],
        }
    }

    #[test]
    fn on_year_start_resets_capital() {
        let mut ins = Insurer::new(InsurerId(1), 1_000_000, 0.02);
        ins.capital = 500_000; // depleted
        ins.on_year_start();
        assert_eq!(ins.capital, ins.initial_capital);
    }

    #[test]
    fn on_claim_settled_reduces_capital() {
        let mut ins = Insurer::new(InsurerId(1), 1_000_000, 0.02);
        ins.on_claim_settled(300_000);
        assert_eq!(ins.capital, 700_000);
    }

    #[test]
    fn on_claim_settled_can_go_negative() {
        let mut ins = Insurer::new(InsurerId(1), 100, 0.02);
        ins.on_claim_settled(1_000_000);
        assert!(ins.capital < 0, "capital should go negative without panicking");
    }

    #[test]
    fn on_lead_quote_requested_always_quotes() {
        let ins = Insurer::new(InsurerId(1), 1_000_000_000, 0.02);
        let risk = small_risk();
        let (_, event) = ins.on_lead_quote_requested(Day(0), SubmissionId(1), InsuredId(1), &risk);
        assert!(
            matches!(event, Event::LeadQuoteIssued { .. }),
            "insurer must always issue a lead quote, got {event:?}"
        );
    }

    #[test]
    fn premium_equals_rate_times_sum_insured() {
        let ins = Insurer::new(InsurerId(1), 1_000_000_000, 0.02);
        let risk = small_risk();
        let expected = (0.02 * SMALL_ASSET_VALUE as f64).round() as u64;
        let (_, event) = ins.on_lead_quote_requested(Day(0), SubmissionId(1), InsuredId(1), &risk);
        if let Event::LeadQuoteIssued { premium, .. } = event {
            assert_eq!(premium, expected, "premium must equal rate × sum_insured");
        }
    }

    #[test]
    fn lead_quote_issued_carries_insured_id() {
        let ins = Insurer::new(InsurerId(1), 1_000_000_000, 0.02);
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
        let ins = Insurer::new(InsurerId(1), 0, 0.02);
        let small = Risk {
            sum_insured: SMALL_ASSET_VALUE,
            territory: "US-SE".to_string(),
            perils_covered: vec![Peril::Attritional],
        };
        let large = Risk {
            sum_insured: SMALL_ASSET_VALUE * 10,
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
        let ins = Insurer::new(InsurerId(1), 1_000_000_000, 0.02);
        let risk = small_risk();
        let (_, event) = ins.on_lead_quote_requested(Day(0), SubmissionId(1), InsuredId(1), &risk);
        if let Event::LeadQuoteIssued { premium, .. } = event {
            assert!(premium > 0, "premium must be positive for a non-trivial risk");
        }
    }
}
