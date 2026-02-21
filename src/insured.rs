use crate::events::{Peril, Risk};
use crate::types::{InsuredId, Year};

pub struct Insured {
    pub id: InsuredId,
    #[allow(dead_code)]
    pub name: String,
    pub assets: Vec<Risk>,
    /// Cumulative ground-up losses experienced, keyed by year.
    /// Populated by `on_insured_loss`; used for statistics and future decisions.
    pub total_ground_up_loss_by_year: std::collections::HashMap<Year, u64>,
}

impl Insured {
    /// Accumulate a ground-up loss for this insured.
    /// Called when an `InsuredLoss` event fires for any of this insured's policies.
    pub fn on_insured_loss(&mut self, ground_up_loss: u64, _peril: Peril, year: Year) {
        *self.total_ground_up_loss_by_year.entry(year).or_insert(0) += ground_up_loss;
    }
}
