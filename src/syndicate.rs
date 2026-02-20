use rand::Rng;

use crate::events::{Event, Risk};
use crate::types::{Day, SubmissionId, SyndicateId, Year};

pub struct Syndicate {
    pub id: SyndicateId,
    pub capital: u64, // pence; placeholder — real capital management comes later
    pub rate_on_line_bps: u32, // basis points, e.g. 500 = 5% rate on line
}

impl Syndicate {
    pub fn new(id: SyndicateId, initial_capital: u64, rate_on_line_bps: u32) -> Self {
        Syndicate {
            id,
            capital: initial_capital,
            rate_on_line_bps,
        }
    }

    /// Price and issue (or decline) a quote for a submission.
    /// MVP: premium = rate_on_line_bps * limit / 10_000.
    /// TODO: actuarial channel (§1), underwriter channel (§2), capital constraint override.
    pub fn on_quote_requested(
        &self,
        day: Day,
        submission_id: SubmissionId,
        risk: &Risk,
        is_lead: bool,
        lead_premium: Option<u64>,
    ) -> (Day, Event) {
        let premium = self.rate_on_line_bps as u64 * risk.limit / 10_000;
        // is_lead and lead_premium are inputs to the underwriter channel — deferred.
        let _ = (is_lead, lead_premium);
        (
            day,
            Event::QuoteIssued {
                submission_id,
                syndicate_id: self.id,
                premium,
                is_lead,
            },
        )
    }

    /// Deduct a settled claim from capital.
    /// TODO: check solvency floor and emit SyndicateInsolvency.
    pub fn on_claim_settled(&mut self, amount: u64) {
        self.capital = self.capital.saturating_sub(amount);
    }

    /// Called by the coordinator at year-end.
    /// Will update actuarial EWMA and internal pricing state.
    pub fn on_year_end(&mut self, _year: Year, _rng: &mut impl Rng) {
        // TODO: update EWMA loss estimates, apply parameter drift
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::events::Peril;
    use crate::types::SubmissionId;

    fn make_risk(limit: u64) -> Risk {
        Risk {
            line_of_business: "property".to_string(),
            sum_insured: limit * 2,
            territory: "US-SE".to_string(),
            limit,
            attachment: 0,
            perils_covered: vec![Peril::WindstormAtlantic],
        }
    }

    #[test]
    fn capital_depletes_by_exact_claim_amount() {
        let mut s = Syndicate::new(SyndicateId(1), 10_000_000, 500);
        s.on_claim_settled(300_000);
        assert_eq!(s.capital, 9_700_000);
    }

    #[test]
    fn capital_saturates_at_zero() {
        let mut s = Syndicate::new(SyndicateId(1), 100_000, 500);
        s.on_claim_settled(500_000);
        assert_eq!(s.capital, 0);
    }

    #[test]
    fn syndicate_quotes_rate_on_line() {
        let s = Syndicate::new(SyndicateId(1), 10_000_000, 500);
        let risk = make_risk(1_000_000);
        let (_, event) =
            s.on_quote_requested(crate::types::Day(0), SubmissionId(1), &risk, true, None);
        match event {
            Event::QuoteIssued { premium, .. } => {
                assert_eq!(premium, 500 * 1_000_000 / 10_000); // 50_000
            }
            _ => panic!("expected QuoteIssued"),
        }
    }
}
