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
    /// Override the market-calibrated 1-in-200 damage fraction used to compute the effective cat
    /// aggregate limit. `None` = use the market-wide value derived from the cat model parameters.
    /// `Some(x)` = use x as the insurer's internal model assumption — a lower value inflates the
    /// denominator and raises the effective cat limit, reflecting an optimistic internal model.
    pub pml_damage_fraction_override: Option<f64>,
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
    /// Upper truncation point for the Pareto damage fraction draw ∈ (0, 1].
    /// Acts as a proxy for the maximum net per-occurrence retained loss fraction in the
    /// absence of explicit reinsurance modelling. Physical justification: a single cat
    /// event cannot destroy more than ~50% of a geographically diversified portfolio.
    /// Canonical: 0.50. Set to 1.0 to disable truncation.
    pub max_damage_fraction: f64,
    /// Geographic territories this peril can strike. Each `LossEvent` targets one
    /// territory drawn uniformly at random from this list. Insureds are distributed
    /// across these territories cyclically at construction time.
    /// Canonical: 3 territories → ~33% of insureds hit per event.
    /// Use a single-element list (`["US-SE"]`) in tests to preserve full-portfolio exposure.
    pub territories: Vec<String>,
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
    /// Number of insureds. Asset value: 25M USD each.
    pub n_insureds: usize,
    pub attritional: AttritionalConfig,
    pub catastrophe: CatConfig,
    /// Number of insurers solicited per submission. None = all insurers.
    pub quotes_per_submission: Option<usize>,
    /// Maximum rate on line an insured will accept (premium / sum_insured).
    /// Quotes above this threshold are rejected; the insured retries at next renewal.
    /// Canonical: 0.15 — well above current 6–8% rate band; becomes binding once
    /// capital-linked pricing raises rates post-cat.
    pub max_rate_on_line: f64,
}

/// Insured asset value: 25M USD in cents.
pub const ASSET_VALUE: u64 = 2_500_000_000;

impl SimulationConfig {
    pub fn canonical() -> Self {
        SimulationConfig {
            seed: 42,
            years: 20,
            warmup_years: 5,
            // 5 established insurers + 3 aggressive small entrants, each 150M USD capital.
            //
            // Cat ELF is territory-adjusted: freq=0.5 × E[df]=6.7% ÷ 3 territories = 1.1%.
            // target_loss_ratio is set so ATP × (1+loading) / target_LR maintains ~7% rate-on-line.
            //
            // Established (IDs 1–5): cat_elf=0.011, target_LR=0.62, profit_loading=0.05.
            //   ATP = (0.030+0.011)/0.62 = 6.6%; premium ≈ 6.9% of SI.
            //
            // Aggressive (IDs 6–8): optimistic cat_elf=0.005 (half territory-adjusted anchor),
            //   target_LR=0.70, zero profit_loading. Premium ≈ 5.0% of SI — ~27% cheaper.
            //   pml_override=0.126 (raw per-event; territory_factor applied in from_config() → 0.042
            //   effective), doubling the cat aggregate limit vs established.
            insurers: {
                let mut insurers: Vec<InsurerConfig> = (1..=5)
                    .map(|i| InsurerConfig {
                        id: InsurerId(i),
                        initial_capital: 15_000_000_000, // 150M USD in cents
                        attritional_elf: 0.030, // annual_rate=2.0 × E[df]=1.5% → att_ELF=3.0%
                        cat_elf: 0.011, // freq=0.5 × E[df]=6.7% ÷ 3 territories → cat_ELF=1.1%
                        target_loss_ratio: 0.62, // (att+cat ELF)/rate = 0.041/0.066 ≈ 0.62
                        ewma_credibility: 0.3,
                        expense_ratio: 0.344, // Lloyd's 2024: 22.6% acquisition + 11.8% management
                        profit_loading: 0.05, // 5% markup above ATP; MS3 risk/capital charge
                        net_line_capacity: Some(0.30),
                        solvency_capital_fraction: Some(0.30),
                        pml_damage_fraction_override: None, // use market-calibrated pml_200 ≈ 0.252
                    })
                    .collect();
                // Aggressive small entrants: undercut on price, undercapitalised relative to tail risk.
                for j in 0..3 {
                    insurers.push(InsurerConfig {
                        id: InsurerId(6 + j as u64),
                        initial_capital: 15_000_000_000, // 150M USD in cents
                        attritional_elf: 0.030,           // same attritional assumption
                        cat_elf: 0.005, // optimistic: half the territory-adjusted anchor (0.011)
                        target_loss_ratio: 0.70, // (0.030+0.005)/0.050 = 0.70; no profit loading
                        ewma_credibility: 0.3,
                        expense_ratio: 0.344,
                        profit_loading: 0.00, // no risk/capital loading — pure actuarial floor
                        net_line_capacity: Some(0.30),
                        solvency_capital_fraction: Some(0.30),
                        // Optimistic internal model: raw per-event pml override = 0.126.
                        // from_config() applies territory_factor (÷3) → effective pml = 0.042,
                        // half the established effective pml (0.084). Doubles cat agg limit
                        // vs established, allowing aggressive writers to accumulate far more
                        // exposure than their capital prudently supports.
                        pml_damage_fraction_override: Some(0.126),
                    });
                }
                insurers
            },
            n_insureds: 100,
            attritional: AttritionalConfig {
                annual_rate: 2.0,  // ~2 claims/yr per insured; freq × E[df] = ELF_att ≈ 3.0%
                mu: -4.7,          // E[df] = exp(-4.7 + 0.5) = exp(-4.2) ≈ 1.5%
                sigma: 1.0,
            },
            catastrophe: CatConfig {
                annual_frequency: 0.5,    // one cat event every 2 years on average
                pareto_scale: 0.04,       // minimum 4% damage fraction ($2M on $50M); gross book
                pareto_shape: 2.5,        // E[df] = 0.04 × 2.5 / 1.5 = 6.7%; fatter tail than shape=3
                max_damage_fraction: 0.50, // cap upper tail — proxy for max net per-occurrence
                                           // retention absent explicit RI; a single hurricane
                                           // cannot destroy >50% of a spread portfolio
                territories: vec![
                    "US-NE".to_string(),
                    "US-SE".to_string(),
                    "US-Gulf".to_string(),
                ],
            },
            quotes_per_submission: None, // solicit all 8 insurers per submission
            max_rate_on_line: 0.15, // 15% RoL ceiling — above current band, binding post-hardening
        }
    }
}
