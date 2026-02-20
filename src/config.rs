use crate::events::{Peril, Risk};
use crate::types::{BrokerId, SyndicateId};

pub struct SyndicateConfig {
    pub id: SyndicateId,
    pub capital: u64,
    pub rate_on_line_bps: u32,
}

pub struct BrokerConfig {
    pub id: BrokerId,
    pub submissions_per_year: usize,
    pub risks: Vec<Risk>,
}

pub struct SimulationConfig {
    pub seed: u64,
    pub years: u32,
    pub syndicates: Vec<SyndicateConfig>,
    pub brokers: Vec<BrokerConfig>,
}

impl SimulationConfig {
    pub fn canonical() -> Self {
        let risk = Risk {
            line_of_business: "property".to_string(),
            sum_insured: 2_000_000,
            territory: "US-SE".to_string(),
            limit: 1_000_000,
            attachment: 100_000,
            perils_covered: vec![Peril::WindstormAtlantic, Peril::Flood],
        };

        SimulationConfig {
            seed: 42,
            years: 5,
            syndicates: vec![
                SyndicateConfig { id: SyndicateId(1), capital: 50_000_000, rate_on_line_bps: 500 },
                SyndicateConfig { id: SyndicateId(2), capital: 40_000_000, rate_on_line_bps: 600 },
                SyndicateConfig { id: SyndicateId(3), capital: 30_000_000, rate_on_line_bps: 450 },
            ],
            brokers: vec![
                BrokerConfig {
                    id: BrokerId(1),
                    submissions_per_year: 3,
                    risks: vec![risk.clone()],
                },
                BrokerConfig {
                    id: BrokerId(2),
                    submissions_per_year: 2,
                    risks: vec![risk],
                },
            ],
        }
    }
}
