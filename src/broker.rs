use rand::Rng;

use crate::events::{Event, Peril};
use crate::insured::Insured;
use crate::types::{BrokerId, Day, InsuredId, SubmissionId, Year};

pub struct Broker {
    pub id: BrokerId,
    pub submissions_per_year: usize,
    pub insureds: Vec<Insured>,
    next_submission_seq: u64,
    // Will hold: HashMap<(SyndicateId, LineOfBusiness), f64> for relationship scores
}

impl Broker {
    pub fn new(id: BrokerId, submissions_per_year: usize, insureds: Vec<Insured>) -> Self {
        Broker {
            id,
            submissions_per_year,
            insureds,
            next_submission_seq: 0,
        }
    }

    /// Generate submissions for the year, spread uniformly across the first 180 days.
    /// The `rng` parameter is accepted now but not used; stochastic timing is deferred.
    pub fn generate_submissions(&mut self, day: Day, _rng: &mut impl Rng) -> Vec<(Day, Event)> {
        // Build a flat list of (insured_id, risk) pairs from all insured assets.
        let catalogue: Vec<(InsuredId, crate::events::Risk)> = self
            .insureds
            .iter()
            .flat_map(|ins| ins.assets.iter().map(move |r| (ins.id, r.clone())))
            .collect();

        if catalogue.is_empty() {
            return vec![];
        }
        let n = self.submissions_per_year;
        let mut events = Vec::with_capacity(n);
        for i in 0..n {
            let offset = if n > 1 { i as u64 * 180 / n as u64 } else { 0 };
            let submission_id = SubmissionId(self.id.0 * 1_000_000 + self.next_submission_seq);
            self.next_submission_seq += 1;
            let (insured_id, mut risk) = catalogue[i % catalogue.len()].clone();
            if !risk.perils_covered.contains(&Peril::Attritional) {
                risk.perils_covered.push(Peril::Attritional);
            }
            events.push((
                day.offset(offset),
                Event::SubmissionArrived {
                    submission_id,
                    broker_id: self.id,
                    insured_id,
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
    use crate::insured::Insured;
    use crate::types::{BrokerId, Day, InsuredId};

    fn make_insured(id: u64, risk: Risk) -> Insured {
        Insured {
            id: InsuredId(id),
            name: format!("Insured {id}"),
            assets: vec![risk],
            total_ground_up_loss_by_year: std::collections::HashMap::new(),
        }
    }

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
        let insured = make_insured(1, risk);
        let mut broker = Broker::new(BrokerId(1), 3, vec![insured]);
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

    #[test]
    fn submissions_carry_insured_id() {
        let risk_a = Risk {
            line_of_business: "property".to_string(),
            sum_insured: 1_000_000,
            territory: "US-SE".to_string(),
            limit: 500_000,
            attachment: 0,
            perils_covered: vec![Peril::WindstormAtlantic],
        };
        let risk_b = Risk {
            line_of_business: "property".to_string(),
            sum_insured: 2_000_000,
            territory: "EU".to_string(),
            limit: 1_000_000,
            attachment: 0,
            perils_covered: vec![Peril::WindstormEuropean],
        };
        // Two insureds with distinct ids.
        let insured_a = make_insured(10, risk_a);
        let insured_b = make_insured(20, risk_b);
        let mut broker = Broker::new(BrokerId(1), 4, vec![insured_a, insured_b]);
        let mut rng = ChaCha20Rng::seed_from_u64(0);
        let submissions = broker.generate_submissions(Day(0), &mut rng);

        // Flat catalogue is [InsuredId(10) × asset, InsuredId(20) × asset].
        // Round-robin: slot 0 → 10, slot 1 → 20, slot 2 → 10, slot 3 → 20.
        let expected = [InsuredId(10), InsuredId(20), InsuredId(10), InsuredId(20)];
        for (i, (_, event)) in submissions.iter().enumerate() {
            if let Event::SubmissionArrived { insured_id, .. } = event {
                assert_eq!(
                    *insured_id, expected[i],
                    "submission {i}: expected insured_id {:?}, got {:?}",
                    expected[i], insured_id
                );
            }
        }
    }
}
