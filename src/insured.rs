use crate::config::{LARGE_ASSET_VALUE, SMALL_ASSET_VALUE};
use crate::events::{Event, Peril};
use crate::types::{Day, InsuredId, InsurerId, SubmissionId, Year};

/// Asset size tier for an insured.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AssetType {
    /// Small property: 50M USD asset value.
    Small,
    /// Large property: 1B USD asset value.
    Large,
}

pub struct Insured {
    pub id: InsuredId,
    pub asset_type: AssetType,
    /// Cumulative ground-up losses experienced, keyed by year.
    pub total_ground_up_loss_by_year: std::collections::HashMap<Year, u64>,
}

impl Insured {
    pub fn sum_insured(&self) -> u64 {
        match self.asset_type {
            AssetType::Small => SMALL_ASSET_VALUE,
            AssetType::Large => LARGE_ASSET_VALUE,
        }
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

    /// Accumulate a ground-up loss for this insured.
    pub fn on_insured_loss(&mut self, ground_up_loss: u64, _peril: Peril, year: Year) {
        *self.total_ground_up_loss_by_year.entry(year).or_insert(0) += ground_up_loss;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_insured(id: u64) -> Insured {
        Insured {
            id: InsuredId(id),
            asset_type: AssetType::Small,
            total_ground_up_loss_by_year: Default::default(),
        }
    }

    #[test]
    fn small_asset_sum_insured() {
        let insured = Insured {
            id: InsuredId(1),
            asset_type: AssetType::Small,
            total_ground_up_loss_by_year: Default::default(),
        };
        assert_eq!(insured.sum_insured(), SMALL_ASSET_VALUE);
    }

    #[test]
    fn large_asset_sum_insured() {
        let insured = Insured {
            id: InsuredId(1),
            asset_type: AssetType::Large,
            total_ground_up_loss_by_year: Default::default(),
        };
        assert_eq!(insured.sum_insured(), LARGE_ASSET_VALUE);
    }

    #[test]
    fn on_insured_loss_accumulates_by_year() {
        let mut insured = Insured {
            id: InsuredId(1),
            asset_type: AssetType::Small,
            total_ground_up_loss_by_year: Default::default(),
        };
        insured.on_insured_loss(100_000, Peril::Attritional, Year(1));
        insured.on_insured_loss(200_000, Peril::WindstormAtlantic, Year(1));
        insured.on_insured_loss(50_000, Peril::Attritional, Year(2));
        assert_eq!(insured.total_ground_up_loss_by_year[&Year(1)], 300_000);
        assert_eq!(insured.total_ground_up_loss_by_year[&Year(2)], 50_000);
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
