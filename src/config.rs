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

        let medium_us_flood = Risk {
            line_of_business: "property".to_string(),
            sum_insured: 2_000_000_000,
            territory: "US-SE".to_string(),
            limit: 1_000_000_000,
            attachment: 100_000_000,
            perils_covered: vec![Peril::Flood],
        };

        let eu_property = Risk {
            line_of_business: "property".to_string(),
            sum_insured: 1_000_000_000,
            territory: "EU".to_string(),
            limit: 500_000_000,
            attachment: 50_000_000,
            perils_covered: vec![Peril::WindstormEuropean, Peril::Flood],
        };

        let uk_property = Risk {
            line_of_business: "property".to_string(),
            sum_insured: 500_000_000,
            territory: "UK".to_string(),
            limit: 200_000_000,
            attachment: 20_000_000,
            perils_covered: vec![Peril::Attritional],
        };

        let us_earthquake = Risk {
            line_of_business: "property".to_string(),
            sum_insured: 3_000_000_000,
            territory: "US-CA".to_string(),
            limit: 1_500_000_000,
            attachment: 200_000_000,
            perils_covered: vec![Peril::EarthquakeUS],
        };

        let jp_property = Risk {
            line_of_business: "property".to_string(),
            sum_insured: 3_000_000_000,  // £30M
            territory: "JP".to_string(),
            limit: 1_500_000_000,        // £15M — within small-syndicate £24M cap
            attachment: 200_000_000,     // £2M
            perils_covered: vec![Peril::EarthquakeJapan],
        };

        SimulationConfig {
            seed: 42,
            years: 5,
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
            // ── Brokers: 6 with different specialisms ─────────────────────────
            brokers: vec![
                // Broker 1: US property — large wind risks
                BrokerConfig {
                    id: BrokerId(1),
                    submissions_per_year: 200,
                    risks: vec![
                        large_us_wind.clone(),
                        medium_us_flood.clone(),
                        us_earthquake.clone(),
                        uk_property.clone(),
                    ],
                },
                // Broker 2: US property — mid risks
                BrokerConfig {
                    id: BrokerId(2),
                    submissions_per_year: 150,
                    risks: vec![
                        medium_us_flood.clone(),
                        large_us_wind.clone(),
                        uk_property.clone(),
                        eu_property.clone(),
                    ],
                },
                // Broker 3: European property / flood
                BrokerConfig {
                    id: BrokerId(3),
                    submissions_per_year: 120,
                    risks: vec![
                        eu_property.clone(),
                        uk_property.clone(),
                        medium_us_flood.clone(),
                        large_us_wind.clone(),
                    ],
                },
                // Broker 4: Casualty / liability — UK attritional book
                BrokerConfig {
                    id: BrokerId(4),
                    submissions_per_year: 100,
                    risks: vec![
                        uk_property.clone(),
                        eu_property.clone(),
                        medium_us_flood.clone(),
                        us_earthquake.clone(),
                    ],
                },
                // Broker 5: Marine / specialist — earthquake-heavy
                BrokerConfig {
                    id: BrokerId(5),
                    submissions_per_year: 80,
                    risks: vec![
                        jp_property.clone(),
                        us_earthquake.clone(),
                        large_us_wind.clone(),
                        eu_property.clone(),
                        medium_us_flood.clone(),
                    ],
                },
                // Broker 6: Mixed / global
                BrokerConfig {
                    id: BrokerId(6),
                    submissions_per_year: 150,
                    risks: vec![
                        jp_property,
                        large_us_wind,
                        eu_property,
                        us_earthquake,
                        uk_property,
                    ],
                },
            ],
        }
    }
}
