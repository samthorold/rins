use crate::config::ASSET_VALUE;
use crate::events::{Event, Peril, Risk};
use crate::types::{Day, InsuredId, InsurerId, SubmissionId};

pub struct Insured {
    pub id: InsuredId,
    /// The asset this insured holds and seeks coverage for.
    pub risk: Risk,
}

impl Insured {
    pub fn new(id: InsuredId, territory: String, perils_covered: Vec<Peril>) -> Self {
        Self {
            id,
            risk: Risk { sum_insured: ASSET_VALUE, territory, perils_covered },
        }
    }

    pub fn sum_insured(&self) -> u64 {
        self.risk.sum_insured
    }

    /// The insured receives a quote and accepts it unconditionally.
    /// Future hook for price sensitivity: inspect `premium` before deciding.
    pub fn on_quote_presented(
        &self,
        day: Day,
        submission_id: SubmissionId,
        insurer_id: InsurerId,
        premium: u64,
    ) -> Vec<(Day, Event)> {
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

#[cfg(test)]
mod tests {
    use super::*;

    fn make_insured(id: u64) -> Insured {
        Insured::new(
            InsuredId(id),
            "US-SE".to_string(),
            vec![Peril::WindstormAtlantic, Peril::Attritional],
        )
    }

    #[test]
    fn asset_sum_insured() {
        let insured = Insured::new(InsuredId(1), "US-SE".to_string(), vec![Peril::WindstormAtlantic]);
        assert_eq!(insured.sum_insured(), ASSET_VALUE);
    }

    // ── on_quote_presented ────────────────────────────────────────────────────

    #[test]
    fn on_quote_presented_always_accepts() {
        let insured = make_insured(1);
        let events =
            insured.on_quote_presented(Day(3), SubmissionId(10), InsurerId(2), 50_000);
        assert_eq!(events.len(), 1);
        assert!(
            matches!(events[0].1, Event::QuoteAccepted { .. }),
            "insured must always return QuoteAccepted, got {:?}",
            events[0].1
        );
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
