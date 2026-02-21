use crate::events::{Peril, Risk};
use crate::types::{BrokerId, InsuredId, SyndicateId};

pub struct SyndicateConfig {
    pub id: SyndicateId,
    pub capital: u64,
    pub rate_on_line_bps: u32,
}

pub struct InsuredConfig {
    pub id: InsuredId,
    pub name: &'static str,
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
            // 26 named insureds total; round-robin across each broker's insured
            // assets preserves the original submission volume (800/year).
            brokers: vec![
                // Broker 1: US property — large wind risks (200 subs/year, 4 insureds)
                BrokerConfig {
                    id: BrokerId(1),
                    submissions_per_year: 200,
                    insureds: vec![
                        InsuredConfig { id: InsuredId(101), name: "Atlantic Energy Corp",       assets: vec![large_us_wind.clone()] },
                        InsuredConfig { id: InsuredId(102), name: "Gulf Flood Holdings",        assets: vec![medium_us_flood.clone()] },
                        InsuredConfig { id: InsuredId(103), name: "Pacific Seismic Group",      assets: vec![us_earthquake.clone()] },
                        InsuredConfig { id: InsuredId(104), name: "Anglo-American Properties",  assets: vec![uk_property.clone()] },
                    ],
                },
                // Broker 2: US property — mid risks (150 subs/year, 4 insureds)
                BrokerConfig {
                    id: BrokerId(2),
                    submissions_per_year: 150,
                    insureds: vec![
                        InsuredConfig { id: InsuredId(201), name: "Mississippi Commercial Trust",    assets: vec![medium_us_flood.clone()] },
                        InsuredConfig { id: InsuredId(202), name: "Southern Wind Power",             assets: vec![large_us_wind.clone()] },
                        InsuredConfig { id: InsuredId(203), name: "Thames Valley Properties",        assets: vec![uk_property.clone()] },
                        InsuredConfig { id: InsuredId(204), name: "Continental European Holdings",   assets: vec![eu_property.clone()] },
                    ],
                },
                // Broker 3: European property / flood (120 subs/year, 4 insureds)
                BrokerConfig {
                    id: BrokerId(3),
                    submissions_per_year: 120,
                    insureds: vec![
                        InsuredConfig { id: InsuredId(301), name: "Rhine Delta Industrial",      assets: vec![eu_property.clone()] },
                        InsuredConfig { id: InsuredId(302), name: "British Retail Property",     assets: vec![uk_property.clone()] },
                        InsuredConfig { id: InsuredId(303), name: "North Sea Flood Group",       assets: vec![medium_us_flood.clone()] },
                        InsuredConfig { id: InsuredId(304), name: "US Atlantic Wind Portfolio",  assets: vec![large_us_wind.clone()] },
                    ],
                },
                // Broker 4: Casualty / liability — UK attritional book (100 subs/year, 4 insureds)
                BrokerConfig {
                    id: BrokerId(4),
                    submissions_per_year: 100,
                    insureds: vec![
                        InsuredConfig { id: InsuredId(401), name: "London Commercial Property",  assets: vec![uk_property.clone()] },
                        InsuredConfig { id: InsuredId(402), name: "European Property Alliance",  assets: vec![eu_property.clone()] },
                        InsuredConfig { id: InsuredId(403), name: "US Flood Risk Partners",      assets: vec![medium_us_flood.clone()] },
                        InsuredConfig { id: InsuredId(404), name: "Pacific Seismic Holdings",    assets: vec![us_earthquake.clone()] },
                    ],
                },
                // Broker 5: Marine / specialist — earthquake-heavy (80 subs/year, 5 insureds)
                BrokerConfig {
                    id: BrokerId(5),
                    submissions_per_year: 80,
                    insureds: vec![
                        InsuredConfig { id: InsuredId(501), name: "Osaka Manufacturing Hub",   assets: vec![jp_property.clone()] },
                        InsuredConfig { id: InsuredId(502), name: "Silicon Valley Industrial", assets: vec![us_earthquake.clone()] },
                        InsuredConfig { id: InsuredId(503), name: "Gulf States Energy Corp",   assets: vec![large_us_wind.clone()] },
                        InsuredConfig { id: InsuredId(504), name: "Hamburg Port Authority",    assets: vec![eu_property.clone()] },
                        InsuredConfig { id: InsuredId(505), name: "New Orleans Logistics",     assets: vec![medium_us_flood.clone()] },
                    ],
                },
                // Broker 6: Mixed / global (150 subs/year, 5 insureds)
                BrokerConfig {
                    id: BrokerId(6),
                    submissions_per_year: 150,
                    insureds: vec![
                        InsuredConfig { id: InsuredId(601), name: "Tokyo Commercial Holdings",   assets: vec![jp_property] },
                        InsuredConfig { id: InsuredId(602), name: "Gulf Energy Group",           assets: vec![large_us_wind] },
                        InsuredConfig { id: InsuredId(603), name: "Northern Europe Properties",  assets: vec![eu_property] },
                        InsuredConfig { id: InsuredId(604), name: "West Coast Seismic Trust",    assets: vec![us_earthquake] },
                        InsuredConfig { id: InsuredId(605), name: "British Industrial Portfolio", assets: vec![uk_property] },
                    ],
                },
            ],
        }
    }
}
