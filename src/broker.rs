use crate::types::{BrokerId, Year};

pub struct Broker {
    pub id: BrokerId,
    // Will hold: HashMap<(SyndicateId, LineOfBusiness), f64> for relationship scores
}

impl Broker {
    pub fn new(id: BrokerId) -> Self {
        Broker { id }
    }

    /// Called by the coordinator at year-end.
    /// Will apply exponential relationship decay across all syndicate pairs.
    pub fn on_year_end(&mut self, _year: Year) {
        // TODO: decay relationship scores
    }
}
