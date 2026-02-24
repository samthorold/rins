use crate::config::ASSET_VALUE;
use crate::events::{Event, Peril, Risk};
use crate::types::{Day, InsuredId, InsurerId, SubmissionId};

pub struct Insured {
    pub id: InsuredId,
    /// The asset this insured holds and seeks coverage for.
    pub risk: Risk,
    /// Maximum rate on line (premium / sum_insured) the insured will accept.
    /// Quotes above this threshold trigger `QuoteRejected` and the insured retries at renewal.
    max_rate_on_line: f64,
}

impl Insured {
    pub fn new(id: InsuredId, territory: String, perils_covered: Vec<Peril>, max_rate_on_line: f64) -> Self {
        Self {
            id,
            risk: Risk { sum_insured: ASSET_VALUE, territory, perils_covered },
            max_rate_on_line,
        }
    }

    pub fn sum_insured(&self) -> u64 {
        self.risk.sum_insured
    }

    /// The insured decides whether to accept the quote based on its reservation price.
    /// Emits `QuoteRejected` if `premium / sum_insured > max_rate_on_line`; `QuoteAccepted` otherwise.
    pub fn on_quote_presented(
        &self,
        day: Day,
        submission_id: SubmissionId,
        insurer_id: InsurerId,
        premium: u64,
    ) -> Vec<(Day, Event)> {
        let rate = premium as f64 / self.risk.sum_insured as f64;
        if rate > self.max_rate_on_line {
            vec![(day, Event::QuoteRejected { submission_id, insured_id: self.id })]
        } else {
            vec![(
                day,
                Event::QuoteAccepted {
                    submission_id,
                    insured_id: self.id,
                    insurer_id,
                    premium,
                },
            )]
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_insured(id: u64) -> Insured {
        Insured::new(
            InsuredId(id),
            "US-SE".to_string(),
            vec![Peril::WindstormAtlantic, Peril::Attritional],
            1.0, // accepts all quotes
        )
    }

    #[test]
    fn asset_sum_insured() {
        let insured = Insured::new(InsuredId(1), "US-SE".to_string(), vec![Peril::WindstormAtlantic], 1.0);
        assert_eq!(insured.sum_insured(), ASSET_VALUE);
    }

    // ── on_quote_presented ────────────────────────────────────────────────────

    #[test]
    fn on_quote_presented_accepts_below_threshold() {
        // max_rate_on_line=0.10; premium at 8% RoL → accepts.
        let insured = Insured::new(
            InsuredId(1), "US-SE".to_string(),
            vec![Peril::WindstormAtlantic, Peril::Attritional], 0.10,
        );
        let premium = (ASSET_VALUE as f64 * 0.08) as u64; // 8% RoL < 10%
        let events = insured.on_quote_presented(Day(3), SubmissionId(1), InsurerId(1), premium);
        assert_eq!(events.len(), 1);
        assert!(
            matches!(events[0].1, Event::QuoteAccepted { .. }),
            "quote at 8% RoL must be accepted when threshold is 10%, got {:?}", events[0].1
        );
    }

    #[test]
    fn on_quote_presented_accepts_at_threshold() {
        // max_rate_on_line=0.10; premium exactly at 10% RoL → accepts (≤ threshold).
        let insured = Insured::new(
            InsuredId(1), "US-SE".to_string(),
            vec![Peril::WindstormAtlantic, Peril::Attritional], 0.10,
        );
        let premium = (ASSET_VALUE as f64 * 0.10) as u64;
        let events = insured.on_quote_presented(Day(3), SubmissionId(1), InsurerId(1), premium);
        assert!(matches!(events[0].1, Event::QuoteAccepted { .. }), "at-threshold quote must be accepted");
    }

    #[test]
    fn on_quote_presented_rejects_above_threshold() {
        // max_rate_on_line=0.05; premium at 6% RoL → rejects.
        let insured = Insured::new(
            InsuredId(1), "US-SE".to_string(),
            vec![Peril::WindstormAtlantic, Peril::Attritional], 0.05,
        );
        let premium = (ASSET_VALUE as f64 * 0.06) as u64; // 6% RoL > 5%
        let events = insured.on_quote_presented(Day(3), SubmissionId(10), InsurerId(2), premium);
        assert_eq!(events.len(), 1);
        assert!(
            matches!(events[0].1, Event::QuoteRejected { .. }),
            "quote at 6% RoL must be rejected when threshold is 5%, got {:?}", events[0].1
        );
    }

    #[test]
    fn on_quote_rejected_carries_correct_ids() {
        let insured = Insured::new(
            InsuredId(42), "US-SE".to_string(),
            vec![Peril::WindstormAtlantic, Peril::Attritional], 0.01,
        );
        let premium = ASSET_VALUE; // 100% RoL — always rejected
        let events = insured.on_quote_presented(Day(5), SubmissionId(99), InsurerId(3), premium);
        if let Event::QuoteRejected { submission_id, insured_id } = events[0].1 {
            assert_eq!(submission_id, SubmissionId(99));
            assert_eq!(insured_id, InsuredId(42));
        } else {
            panic!("expected QuoteRejected");
        }
    }

    #[test]
    fn on_quote_presented_accepted_same_day() {
        let insured = make_insured(1);
        let day = Day(7);
        let events = insured.on_quote_presented(day, SubmissionId(1), InsurerId(1), 1_000);
        assert_eq!(events[0].0, day, "QuoteAccepted must fire on the same day as QuotePresented");
    }

    #[test]
    fn on_quote_presented_carries_correct_fields() {
        let insured = make_insured(42);
        let events =
            insured.on_quote_presented(Day(5), SubmissionId(99), InsurerId(3), 75_000);
        if let Event::QuoteAccepted { submission_id, insured_id, insurer_id, premium } =
            events[0].1
        {
            assert_eq!(submission_id, SubmissionId(99));
            assert_eq!(insured_id, InsuredId(42));
            assert_eq!(insurer_id, InsurerId(3));
            assert_eq!(premium, 75_000);
        } else {
            panic!("expected QuoteAccepted");
        }
    }
}
