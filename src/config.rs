use crate::types::InsurerId;

pub struct InsurerConfig {
    pub id: InsurerId,
    pub initial_capital: i64, // signed to allow negative (no insolvency in MVP)
    /// Premium as a fraction of sum_insured (e.g. 0.02 = 2% rate on line).
    pub rate: f64,
}

/// Attritional peril parameters — LogNormal damage fraction, Poisson frequency.
pub struct AttritionalConfig {
    /// Expected number of attritional claims per insured per year.
    pub annual_rate: f64,
    /// LogNormal ln-space mean of the damage fraction.
    pub mu: f64,
    /// LogNormal ln-space std-dev of the damage fraction.
    pub sigma: f64,
}

/// Catastrophe peril parameters — Pareto damage fraction, Poisson market-wide frequency.
pub struct CatConfig {
    /// Expected number of cat events per year (market-wide).
    pub annual_frequency: f64,
    /// Pareto scale: minimum damage fraction (> 0, < 1).
    pub pareto_scale: f64,
    /// Pareto shape: tail index α (> 1 for finite mean).
    pub pareto_shape: f64,
}

pub struct SimulationConfig {
    pub seed: u64,
    pub years: u32,
    pub insurers: Vec<InsurerConfig>,
    /// Number of small-asset insureds (90% of population). Asset value: 50M USD.
    pub n_small_insureds: usize,
    /// Number of large-asset insureds (10% of population). Asset value: 1B USD.
    pub n_large_insureds: usize,
    pub attritional: AttritionalConfig,
    pub catastrophe: CatConfig,
}

/// Small insured asset value: 50M USD in cents.
pub const SMALL_ASSET_VALUE: u64 = 5_000_000_000;
/// Large insured asset value: 1B USD in cents.
pub const LARGE_ASSET_VALUE: u64 = 100_000_000_000;

impl SimulationConfig {
    pub fn canonical() -> Self {
        SimulationConfig {
            seed: 42,
            years: 20,
            // 5 insurers, each endowed with 1B USD capital each year.
            insurers: (1..=5)
                .map(|i| InsurerConfig {
                    id: InsurerId(i),
                    initial_capital: 100_000_000_000, // 1B USD in cents
                    rate: 0.35,
                })
                .collect(),
            n_small_insureds: 90,
            n_large_insureds: 10,
            attritional: AttritionalConfig {
                annual_rate: 2.0,  // 2 claims per insured per year on average
                mu: -3.0,          // E[df] = exp(-3.0 + 0.5) ≈ 8.2%
                sigma: 1.0,
            },
            catastrophe: CatConfig {
                annual_frequency: 0.5,  // one cat event every 2 years on average
                pareto_scale: 0.05,     // minimum 5% damage fraction
                pareto_shape: 1.5,      // E[df] = 0.05 × 1.5 / 0.5 = 0.15 (unclipped)
            },
        }
    }
}
