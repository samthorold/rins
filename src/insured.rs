use crate::config::{LARGE_ASSET_VALUE, SMALL_ASSET_VALUE};
use crate::events::Peril;
use crate::types::{InsuredId, Year};

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

    /// Accumulate a ground-up loss for this insured.
    pub fn on_insured_loss(&mut self, ground_up_loss: u64, _peril: Peril, year: Year) {
        *self.total_ground_up_loss_by_year.entry(year).or_insert(0) += ground_up_loss;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

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
}
