use crate::config::{AttritionalConfig, CatConfig};
use crate::events::{Event, Peril, Risk};
use crate::types::{Day, InsurerId, SubmissionId};

/// A single insurer in the minimal property market.
/// Writes 100% of each risk it quotes (lead-only, no follow market).
/// Capital is re-endowed at each YearStart — no insolvency in this model.
pub struct Insurer {
    pub id: InsurerId,
    /// Current capital (signed to allow negative without panicking).
    pub capital: i64,
    pub initial_capital: i64,
    pub target_loss_ratio: f64,
}

impl Insurer {
    pub fn new(id: InsurerId, initial_capital: i64, target_loss_ratio: f64) -> Self {
        Insurer { id, capital: initial_capital, initial_capital, target_loss_ratio }
    }

    /// Reset capital to initial_capital at the start of each year.
    pub fn on_year_start(&mut self) {
        self.capital = self.initial_capital;
    }

    /// Price and issue a quote for a risk. Always quotes (no capacity checks).
    /// Premium = E[annual loss] / target_loss_ratio.
    pub fn on_quote_requested(
        &self,
        day: Day,
        submission_id: SubmissionId,
        risk: &Risk,
        att: &AttritionalConfig,
        cat: &CatConfig,
    ) -> (Day, Event) {
        let expected_loss = self.expected_annual_loss(risk, att, cat);
        let premium = (expected_loss as f64 / self.target_loss_ratio).round() as u64;
        (day, Event::QuoteIssued { submission_id, insurer_id: self.id, premium })
    }

    /// Compute E[annual ground-up loss] from peril parameters.
    fn expected_annual_loss(
        &self,
        risk: &Risk,
        att: &AttritionalConfig,
        cat: &CatConfig,
    ) -> u64 {
        let si = risk.sum_insured as f64;
        let mut expected = 0.0_f64;

        for peril in &risk.perils_covered {
            match peril {
                Peril::Attritional => {
                    // E[df] = exp(mu + sigma²/2) for LogNormal
                    let e_df = (att.mu + att.sigma * att.sigma / 2.0).exp();
                    expected += att.annual_rate * e_df * si;
                }
                Peril::WindstormAtlantic => {
                    // E[df] = scale × shape / (shape − 1) for Pareto (shape > 1)
                    let e_df = if cat.pareto_shape > 1.0 {
                        (cat.pareto_scale * cat.pareto_shape / (cat.pareto_shape - 1.0)).min(1.0)
                    } else {
                        cat.pareto_scale
                    };
                    expected += cat.annual_frequency * e_df * si;
                }
            }
        }

        expected.round() as u64
    }

    /// Deduct a settled claim from capital (can go negative — no insolvency logic yet).
    pub fn on_claim_settled(&mut self, amount: u64) {
        self.capital -= amount as i64;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::{AttritionalConfig, CatConfig, SMALL_ASSET_VALUE};

    fn att() -> AttritionalConfig {
        AttritionalConfig { annual_rate: 2.0, mu: -3.0, sigma: 1.0 }
    }

    fn cat() -> CatConfig {
        CatConfig { annual_frequency: 0.5, pareto_scale: 0.05, pareto_shape: 1.5 }
    }

    fn small_risk() -> Risk {
        Risk {
            sum_insured: SMALL_ASSET_VALUE,
            territory: "US-SE".to_string(),
            perils_covered: vec![Peril::WindstormAtlantic, Peril::Attritional],
        }
    }

    #[test]
    fn on_year_start_resets_capital() {
        let mut ins = Insurer::new(InsurerId(1), 1_000_000, 0.65);
        ins.capital = 500_000; // depleted
        ins.on_year_start();
        assert_eq!(ins.capital, ins.initial_capital);
    }

    #[test]
    fn on_claim_settled_reduces_capital() {
        let mut ins = Insurer::new(InsurerId(1), 1_000_000, 0.65);
        ins.on_claim_settled(300_000);
        assert_eq!(ins.capital, 700_000);
    }

    #[test]
    fn on_claim_settled_can_go_negative() {
        let mut ins = Insurer::new(InsurerId(1), 100, 0.65);
        ins.on_claim_settled(1_000_000);
        assert!(ins.capital < 0, "capital should go negative without panicking");
    }

    #[test]
    fn on_quote_requested_always_quotes() {
        let ins = Insurer::new(InsurerId(1), 1_000_000_000, 0.65);
        let risk = small_risk();
        let (_, event) =
            ins.on_quote_requested(Day(0), SubmissionId(1), &risk, &att(), &cat());
        assert!(
            matches!(event, Event::QuoteIssued { .. }),
            "insurer must always issue a quote, got {event:?}"
        );
    }

    #[test]
    fn premium_equals_expected_loss_divided_by_target_lr() {
        let ins = Insurer::new(InsurerId(1), 1_000_000_000, 0.65);
        let risk = small_risk();
        let expected_loss = ins.expected_annual_loss(&risk, &att(), &cat());
        let expected_premium = (expected_loss as f64 / 0.65).round() as u64;
        let (_, event) =
            ins.on_quote_requested(Day(0), SubmissionId(1), &risk, &att(), &cat());
        if let Event::QuoteIssued { premium, .. } = event {
            assert_eq!(premium, expected_premium, "premium != E[loss] / target_lr");
        }
    }

    #[test]
    fn expected_loss_increases_with_sum_insured() {
        let ins = Insurer::new(InsurerId(1), 0, 0.65);
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
        let e_small = ins.expected_annual_loss(&small, &att(), &cat());
        let e_large = ins.expected_annual_loss(&large, &att(), &cat());
        assert!(
            e_large > e_small,
            "larger sum_insured must produce larger expected loss: {e_large} vs {e_small}"
        );
    }

    #[test]
    fn quote_premium_is_positive_for_nonzero_risk() {
        let ins = Insurer::new(InsurerId(1), 1_000_000_000, 0.65);
        let risk = small_risk();
        let (_, event) =
            ins.on_quote_requested(Day(0), SubmissionId(1), &risk, &att(), &cat());
        if let Event::QuoteIssued { premium, .. } = event {
            assert!(premium > 0, "premium must be positive for a non-trivial risk");
        }
    }
}
