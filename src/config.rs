use crate::types::InsurerId;

#[derive(Clone)]
pub struct InsurerConfig {
    pub id: InsurerId,
    pub initial_capital: i64, // signed to allow negative (no insolvency in MVP)
    /// E[annual_loss] / sum_insured, combining all perils. Initial prior; updated each year by EWMA.
    /// Canonical: E_att(0.164) + E_cat(0.075) ≈ 0.239.
    pub expected_loss_fraction: f64,
    /// Target combined loss ratio. ATP = expected_loss_fraction / target_loss_ratio.
    /// With target_loss_ratio < 1, ATP includes a profit margin above expected loss.
    pub target_loss_ratio: f64,
    /// EWMA credibility weight α ∈ (0, 1): new_elf = α × realized_lf + (1-α) × old_elf.
    /// Higher α = faster response to recent experience; lower α = more weight on history.
    pub ewma_credibility: f64,
    /// Fraction of gross premium consumed by acquisition costs + overhead.
    /// Lloyd's 2024: 22.6% acquisition + 11.8% management ≈ 34.4%.
    pub expense_ratio: f64,
    /// Max WindstormAtlantic aggregate sum_insured across all in-force policies (None = unlimited).
    pub max_cat_aggregate: Option<u64>,
    /// Max sum_insured on any single risk (None = unlimited).
    pub max_line_size: Option<u64>,
}

/// Attritional peril parameters — LogNormal damage fraction, Poisson frequency.
#[derive(Clone)]
pub struct AttritionalConfig {
    /// Expected number of attritional claims per insured per year.
    pub annual_rate: f64,
    /// LogNormal ln-space mean of the damage fraction.
    pub mu: f64,
    /// LogNormal ln-space std-dev of the damage fraction.
    pub sigma: f64,
}

/// Catastrophe peril parameters — Pareto damage fraction, Poisson market-wide frequency.
#[derive(Clone)]
pub struct CatConfig {
    /// Expected number of cat events per year (market-wide).
    pub annual_frequency: f64,
    /// Pareto scale: minimum damage fraction (> 0, < 1).
    pub pareto_scale: f64,
    /// Pareto shape: tail index α (> 1 for finite mean).
    pub pareto_shape: f64,
}

#[derive(Clone)]
pub struct SimulationConfig {
    pub seed: u64,
    /// Number of analysis years. The simulation runs `warmup_years + years` in total;
    /// only the analysis years are reported by `analyse_sim.py`.
    pub years: u32,
    /// Warm-up years prepended before the analysis period. Used to let the EWMA
    /// stabilise past the staggered year-1 partial-exposure artefact. Not reported.
    pub warmup_years: u32,
    pub insurers: Vec<InsurerConfig>,
    /// Number of insureds. Asset value: 50M USD each.
    pub n_insureds: usize,
    pub attritional: AttritionalConfig,
    pub catastrophe: CatConfig,
}

/// Insured asset value: 50M USD in cents.
pub const ASSET_VALUE: u64 = 5_000_000_000;

impl SimulationConfig {
    pub fn canonical() -> Self {
        SimulationConfig {
            seed: 42,
            years: 20,
            warmup_years: 2,
            // 5 insurers, each endowed with 1B USD capital each year.
            insurers: (1..=5)
                .map(|i| InsurerConfig {
                    id: InsurerId(i),
                    initial_capital: 100_000_000_000, // 1B USD in cents
                    expected_loss_fraction: 0.239, // E_att(0.164) + E_cat(0.075)
                    target_loss_ratio: 0.55, // 1 − 0.344 expenses − 0.106 profit → CR ≈ 89.4%
                    ewma_credibility: 0.3,
                    expense_ratio: 0.344, // Lloyd's 2024: 22.6% acquisition + 11.8% management
                    max_cat_aggregate: None,
                    max_line_size: None,
                })
                .collect(),
            n_insureds: 100,
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
