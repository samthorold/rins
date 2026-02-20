use rand::Rng;

use crate::events::{Event, Peril, Risk};
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
            let mut risk = self.risk_catalogue[i % self.risk_catalogue.len()].clone();
            if !risk.perils_covered.contains(&Peril::Attritional) {
                risk.perils_covered.push(Peril::Attritional);
            }
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

#[cfg(test)]
mod tests {
    use rand::SeedableRng;
    use rand_chacha::ChaCha20Rng;

    use super::*;
    use crate::events::{Event, Peril, Risk};
    use crate::types::{BrokerId, Day};

    #[test]
    fn broker_submission_always_includes_attritional() {
        // Risk deliberately omits Attritional.
        let risk = Risk {
            line_of_business: "property".to_string(),
            sum_insured: 1_000_000,
            territory: "US-SE".to_string(),
            limit: 500_000,
            attachment: 0,
            perils_covered: vec![Peril::WindstormAtlantic],
        };
        let mut broker = Broker::new(BrokerId(1), 3, vec![risk]);
        let mut rng = ChaCha20Rng::seed_from_u64(0);
        let submissions = broker.generate_submissions(Day(0), &mut rng);
        for (_, event) in &submissions {
            if let Event::SubmissionArrived { risk, .. } = event {
                assert!(
                    risk.perils_covered.contains(&Peril::Attritional),
                    "every broker submission must include Peril::Attritional"
                );
            }
        }
    }
}
