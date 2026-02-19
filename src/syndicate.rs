use rand::Rng;

use crate::types::{SyndicateId, Year};

pub struct Syndicate {
    pub id: SyndicateId,
    pub capital: u64, // pence; placeholder — real capital management comes later
}

impl Syndicate {
    pub fn new(id: SyndicateId, initial_capital: u64) -> Self {
        Syndicate {
            id,
            capital: initial_capital,
        }
    }

    /// Called by the coordinator at year-end.
    /// Will update actuarial EWMA and internal pricing state.
    pub fn on_year_end(&mut self, _year: Year, _rng: &mut impl Rng) {
        // TODO: update EWMA loss estimates, apply parameter drift
    }
}
