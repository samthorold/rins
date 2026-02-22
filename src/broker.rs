use rand::Rng;

use crate::events::{Event, Peril, Risk};
use crate::insured::{AssetType, Insured};
use crate::types::{Day, InsuredId, SubmissionId};

/// Single broker that services all insureds.
/// Submits one risk per insured per year; market handles insurer selection.
pub struct Broker {
    pub insureds: Vec<Insured>,
    next_submission_seq: u64,
}

impl Broker {
    pub fn new(insureds: Vec<Insured>) -> Self {
        Broker { insureds, next_submission_seq: 0 }
    }

    /// Generate one SubmissionArrived per insured, spread uniformly across the first 180 days.
    pub fn generate_submissions(
        &mut self,
        year_start: Day,
        _rng: &mut impl Rng,
    ) -> Vec<(Day, Event)> {
        let n = self.insureds.len();
        let mut events = Vec::with_capacity(n);
        for (i, insured) in self.insureds.iter().enumerate() {
            let offset = if n > 1 { i as u64 * 180 / n as u64 } else { 0 };
            let submission_id = SubmissionId(self.next_submission_seq);
            self.next_submission_seq += 1;
            let risk = Risk {
                sum_insured: insured.sum_insured(),
                territory: "US-SE".to_string(),
                perils_covered: vec![Peril::WindstormAtlantic, Peril::Attritional],
            };
            events.push((
                year_start.offset(offset),
                Event::SubmissionArrived { submission_id, insured_id: insured.id, risk },
            ));
        }
        events
    }
}

#[cfg(test)]
mod tests {
    use rand::SeedableRng;
    use rand_chacha::ChaCha20Rng;

    use super::*;
    use crate::config::{LARGE_ASSET_VALUE, SMALL_ASSET_VALUE};

    fn make_insured(id: u64, asset_type: AssetType) -> Insured {
        Insured {
            id: InsuredId(id),
            asset_type,
            total_ground_up_loss_by_year: Default::default(),
        }
    }

    fn rng() -> ChaCha20Rng {
        ChaCha20Rng::seed_from_u64(42)
    }

    #[test]
    fn one_submission_per_insured() {
        let insureds = vec![
            make_insured(1, AssetType::Small),
            make_insured(2, AssetType::Large),
            make_insured(3, AssetType::Small),
        ];
        let n = insureds.len();
        let mut broker = Broker::new(insureds);
        let submissions = broker.generate_submissions(Day(0), &mut rng());
        assert_eq!(submissions.len(), n, "must generate exactly one submission per insured");
    }

    #[test]
    fn submissions_carry_correct_insured_ids() {
        let insureds = vec![
            make_insured(10, AssetType::Small),
            make_insured(20, AssetType::Large),
        ];
        let mut broker = Broker::new(insureds);
        let submissions = broker.generate_submissions(Day(0), &mut rng());

        let ids: Vec<u64> = submissions
            .iter()
            .filter_map(|(_, e)| match e {
                Event::SubmissionArrived { insured_id, .. } => Some(insured_id.0),
                _ => None,
            })
            .collect();
        assert_eq!(ids, vec![10, 20]);
    }

    #[test]
    fn submissions_include_both_perils() {
        let mut broker = Broker::new(vec![make_insured(1, AssetType::Small)]);
        let submissions = broker.generate_submissions(Day(0), &mut rng());
        for (_, e) in &submissions {
            if let Event::SubmissionArrived { risk, .. } = e {
                assert!(risk.perils_covered.contains(&Peril::WindstormAtlantic));
                assert!(risk.perils_covered.contains(&Peril::Attritional));
            }
        }
    }

    #[test]
    fn small_insured_sum_insured_is_correct() {
        let mut broker = Broker::new(vec![make_insured(1, AssetType::Small)]);
        let submissions = broker.generate_submissions(Day(0), &mut rng());
        for (_, e) in &submissions {
            if let Event::SubmissionArrived { risk, .. } = e {
                assert_eq!(risk.sum_insured, SMALL_ASSET_VALUE);
            }
        }
    }

    #[test]
    fn large_insured_sum_insured_is_correct() {
        let mut broker = Broker::new(vec![make_insured(1, AssetType::Large)]);
        let submissions = broker.generate_submissions(Day(0), &mut rng());
        for (_, e) in &submissions {
            if let Event::SubmissionArrived { risk, .. } = e {
                assert_eq!(risk.sum_insured, LARGE_ASSET_VALUE);
            }
        }
    }

    #[test]
    fn submissions_spread_across_first_180_days() {
        let insureds: Vec<Insured> = (1..=10)
            .map(|i| make_insured(i, AssetType::Small))
            .collect();
        let mut broker = Broker::new(insureds);
        let submissions = broker.generate_submissions(Day(0), &mut rng());
        for (day, _) in &submissions {
            assert!(
                day.0 < 180,
                "submission scheduled at day {}, expected < 180",
                day.0
            );
        }
    }
}
