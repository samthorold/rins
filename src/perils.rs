use rand::Rng;
use rand_distr::{Distribution, LogNormal, Pareto, Poisson};
use serde::Serialize;

use crate::config::{AttritionalConfig, CatConfig};
use crate::events::{Event, Peril, Risk};
use crate::types::{Day, InsuredId, Year};

/// A damage fraction model: `sample()` returns a value in `[0.0, 1.0]`
/// representing the fraction of the insured asset's total value that is damaged.
/// Raw distribution outputs are clipped to 1.0.
pub enum DamageFractionModel {
    /// Log-normal damage fraction; ln-space params.
    /// E[X] = exp(mu + sigma²/2), clipped to 1.0.
    LogNormal { mu: f64, sigma: f64 },
    /// Pareto damage fraction: `scale` = minimum value (< 1.0), `shape` = tail index α.
    /// E[X] = scale * shape / (shape − 1)  (requires shape > 1), clipped to `cap`.
    /// `cap` truncates the upper tail; canonical value is 0.50, representing the maximum
    /// plausible net per-occurrence loss fraction for a diversified book written without
    /// reinsurance. Absent an explicit RI treaty this is the appropriate proxy for the
    /// maximum retained severity; a hurricane cannot destroy more than ~50% of a
    /// geographically spread portfolio.
    Pareto { scale: f64, shape: f64, cap: f64 },
}

impl DamageFractionModel {
    /// Sample a damage fraction in `[0.0, 1.0]`.
    pub fn sample(&self, rng: &mut impl Rng) -> f64 {
        match self {
            DamageFractionModel::LogNormal { mu, sigma } => {
                let dist = LogNormal::new(*mu, *sigma).expect("invalid LogNormal params");
                dist.sample(rng).min(1.0_f64)
            }
            DamageFractionModel::Pareto { scale, shape, cap } => {
                let dist = Pareto::new(*scale, *shape).expect("invalid Pareto params");
                dist.sample(rng).min(*cap)
            }
        }
    }
}

/// Schedule market-wide catastrophe `LossEvent`s for `year`.
///
/// Draws a Poisson count from `cat.annual_frequency`, then for each event
/// draws a uniform day-offset within the year. A single damage fraction is
/// drawn at dispatch time (in `Market::on_loss_event`) and shared across all
/// affected policies, modelling the correlated intensity of a physical event.
///
/// `next_id` is mutated in-place; the caller owns the event-id counter.
pub fn schedule_loss_events(
    cat: &CatConfig,
    year: Year,
    rng: &mut impl Rng,
    next_id: &mut u64,
) -> Vec<(Day, Event)> {
    if cat.annual_frequency <= 0.0 || cat.territories.is_empty() {
        return vec![];
    }
    let year_start = Day::year_start(year);
    let poisson = Poisson::new(cat.annual_frequency).expect("invalid Poisson lambda");
    let n = poisson.sample(rng) as u64;
    (0..n)
        .map(|_| {
            let offset = rng.random_range(1_u64..360);
            let event_id = *next_id;
            *next_id += 1;
            let territory_idx = rng.random_range(0..cat.territories.len());
            let territory = cat.territories[territory_idx].clone();
            (year_start.offset(offset), Event::LossEvent { event_id, peril: Peril::WindstormAtlantic, territory })
        })
        .collect()
}

/// Schedule attritional `AssetDamage` events for a single insured.
///
/// Called at `CoverageRequested` time so all insureds accumulate attritional
/// exposure for the year regardless of whether they ultimately bind a policy.
/// `from_day` is the `CoverageRequested` day; all losses are scheduled strictly
/// after it so the DES log remains day-ordered (no event is in the past).
/// Draws a Poisson count from `config.annual_rate`, then for each occurrence
/// draws a random day in `(from_day, year_end]` and a damage fraction.
pub fn schedule_attritional_losses_for_insured(
    insured_id: InsuredId,
    risk: &Risk,
    from_day: Day,
    rng: &mut impl Rng,
    config: &AttritionalConfig,
) -> Vec<(Day, Event)> {
    if !risk.perils_covered.contains(&Peril::Attritional) {
        return vec![];
    }
    let year_end = Day::year_end(from_day.year());
    if from_day >= year_end {
        return vec![];
    }
    let model = DamageFractionModel::LogNormal { mu: config.mu, sigma: config.sigma };
    let Ok(poisson) = Poisson::new(config.annual_rate) else { return vec![] };
    let n = poisson.sample(rng) as u64;

    (0..n)
        .filter_map(|_| {
            let day = Day(rng.random_range(from_day.0 + 1..=year_end.0));
            let damage_fraction = model.sample(rng);
            let ground_up_loss = (damage_fraction * risk.sum_insured as f64) as u64;
            if ground_up_loss == 0 {
                return None;
            }
            Some((
                day,
                Event::AssetDamage { insured_id, peril: Peril::Attritional, ground_up_loss },
            ))
        })
        .collect()
}

/// A single entry in a standalone catastrophe event catalog.
#[derive(Serialize)]
pub struct CatCatalogEntry {
    pub year: u32,
    /// Absolute day within the year (1–359).
    pub day: u64,
    pub territory: String,
    pub damage_fraction: f64,
    pub peril: String,
}

/// Generate `n_years` of stochastic cat events independent of the market simulation.
///
/// Damage fractions are sampled at generation time from the cat damage model — unlike
/// the main simulation, which defers sampling until `Market::on_loss_event` fires. This
/// is appropriate for a standalone catalog tool where the damage fraction is an intrinsic
/// property of the event rather than a market-state-dependent quantity.
pub fn generate_cat_catalog(
    cat: &CatConfig,
    n_years: u32,
    rng: &mut impl Rng,
) -> Vec<CatCatalogEntry> {
    if cat.annual_frequency <= 0.0 || cat.territories.is_empty() {
        return vec![];
    }
    let damage_model = DamageFractionModel::Pareto {
        scale: cat.pareto_scale,
        shape: cat.pareto_shape,
        cap: cat.max_damage_fraction,
    };
    let poisson = Poisson::new(cat.annual_frequency).expect("invalid Poisson lambda");
    let mut entries = Vec::new();
    for year in 1..=n_years {
        let n = poisson.sample(rng) as u64;
        for _ in 0..n {
            let day = rng.random_range(1_u64..360);
            let territory_idx = rng.random_range(0..cat.territories.len());
            let territory = cat.territories[territory_idx].clone();
            let damage_fraction = damage_model.sample(rng);
            entries.push(CatCatalogEntry {
                year,
                day,
                territory,
                damage_fraction,
                peril: "WindstormAtlantic".to_string(),
            });
        }
    }
    entries
}

#[cfg(test)]
mod tests {
    use rand::SeedableRng;
    use rand_chacha::ChaCha20Rng;

    use super::*;
    use crate::config::{AttritionalConfig, CatConfig, ASSET_VALUE};
    use crate::types::{Day, InsuredId, Year};

    fn rng() -> ChaCha20Rng {
        ChaCha20Rng::seed_from_u64(42)
    }

    fn att_config() -> AttritionalConfig {
        AttritionalConfig { annual_rate: 10.0, mu: -3.0, sigma: 1.0 }
    }

    fn cat_config() -> CatConfig {
        CatConfig { annual_frequency: 2.0, pareto_scale: 0.05, pareto_shape: 1.5, max_damage_fraction: 1.0, territories: vec!["US-SE".to_string()] }
    }

    fn small_risk() -> Risk {
        Risk {
            sum_insured: ASSET_VALUE,
            territory: "US-SE".to_string(),
            perils_covered: vec![Peril::WindstormAtlantic, Peril::Attritional],
        }
    }

    // ── DamageFractionModel tests ─────────────────────────────────────────────

    /// LogNormal with mu=-2.0, sigma=1.0: E[X] ≈ 0.223 (unclipped). 10k samples in ±30%.
    #[test]
    fn damage_fraction_lognormal_mean_in_expected_range() {
        let model = DamageFractionModel::LogNormal { mu: -2.0, sigma: 1.0 };
        let mut rng = rng();
        let n = 10_000;
        let mean: f64 = (0..n).map(|_| model.sample(&mut rng)).sum::<f64>() / n as f64;
        let expected = (-2.0_f64 + 1.0_f64 * 1.0 / 2.0).exp(); // ≈ 0.223
        let lo = expected * 0.70;
        let hi = expected * 1.30;
        assert!(
            mean >= lo && mean <= hi,
            "DamageFraction LogNormal mean {mean:.4} outside [{lo:.4}, {hi:.4}]"
        );
    }

    /// All samples must be in [0.0, 1.0] — clipping is enforced.
    #[test]
    fn damage_fraction_sample_always_in_unit_interval() {
        let model = DamageFractionModel::LogNormal { mu: 1.0, sigma: 2.0 }; // many raw > 1
        let mut rng = rng();
        for _ in 0..1_000 {
            let v = model.sample(&mut rng);
            assert!(v >= 0.0 && v <= 1.0, "sample {v} outside [0, 1]");
        }
    }

    /// Pareto tail is heavier than LogNormal with similar median.
    #[test]
    fn damage_fraction_pareto_tail_heavier_than_lognormal() {
        use rand_distr::Distribution;
        let mut rng = rng();
        let n = 10_000usize;
        let mut pareto_samples: Vec<f64> = (0..n)
            .map(|_| Pareto::new(0.01_f64, 1.5_f64).unwrap().sample(&mut rng))
            .collect();
        let mut lognorm_samples: Vec<f64> = (0..n)
            .map(|_| LogNormal::new((0.01_f64).ln(), 0.5_f64).unwrap().sample(&mut rng))
            .collect();
        pareto_samples.sort_unstable_by(f64::total_cmp);
        lognorm_samples.sort_unstable_by(f64::total_cmp);
        let p99_pareto = pareto_samples[n * 99 / 100];
        let p99_lognorm = lognorm_samples[n * 99 / 100];
        assert!(
            p99_pareto > p99_lognorm,
            "Pareto 99th pct {p99_pareto:.4} should exceed LogNormal 99th pct {p99_lognorm:.4}"
        );
    }

    // ── schedule_loss_events tests ────────────────────────────────────────────

    /// Every LossEvent must carry WindstormAtlantic peril.
    #[test]
    fn schedule_loss_events_returns_correct_peril() {
        let mut rng = rng();
        let mut next_id = 0u64;
        let events = schedule_loss_events(&cat_config(), Year(1), &mut rng, &mut next_id);
        assert!(!events.is_empty(), "expected events with lambda=2.0");
        for (_, e) in &events {
            assert!(
                matches!(e, Event::LossEvent { peril: Peril::WindstormAtlantic, .. }),
                "expected WindstormAtlantic, got {e:?}"
            );
        }
    }

    /// With λ=2.0 over 100 years, mean annual count must lie in [1.5, 2.5].
    #[test]
    fn poisson_count_is_reasonable() {
        let cfg = CatConfig { annual_frequency: 2.0, pareto_scale: 0.05, pareto_shape: 1.5, max_damage_fraction: 1.0, territories: vec!["US-SE".to_string()] };
        let mut rng = rng();
        let years = 100u32;
        let mut total = 0usize;
        let mut next_id = 0u64;
        for y in 1..=years {
            let events = schedule_loss_events(&cfg, Year(y), &mut rng, &mut next_id);
            total += events.len();
        }
        let mean = total as f64 / years as f64;
        assert!(mean >= 1.5 && mean <= 2.5, "mean annual count {mean:.2} outside [1.5, 2.5]");
    }

    /// All LossEventIds across 3 years must be unique.
    #[test]
    fn loss_event_ids_are_unique() {
        use std::collections::HashSet;
        let mut rng = rng();
        let mut next_id = 0u64;
        let mut seen = HashSet::new();
        for y in 1..=3u32 {
            let events = schedule_loss_events(&cat_config(), Year(y), &mut rng, &mut next_id);
            for (_, e) in events {
                if let Event::LossEvent { event_id, .. } = e {
                    assert!(seen.insert(event_id), "duplicate event_id {event_id}");
                }
            }
        }
    }

    /// All scheduled days must lie within [year_start+1, year_start+359].
    #[test]
    fn scheduled_days_within_year() {
        let cfg = CatConfig { annual_frequency: 10.0, pareto_scale: 0.05, pareto_shape: 1.5, max_damage_fraction: 1.0, territories: vec!["US-SE".to_string()] };
        let mut rng = rng();
        let mut next_id = 0u64;
        let year = Year(3);
        let year_start = Day::year_start(year);
        let events = schedule_loss_events(&cfg, year, &mut rng, &mut next_id);
        assert!(!events.is_empty(), "expected events with lambda=10");
        for (day, _) in &events {
            assert!(
                day.0 > year_start.0 && day.0 <= year_start.0 + 359,
                "day {} outside [{}, {}]", day.0, year_start.0 + 1, year_start.0 + 359
            );
        }
    }

    // ── schedule_attritional_losses_for_insured tests ────────────────────────

    /// Scheduler emits AssetDamage events with ground_up_loss ≤ sum_insured.
    #[test]
    fn attritional_produces_bounded_asset_damages() {
        let mut rng = rng();
        let risk = small_risk();
        let events = schedule_attritional_losses_for_insured(
            InsuredId(1),
            &risk,
            Day::year_start(Year(1)),
            &mut rng,
            &att_config(),
        );
        assert!(!events.is_empty(), "expected events with rate=10.0");
        for (_, e) in &events {
            if let Event::AssetDamage { insured_id, ground_up_loss, peril } = e {
                assert_eq!(*insured_id, InsuredId(1));
                assert_eq!(*peril, Peril::Attritional);
                assert!(
                    *ground_up_loss <= ASSET_VALUE,
                    "gul {ground_up_loss} > sum_insured {ASSET_VALUE}"
                );
            }
        }
    }

    /// Returns empty when risk does not cover Attritional.
    #[test]
    fn attritional_skips_non_attritional_risk() {
        let mut rng = rng();
        let risk = Risk {
            sum_insured: ASSET_VALUE,
            territory: "US-SE".to_string(),
            perils_covered: vec![Peril::WindstormAtlantic], // no Attritional
        };
        let events = schedule_attritional_losses_for_insured(
            InsuredId(1),
            &risk,
            Day::year_start(Year(1)),
            &mut rng,
            &att_config(),
        );
        assert!(events.is_empty(), "must return no events when Attritional not covered");
    }

    /// Scheduler emits only AssetDamage events (never ClaimSettled).
    #[test]
    fn attritional_emits_only_asset_damage() {
        let mut rng = rng();
        let events = schedule_attritional_losses_for_insured(
            InsuredId(1),
            &small_risk(),
            Day::year_start(Year(1)),
            &mut rng,
            &att_config(),
        );
        assert!(!events.is_empty());
        for (_, e) in &events {
            assert!(
                matches!(e, Event::AssetDamage { .. }),
                "must only emit AssetDamage, got {e:?}"
            );
        }
    }

    // ── Pareto severity range tests ───────────────────────────────────────────

    /// Canonical Pareto(scale=0.04): all samples must be ≥ scale (the distribution
    /// minimum). The rand_distr Pareto is parameterised over [scale, ∞).
    #[test]
    fn pareto_damage_fraction_never_below_scale() {
        let scale = 0.04;
        let model = DamageFractionModel::Pareto { scale, shape: 2.5, cap: 0.50 };
        let mut rng = rng();
        for _ in 0..10_000 {
            let v = model.sample(&mut rng);
            assert!(v >= scale, "Pareto sample {v} is below scale {scale}");
        }
    }

    /// Canonical Pareto with cap=0.50: all samples must be ≤ cap after truncation.
    #[test]
    fn pareto_damage_fraction_never_above_cap() {
        let cap = 0.50;
        let model = DamageFractionModel::Pareto { scale: 0.04, shape: 2.5, cap };
        let mut rng = rng();
        for _ in 0..10_000 {
            let v = model.sample(&mut rng);
            assert!(v <= cap, "Pareto sample {v} exceeds cap {cap}");
        }
    }

    // ── Multi-territory scheduling tests ──────────────────────────────────────

    /// With 3 territories in config, every scheduled LossEvent must target a
    /// territory that appears in the config list — never an unknown value.
    #[test]
    fn schedule_loss_events_assigns_known_territories() {
        let territories =
            vec!["US-NE".to_string(), "US-SE".to_string(), "US-Gulf".to_string()];
        let cfg = CatConfig {
            annual_frequency: 20.0,
            pareto_scale: 0.04,
            pareto_shape: 2.5,
            max_damage_fraction: 0.50,
            territories: territories.clone(),
        };
        let mut rng = rng();
        let mut next_id = 0u64;
        for y in 1..=20u32 {
            for (_, e) in schedule_loss_events(&cfg, Year(y), &mut rng, &mut next_id) {
                if let Event::LossEvent { territory, .. } = e {
                    assert!(
                        territories.contains(&territory),
                        "event territory '{territory}' not in config list"
                    );
                }
            }
        }
    }

    /// With λ=20 and 3 territories over 20 years (~400 total events), each territory
    /// must receive a statistically significant share. Requires ≥ 50 events each
    /// (expected ≈133). Probability of failing on seed=42 is negligible.
    #[test]
    fn schedule_loss_events_covers_all_territories() {
        use std::collections::HashMap;
        let territories =
            vec!["US-NE".to_string(), "US-SE".to_string(), "US-Gulf".to_string()];
        let cfg = CatConfig {
            annual_frequency: 20.0,
            pareto_scale: 0.04,
            pareto_shape: 2.5,
            max_damage_fraction: 0.50,
            territories: territories.clone(),
        };
        let mut rng = rng();
        let mut next_id = 0u64;
        let mut counts: HashMap<String, usize> = HashMap::new();
        for y in 1..=20u32 {
            for (_, e) in schedule_loss_events(&cfg, Year(y), &mut rng, &mut next_id) {
                if let Event::LossEvent { territory, .. } = e {
                    *counts.entry(territory).or_insert(0) += 1;
                }
            }
        }
        for t in &territories {
            let n = counts.get(t).copied().unwrap_or(0);
            assert!(n >= 50, "territory '{t}' received only {n} events (expected ≈133)");
        }
    }

    /// Pareto(scale=1.0, shape=2.0) always samples ≥ 1.0, clipped to 1.0
    /// → ground_up_loss must equal sum_insured.
    #[test]
    fn full_damage_fraction_gives_sum_insured() {
        // Use a high mu that forces damage_fraction → 1.0 after clipping.
        let config = AttritionalConfig { annual_rate: 5.0, mu: 10.0, sigma: 0.01 };
        let mut rng = rng();
        let risk = small_risk();
        let events = schedule_attritional_losses_for_insured(
            InsuredId(1),
            &risk,
            Day::year_start(Year(1)),
            &mut rng,
            &config,
        );
        assert!(!events.is_empty());
        for (_, e) in &events {
            if let Event::AssetDamage { ground_up_loss, .. } = e {
                assert_eq!(
                    *ground_up_loss, ASSET_VALUE,
                    "with damage_fraction=1.0, gul must equal sum_insured"
                );
            }
        }
    }
}
