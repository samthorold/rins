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
    /// Sensitivity of the capital-depletion pricing adjustment.
    /// cap_depletion_adj = clamp(depletion_ratio × depletion_sensitivity, 0.0, 0.30).
    /// Canonical: 1.0 — a fully depleted insurer adds the maximum 0.30 loading.
    /// Set to 0.0 in tests to disable the effect and preserve prior behaviour.
    pub depletion_sensitivity: f64,
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

/// One severity class in the compound catastrophe model (e.g. "minor" or "major").
/// `schedule_loss_events` runs one independent Poisson draw per class and samples
/// a damage fraction from that class's Pareto distribution.
#[derive(Clone)]
pub struct CatEventClass {
    /// Short label for debugging and catalog output ("minor", "major", …).
    pub label: String,
    /// Expected number of events of this class per year (Poisson rate).
    pub annual_frequency: f64,
    /// Pareto minimum damage fraction (scale > 0, < 1).
    pub pareto_scale: f64,
    /// Pareto tail index α; shape > 1 for finite mean.
    pub pareto_shape: f64,
    /// Upper truncation for Pareto draws ∈ (0, 1].
    /// Proxy for maximum net per-occurrence retained severity absent explicit RI.
    pub max_damage_fraction: f64,
}

/// Compound catastrophe peril parameters.
/// Each event class has its own Poisson frequency and Pareto severity distribution,
/// allowing the model to separate high-frequency/low-severity (minor) from
/// low-frequency/high-severity (major) events.
#[derive(Clone)]
pub struct CatConfig {
    /// One or more severity classes. `schedule_loss_events` draws independently per class.
    pub event_classes: Vec<CatEventClass>,
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
    /// When true, no cat `LossEvent`s are scheduled. Attritional losses still run.
    /// Useful for isolating attritional dynamics without cat noise.
    pub disable_cats: bool,
}

/// Insured asset value: 25M USD in cents.
pub const ASSET_VALUE: u64 = 2_500_000_000;

impl SimulationConfig {
    pub fn canonical() -> Self {
        SimulationConfig {
            seed: 42,
            years: 50,
            warmup_years: 5,
            // 8 homogeneous established insurers, each 150M USD capital.
            //
            // Cat ELF is territory-adjusted: freq=0.8 × E[df]=10.83% ÷ 3 territories = 2.89%.
            // ATP = (0.050+0.030)/0.62 = 12.9%; premium (TP) ≈ 13.55% of SI.
            insurers: (1..=8)
                .map(|i| InsurerConfig {
                    id: InsurerId(i),
                    initial_capital: 15_000_000_000, // 150M USD in cents
                    attritional_elf: 0.050, // annual_rate=2.0 × E[df]=2.5% → att_ELF=5.0%
                    // Compound cat ELF (÷ 3 territories):
                    //   minor: λ=1.0, E[df]=0.42% → 0.14% per insured
                    //   major: λ=0.8, E[df]=10.83% → 2.89% per insured
                    //   total cat_ELF ≈ 3.03% → rounded to 0.030
                    cat_elf: 0.030,
                    target_loss_ratio: 0.62,
                    ewma_credibility: 0.3,
                    expense_ratio: 0.344, // Lloyd's 2024: 22.6% acquisition + 11.8% management
                    profit_loading: 0.05, // 5% markup above ATP; MS3 risk/capital charge
                    net_line_capacity: Some(0.30),
                    solvency_capital_fraction: Some(0.30),
                    pml_damage_fraction_override: None,
                    depletion_sensitivity: 1.0,
                })
                .collect(),
            n_insureds: 100,
            attritional: AttritionalConfig {
                annual_rate: 2.0,   // ~2 claims/yr per insured; freq × E[df] = ELF_att ≈ 5.0%
                mu: -3.73,          // E[df] = exp(-3.73 + 0.045) = exp(-3.685) ≈ 2.5%
                sigma: 0.3,         // tight spread — attritional = high-frequency, small losses;
                                    // CV_per_claim ≈ 0.31 → aggregate CV across 57 policies ≈ 3%
                                    // (was sigma=1.0 → CV≈15%, masking cat signal)
            },
            catastrophe: CatConfig {
                event_classes: vec![
                    // Minor events (tropical storms / Cat 1–2): high frequency, low severity.
                    // Provides chronic EWMA drag without capital-depleting shocks.
                    // Return period: 1-in-10 → scale × (10 × 1.0)^(1/3.5) ≈ 0.009
                    CatEventClass {
                        label: "minor".to_string(),
                        annual_frequency: 1.0,
                        pareto_scale: 0.003,  // minimum 0.3% df — below att noise
                        pareto_shape: 3.5,    // E[df] = 0.003 × 3.5/2.5 = 0.42%
                        max_damage_fraction: 0.08,
                    },
                    // Major events (Cat 3–5): lower frequency, capital-depleting severity.
                    // Return period: 1-in-200 → scale × (200 × 0.8)^(1/2.5) ≈ 0.495
                    CatEventClass {
                        label: "major".to_string(),
                        annual_frequency: 0.8,
                        pareto_scale: 0.065,  // minimum 6.5% df ($1.625M on $25M)
                        pareto_shape: 2.5,    // E[df] = 0.065 × 2.5/1.5 = 10.83%
                        max_damage_fraction: 0.50,
                    },
                ],
                territories: vec![
                    "US-NE".to_string(),
                    "US-SE".to_string(),
                    "US-Gulf".to_string(),
                ],
            },
            quotes_per_submission: Some(4), // solicit top-4 (by relationship score) per submission
            max_rate_on_line: 0.30, // 30% RoL ceiling — allows market recovery after cat-driven EWMA spikes
            disable_cats: false,
        }
    }
}
