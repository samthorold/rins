use crate::events::{Peril, Risk};
use crate::types::{BrokerId, InsuredId, SyndicateId};

pub struct SyndicateConfig {
    pub id: SyndicateId,
    pub capital: u64,
    pub rate_on_line_bps: u32,
}

pub struct InsuredConfig {
    pub id: InsuredId,
    pub name: String,
    pub assets: Vec<Risk>,
}

pub struct BrokerConfig {
    pub id: BrokerId,
    pub submissions_per_year: usize,
    pub insureds: Vec<InsuredConfig>,
}

pub struct SimulationConfig {
    pub seed: u64,
    pub years: u32,
    pub syndicates: Vec<SyndicateConfig>,
    pub brokers: Vec<BrokerConfig>,
}

impl SimulationConfig {
    pub fn canonical() -> Self {
        // ── Risk templates ────────────────────────────────────────────────────
        // All monetary values in pence. Scaled to ~real Lloyd's order-of-magnitude.

        let large_us_wind = Risk {
            line_of_business: "property".to_string(),
            sum_insured: 10_000_000_000,
            territory: "US-SE".to_string(),
            limit: 5_000_000_000,
            attachment: 500_000_000,
            perils_covered: vec![Peril::WindstormAtlantic],
        };

        let medium_us_wind = Risk {
            line_of_business: "property".to_string(),
            sum_insured: 2_500_000_000,  // £25M
            territory: "US-SE".to_string(),
            limit: 1_000_000_000,        // £10M
            attachment: 100_000_000,     // £1M
            perils_covered: vec![Peril::WindstormAtlantic],
        };

        let small_us_wind = Risk {
            line_of_business: "property".to_string(),
            sum_insured: 500_000_000,    // £5M
            territory: "US-SE".to_string(),
            limit: 200_000_000,          // £2M
            attachment: 20_000_000,      // £200K
            perils_covered: vec![Peril::WindstormAtlantic],
        };

        // ── Insured generator ─────────────────────────────────────────────────
        // Each insured holds a single risk asset. The broker's submissions_per_year
        // equals its insured count so every insured submits exactly once per year.
        // IDs: broker_number * 10_000 + sequential (1-based).
        let make_insureds = |broker_num: u32, risks: &[Risk], prefix: &str, count: usize| -> Vec<InsuredConfig> {
            (0..count)
                .map(|i| InsuredConfig {
                    id: InsuredId((broker_num * 10_000 + i as u32 + 1) as u64),
                    name: format!("{} {}", prefix, i + 1),
                    assets: vec![risks[i % risks.len()].clone()],
                })
                .collect()
        };

        SimulationConfig {
            seed: 42,
            years: 10,
            // ── Syndicates: 3 size tiers ──────────────────────────────────────
            // Capital in pence. Large: ~£500M, Medium: ~£200M, Small: ~£80M.
            syndicates: vec![
                // Large (3)
                SyndicateConfig { id: SyndicateId(1),  capital: 50_000_000_000, rate_on_line_bps: 450 },
                SyndicateConfig { id: SyndicateId(2),  capital: 50_000_000_000, rate_on_line_bps: 500 },
                SyndicateConfig { id: SyndicateId(3),  capital: 50_000_000_000, rate_on_line_bps: 550 },
                // Medium (6)
                SyndicateConfig { id: SyndicateId(4),  capital: 20_000_000_000, rate_on_line_bps: 500 },
                SyndicateConfig { id: SyndicateId(5),  capital: 20_000_000_000, rate_on_line_bps: 550 },
                SyndicateConfig { id: SyndicateId(6),  capital: 20_000_000_000, rate_on_line_bps: 600 },
                SyndicateConfig { id: SyndicateId(7),  capital: 20_000_000_000, rate_on_line_bps: 620 },
                SyndicateConfig { id: SyndicateId(8),  capital: 20_000_000_000, rate_on_line_bps: 580 },
                SyndicateConfig { id: SyndicateId(9),  capital: 20_000_000_000, rate_on_line_bps: 650 },
                // Small (6)
                SyndicateConfig { id: SyndicateId(10), capital: 8_000_000_000, rate_on_line_bps: 550 },
                SyndicateConfig { id: SyndicateId(11), capital: 8_000_000_000, rate_on_line_bps: 600 },
                SyndicateConfig { id: SyndicateId(12), capital: 8_000_000_000, rate_on_line_bps: 625 },
                SyndicateConfig { id: SyndicateId(13), capital: 8_000_000_000, rate_on_line_bps: 650 },
                SyndicateConfig { id: SyndicateId(14), capital: 8_000_000_000, rate_on_line_bps: 675 },
                SyndicateConfig { id: SyndicateId(15), capital: 8_000_000_000, rate_on_line_bps: 700 },
            ],
            // ── Brokers: 6 with different US wind specialisms ─────────────────
            // 800 insureds total (one submission per insured per year).
            // Each broker's risk mix reflects its specialism; insureds cycle
            // through the mix so the portfolio is evenly distributed.
            brokers: vec![
                // Broker 1: Large property specialist — 200 insureds
                BrokerConfig {
                    id: BrokerId(1),
                    submissions_per_year: 200,
                    insureds: make_insureds(1, &[
                        large_us_wind.clone(),
                        large_us_wind.clone(),
                        medium_us_wind.clone(),
                    ], "Large US Wind Client", 200),
                },
                // Broker 2: Mid-market specialist — 150 insureds
                BrokerConfig {
                    id: BrokerId(2),
                    submissions_per_year: 150,
                    insureds: make_insureds(2, &[
                        medium_us_wind.clone(),
                        medium_us_wind.clone(),
                        small_us_wind.clone(),
                    ], "Mid-Market US Wind Client", 150),
                },
                // Broker 3: Diversified US wind — 120 insureds
                BrokerConfig {
                    id: BrokerId(3),
                    submissions_per_year: 120,
                    insureds: make_insureds(3, &[
                        large_us_wind.clone(),
                        medium_us_wind.clone(),
                        small_us_wind.clone(),
                    ], "Diversified US Wind Client", 120),
                },
                // Broker 4: Small commercial — 100 insureds
                BrokerConfig {
                    id: BrokerId(4),
                    submissions_per_year: 100,
                    insureds: make_insureds(4, &[
                        medium_us_wind.clone(),
                        small_us_wind.clone(),
                        small_us_wind.clone(),
                    ], "Small US Wind Client", 100),
                },
                // Broker 5: High-value specialist — 80 insureds
                BrokerConfig {
                    id: BrokerId(5),
                    submissions_per_year: 80,
                    insureds: make_insureds(5, &[
                        large_us_wind.clone(),
                        large_us_wind.clone(),
                        medium_us_wind.clone(),
                    ], "High-Value US Wind Client", 80),
                },
                // Broker 6: General market — 150 insureds
                BrokerConfig {
                    id: BrokerId(6),
                    submissions_per_year: 150,
                    insureds: make_insureds(6, &[
                        large_us_wind,
                        medium_us_wind.clone(),
                        medium_us_wind,
                        small_us_wind,
                    ], "General US Wind Client", 150),
                },
            ],
        }
    }
}
