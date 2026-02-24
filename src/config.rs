use crate::types::InsurerId;

#[derive(Clone)]
pub struct InsurerConfig {
    pub id: InsurerId,
    pub initial_capital: i64, // signed to allow negative (no insolvency in MVP)
    /// E[attritional_loss] / sum_insured. Updated each year via EWMA from realized attritional
    /// burning cost. High-frequency data makes annual EWMA credible.
    /// Canonical: annual_rate × exp(mu + σ²/2) ≈ 0.030.
    pub attritional_elf: f64,
    /// E[cat_loss] / sum_insured. Anchored — never updated from experience. Derived from
    /// the cat model (Poisson frequency × expected damage fraction). A quiet cat year is not
    /// evidence of a lower cat rate; updating via EWMA produces systematic soft-market erosion.
    /// Canonical: annual_frequency × scale × shape / (shape − 1) ≈ 0.015.
    pub cat_elf: f64,
    /// Target combined loss ratio. ATP = (attritional_elf + cat_elf) / target_loss_ratio.
    /// With target_loss_ratio < 1, ATP exceeds expected loss by a built-in profit margin.
    pub target_loss_ratio: f64,
    /// EWMA credibility weight α ∈ (0, 1): new_att_elf = α × realized_att_lf + (1-α) × old.
    /// Higher α = faster response to recent experience; lower α = more weight on history.
    pub ewma_credibility: f64,
    /// Multiplicative loading above ATP applied in the underwriter channel.
    /// premium = ATP × (1 + profit_loading). Represents minimum risk/capital charge above
    /// actuarial expected loss. Canonical: 0.05 (5%).
    pub profit_loading: f64,
    /// Fraction of gross premium consumed by acquisition costs + overhead.
    /// Lloyd's 2024: 22.6% acquisition + 11.8% management ≈ 34.4%.
    pub expense_ratio: f64,
    /// Fraction of current capital committable to a single risk net line (Lloyd's: 0.30).
    /// None = no limit (tests only; the canonical config always sets Some).
    pub net_line_capacity: Option<f64>,
    /// Fraction of capital allocated to cover the 1-in-200 cat scenario (Lloyd's: ~0.30).
    /// Effective cat aggregate limit = solvency_capital_fraction × capital / pml_damage_fraction_200.
    /// None = no limit (tests only; the canonical config always sets Some).
    pub solvency_capital_fraction: Option<f64>,
    /// Cycle sensitivity: how aggressively the underwriter reprices after a bad own-CR year.
    /// 0.0 = through-cycle writer; 0.5 = cycle trader.
    pub cycle_sensitivity: f64,
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
            // 5 insurers, each endowed with 500M USD capital at construction; persists year-over-year.
            insurers: (1..=5)
                .map(|i| InsurerConfig {
                    id: InsurerId(i),
                    initial_capital: 50_000_000_000, // 500M USD in cents
                    attritional_elf: 0.030, // annual_rate=2.0 × E[df]=1.5% → att_ELF=3.0%
                    cat_elf: 0.033,         // frequency=0.5 × E[df]=6.7% → cat_ELF=3.3%; anchored
                    target_loss_ratio: 0.80, // gross (pre-reinsurance) pricing; benign CR ≈ 70%
                    ewma_credibility: 0.3,
                    expense_ratio: 0.344, // Lloyd's 2024: 22.6% acquisition + 11.8% management
                    profit_loading: 0.05, // 5% markup above ATP; MS3 risk/capital charge
                    net_line_capacity: Some(0.30),
                    solvency_capital_fraction: Some(0.30),
                    // 0.10 = through-cycle writer; 0.50 = cycle trader.
                    cycle_sensitivity: [0.10, 0.20, 0.30, 0.40, 0.50][(i - 1) as usize],
                })
                .collect(),
            n_insureds: 100,
            attritional: AttritionalConfig {
                annual_rate: 2.0,  // ~2 claims/yr per insured; freq × E[df] = ELF_att ≈ 3.0%
                mu: -4.7,          // E[df] = exp(-4.7 + 0.5) = exp(-4.2) ≈ 1.5%
                sigma: 1.0,
            },
            catastrophe: CatConfig {
                annual_frequency: 0.5,  // one cat event every 2 years on average
                pareto_scale: 0.04,     // minimum 4% damage fraction ($2M on $50M); gross book
                pareto_shape: 2.5,      // E[df] = 0.04 × 2.5 / 1.5 = 6.7%; fatter tail than shape=3
            },
        }
    }
}
