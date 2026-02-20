use rand::Rng;

use crate::events::{Event, Risk};
use crate::types::{BrokerId, Day, SubmissionId, Year};

pub struct Broker {
    pub id: BrokerId,
    pub submissions_per_year: usize,
    pub risk_catalogue: Vec<Risk>,
    next_submission_seq: u64,
    // Will hold: HashMap<(SyndicateId, LineOfBusiness), f64> for relationship scores
}

impl Broker {
    pub fn new(id: BrokerId, submissions_per_year: usize, risk_catalogue: Vec<Risk>) -> Self {
        Broker {
            id,
            submissions_per_year,
            risk_catalogue,
            next_submission_seq: 0,
        }
    }

    /// Generate submissions for the year, spread uniformly across the first 30 days.
    /// The `rng` parameter is accepted now but not used; stochastic timing is deferred.
    pub fn generate_submissions(&mut self, day: Day, _rng: &mut impl Rng) -> Vec<(Day, Event)> {
        if self.risk_catalogue.is_empty() {
            return vec![];
        }
        let n = self.submissions_per_year;
        let mut events = Vec::with_capacity(n);
        for i in 0..n {
            let offset = if n > 1 { i as u64 * 30 / n as u64 } else { 0 };
            let submission_id = SubmissionId(self.id.0 * 1_000_000 + self.next_submission_seq);
            self.next_submission_seq += 1;
            let risk = self.risk_catalogue[i % self.risk_catalogue.len()].clone();
            events.push((
                day.offset(offset),
                Event::SubmissionArrived {
                    submission_id,
                    broker_id: self.id,
                    risk,
                },
            ));
        }
        events
    }

    /// Called by the coordinator at year-end.
    /// Will apply exponential relationship decay across all syndicate pairs.
    pub fn on_year_end(&mut self, _year: Year) {
        // TODO: decay relationship scores
    }
}
