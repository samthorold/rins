use rand::Rng;
use rand_distr::{Distribution, LogNormal, Pareto, Poisson};

use crate::events::{Event, Peril, Risk};
use crate::types::{Day, InsuredId, LossEventId, PolicyId, Year};

/// A damage fraction model: `sample()` returns a value in `[0.0, 1.0]`
/// representing the fraction of the insured asset's total value that is damaged.
/// Raw distribution outputs are clipped to 1.0.
#[allow(dead_code)]
pub enum DamageFractionModel {
    /// Log-normal damage fraction; ln-space params.
    /// E[X] = exp(mu + sigma²/2), clipped to 1.0.
    LogNormal { mu: f64, sigma: f64 },
    /// Pareto damage fraction: `scale` = minimum value (should be < 1.0), `shape` = tail index α.
    /// E[X] = scale * shape / (shape − 1)  (requires shape > 1), clipped to 1.0.
    Pareto { scale: f64, shape: f64 },
}

impl DamageFractionModel {
    /// Sample a damage fraction in `[0.0, 1.0]`.
    pub fn sample(&self, rng: &mut impl Rng) -> f64 {
        match self {
            DamageFractionModel::LogNormal { mu, sigma } => {
                let dist = LogNormal::new(*mu, *sigma).expect("invalid LogNormal params");
                dist.sample(rng).min(1.0_f64)
            }
            DamageFractionModel::Pareto { scale, shape } => {
                let dist = Pareto::new(*scale, *shape).expect("invalid Pareto params");
                dist.sample(rng).min(1.0_f64)
            }
        }
    }
}

pub struct PerilConfig {
    pub peril: Peril,
    pub region: &'static str,
    /// Poisson λ: expected number of events per year.
    pub annual_frequency: f64,
    /// Damage fraction model: `sample()` → fraction of sum_insured damaged.
    /// PLACEHOLDER calibration — tune E[damage_fraction × sum_insured] against
    /// target per-policy loss levels.
    pub damage_fraction: DamageFractionModel,
}

/// Default per-peril configurations.
/// All damage fraction parameters are PLACEHOLDER calibration — tune against
/// empirical industry loss data and calibration benchmarks.
///
/// Target: E[damage_fraction × sum_insured] ≈ former per-policy expected loss.
/// Former scales (Pareto pence severity) → approximate E per policy listed below.
pub fn default_peril_configs() -> Vec<PerilConfig> {
    vec![
        // ── Catastrophe perils ────────────────────────────────────────────────
        // PLACEHOLDER: LogNormal damage fractions targeting moderate-to-severe
        // per-policy losses for large property risks.
        // E[df] ≈ 0.29 for WindstormAtlantic; sum_insured≈£100M → E[loss]≈£29M
        PerilConfig {
            peril: Peril::WindstormAtlantic,
            region: "US-SE",
            annual_frequency: 0.5, // PLACEHOLDER
            damage_fraction: DamageFractionModel::LogNormal { mu: -1.5, sigma: 1.0 }, // PLACEHOLDER
        },
        // E[df] ≈ 0.17; sum_insured≈£10M → E[loss]≈£1.7M
        PerilConfig {
            peril: Peril::WindstormEuropean,
            region: "EU",
            annual_frequency: 0.8, // PLACEHOLDER
            damage_fraction: DamageFractionModel::LogNormal { mu: -2.0, sigma: 1.0 }, // PLACEHOLDER
        },
        // E[df] ≈ 0.47; sum_insured≈£30M → E[loss]≈£14M
        PerilConfig {
            peril: Peril::EarthquakeUS,
            region: "US-CA",
            annual_frequency: 0.2, // PLACEHOLDER
            damage_fraction: DamageFractionModel::LogNormal { mu: -1.0, sigma: 1.0 }, // PLACEHOLDER
        },
        // E[df] ≈ 0.38; sum_insured≈£30M → E[loss]≈£11M
        PerilConfig {
            peril: Peril::EarthquakeJapan,
            region: "JP",
            annual_frequency: 0.3, // PLACEHOLDER
            damage_fraction: DamageFractionModel::LogNormal { mu: -1.2, sigma: 1.0 }, // PLACEHOLDER
        },
        // E[df] ≈ 0.08; sum_insured≈£10M → E[loss]≈£0.8M
        PerilConfig {
            peril: Peril::Flood,
            region: "EU",
            annual_frequency: 1.5, // PLACEHOLDER
            damage_fraction: DamageFractionModel::LogNormal { mu: -2.5, sigma: 1.0 }, // PLACEHOLDER
        },
        PerilConfig {
            peril: Peril::Flood,
            region: "US-SE",
            annual_frequency: 1.5, // PLACEHOLDER
            damage_fraction: DamageFractionModel::LogNormal { mu: -2.5, sigma: 1.0 }, // PLACEHOLDER
        },
    ]
}

/// Per-territory attritional claim configuration for per-policy Poisson scheduling.
pub struct AttritionalConfig {
    /// Expected number of claims per policy per year.
    pub annual_rate: f64,
    /// Per-occurrence damage fraction distribution: `sample()` → fraction of sum_insured.
    /// PLACEHOLDER calibration — tune against desired attritional LR targets.
    pub damage_fraction: DamageFractionModel,
}

/// Per-territory attritional claim rates and damage fractions.
///
/// mu=-3.0, sigma=1.0 → E[df] ≈ 8.2% (exp(-2.5)), median ≈ 5%.
/// For uk_property (SI=£5M, attachment=£200K): E[GUL] ≈ £410K — above attachment.
/// For eu_property (SI=£10M, attachment=£500K): E[GUL] ≈ £820K — above attachment.
/// Target: attritional ≥ 50% of GUL in benign (low cat-frequency) years.
pub fn default_attritional_configs() -> std::collections::HashMap<&'static str, AttritionalConfig> {
    [
        ("UK",    AttritionalConfig { annual_rate: 3.0, damage_fraction: DamageFractionModel::LogNormal { mu: -3.0, sigma: 1.0 } }),
        ("EU",    AttritionalConfig { annual_rate: 3.0, damage_fraction: DamageFractionModel::LogNormal { mu: -3.0, sigma: 1.0 } }),
        ("US-SE", AttritionalConfig { annual_rate: 4.0, damage_fraction: DamageFractionModel::LogNormal { mu: -3.0, sigma: 1.0 } }),
        ("US-CA", AttritionalConfig { annual_rate: 4.0, damage_fraction: DamageFractionModel::LogNormal { mu: -3.0, sigma: 1.0 } }),
    ]
    .into_iter()
    .collect()
}

/// Generate attritional `InsuredLoss` events for one newly bound policy.
#[allow(dead_code)]
///
/// Called at `PolicyBound` time. Draws a Poisson count from the territory's
/// `annual_rate`, then for each occurrence draws a random day and damage fraction.
/// Each occurrence emits one `InsuredLoss { ground_up_loss = damage_fraction × sum_insured }`.
/// Claims in unknown territories produce no events.
pub fn schedule_attritional_claims_for_policy(
    policy_id: PolicyId,
    insured_id: InsuredId,
    risk: &Risk,
    year: Year,
    rng: &mut impl Rng,
    configs: &std::collections::HashMap<&'static str, AttritionalConfig>,
) -> Vec<(Day, Event)> {
    if !risk.perils_covered.contains(&Peril::Attritional) {
        return vec![];
    }
    let Some(config) = configs.get(risk.territory.as_str()) else {
        return vec![];
    };
    let year_start = Day::year_start(year);
    let poisson = Poisson::new(config.annual_rate).expect("invalid Poisson rate");
    let n = poisson.sample(rng) as u64;
    let mut out = Vec::new();
    for _ in 0..n {
        let day = year_start.offset(rng.random_range(1_u64..360));
        let damage_fraction = config.damage_fraction.sample(rng);
        // ground_up_loss is naturally capped at sum_insured by damage_fraction ≤ 1.0.
        let ground_up_loss = (damage_fraction * risk.sum_insured as f64) as u64;
        if ground_up_loss == 0 {
            continue;
        }
        out.push((
            day,
            Event::InsuredLoss {
                policy_id: Some(policy_id),
                insured_id,
                peril: Peril::Attritional,
                ground_up_loss,
            },
        ));
    }
    out
}

/// Generate attritional `InsuredLoss` events for one insured's asset, independent of
/// whether a policy is bound.
///
/// Called at `SimulationStart` for ALL insured assets. Emits
/// `InsuredLoss { policy_id: None, ... }`; the dispatcher looks up
/// `insured_active_policies` at fire time to decide whether a ClaimSettled follows.
/// Claims in unknown territories produce no events.
pub fn schedule_attritional_claims_for_insured(
    insured_id: InsuredId,
    risk: &Risk,
    year: Year,
    rng: &mut impl Rng,
    configs: &std::collections::HashMap<&'static str, AttritionalConfig>,
) -> Vec<(Day, Event)> {
    if !risk.perils_covered.contains(&Peril::Attritional) {
        return vec![];
    }
    let Some(config) = configs.get(risk.territory.as_str()) else {
        return vec![];
    };
    let year_start = Day::year_start(year);
    let poisson = Poisson::new(config.annual_rate).expect("invalid Poisson rate");
    let n = poisson.sample(rng) as u64;
    let mut out = Vec::new();
    for _ in 0..n {
        let day = year_start.offset(rng.random_range(1_u64..360));
        let damage_fraction = config.damage_fraction.sample(rng);
        let ground_up_loss = (damage_fraction * risk.sum_insured as f64) as u64;
        if ground_up_loss == 0 {
            continue;
        }
        out.push((
            day,
            Event::InsuredLoss {
                policy_id: None,
                insured_id,
                peril: Peril::Attritional,
                ground_up_loss,
            },
        ));
    }
    out
}

/// Schedule `LossEvent`s for `year` across all `configs`.
///
/// Each config runs an independent Poisson process: draw a count from
/// Poisson(λ), then for each event draw a uniform day-offset within the
/// year. `LossEvent` no longer carries a severity; per-policy damage is
/// sampled in `Market::on_loss_event` when the event fires.
///
/// `next_id` is mutated in-place; the caller owns `Simulation.next_loss_event_id`.
pub fn schedule_loss_events(
    configs: &[PerilConfig],
    year: Year,
    rng: &mut impl Rng,
    next_id: &mut u64,
) -> Vec<(Day, Event)> {
    let year_start = Day::year_start(year);
    let mut out = Vec::new();

    for config in configs {
        if config.annual_frequency <= 0.0 {
            continue;
        }
        let poisson = Poisson::new(config.annual_frequency).expect("invalid Poisson lambda");
        let n = poisson.sample(rng) as u64;
        for _ in 0..n {
            let offset = rng.random_range(1_u64..360);
            let event_id = LossEventId(*next_id);
            *next_id += 1;
            out.push((
                year_start.offset(offset),
                Event::LossEvent {
                    event_id,
                    region: config.region.to_string(),
                    peril: config.peril,
                },
            ));
        }
    }

    out
}

#[cfg(test)]
mod tests {
    use rand::SeedableRng;
    use rand_chacha::ChaCha20Rng;

    use super::*;
    use crate::events::Peril;
    use crate::types::{Day, Year};

    fn rng() -> ChaCha20Rng {
        ChaCha20Rng::seed_from_u64(42)
    }

    /// DamageFractionModel::LogNormal with mu=-2.0, sigma=1.0:
    /// E[X] (unclipped) = exp(-2.0 + 0.5) = exp(-1.5) ≈ 0.223.
    /// 10k samples must have mean within ±30% of unclipped expectation
    /// (clipping at 1.0 negligibly affects a distribution centred at 0.22).
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
        let model = DamageFractionModel::LogNormal { mu: 1.0, sigma: 2.0 }; // many raw samples > 1
        let mut rng = rng();
        for _ in 0..1_000 {
            let v = model.sample(&mut rng);
            assert!(
                v >= 0.0 && v <= 1.0,
                "sample {v} outside [0, 1]"
            );
        }
    }

    /// Pareto tail is heavier than LogNormal with similar median (both fractions).
    /// Pareto(scale=0.01, shape=1.5) vs LogNormal(mu=ln(0.01), sigma=0.5):
    /// 99th percentile of Pareto (before clipping) should exceed LogNormal's.
    #[test]
    fn damage_fraction_pareto_tail_heavier_than_lognormal() {
        let _pareto = DamageFractionModel::Pareto { scale: 0.01, shape: 1.5 };
        let _lognorm = DamageFractionModel::LogNormal { mu: (0.01_f64).ln(), sigma: 0.5 };

        let mut rng = rng();
        let n = 10_000usize;

        // Sample raw values via the underlying distributions directly for tail comparison.
        let sample_raw_pareto = |rng: &mut ChaCha20Rng| -> f64 {
            Pareto::new(0.01_f64, 1.5_f64).unwrap().sample(rng)
        };
        let sample_raw_lognorm = |rng: &mut ChaCha20Rng| -> f64 {
            LogNormal::new((0.01_f64).ln(), 0.5_f64).unwrap().sample(rng)
        };

        let mut pareto_samples: Vec<f64> = (0..n).map(|_| sample_raw_pareto(&mut rng)).collect();
        let mut lognorm_samples: Vec<f64> = (0..n).map(|_| sample_raw_lognorm(&mut rng)).collect();

        pareto_samples.sort_unstable_by(f64::total_cmp);
        lognorm_samples.sort_unstable_by(f64::total_cmp);

        let p99_pareto = pareto_samples[n * 99 / 100];
        let p99_lognorm = lognorm_samples[n * 99 / 100];

        assert!(
            p99_pareto > p99_lognorm,
            "Pareto 99th pct {p99_pareto:.4} should exceed LogNormal 99th pct {p99_lognorm:.4}"
        );
    }

    /// Single-config schedule: every LossEvent must carry the config's peril and region.
    #[test]
    fn schedule_returns_correct_peril_region_pairs() {
        let configs = vec![PerilConfig {
            peril: Peril::EarthquakeUS,
            region: "US-CA",
            annual_frequency: 5.0,
            damage_fraction: DamageFractionModel::Pareto { scale: 0.01, shape: 1.5 },
        }];
        let mut rng = rng();
        let mut next_id = 0u64;
        let events = schedule_loss_events(&configs, Year(1), &mut rng, &mut next_id);

        for (_, e) in &events {
            match e {
                Event::LossEvent { peril, region, .. } => {
                    assert_eq!(*peril, Peril::EarthquakeUS);
                    assert_eq!(region, "US-CA");
                }
                _ => panic!("unexpected event type"),
            }
        }
    }

    /// With λ=2.0 over 100 years the mean annual event count must lie in [1.5, 2.5].
    #[test]
    fn poisson_count_is_reasonable() {
        let configs = vec![PerilConfig {
            peril: Peril::WindstormAtlantic,
            region: "US-SE",
            annual_frequency: 2.0,
            damage_fraction: DamageFractionModel::Pareto { scale: 0.01, shape: 1.5 },
        }];
        let mut rng = rng();
        let years = 100u32;
        let mut total = 0usize;
        let mut next_id = 0u64;
        for y in 1..=years {
            let events = schedule_loss_events(&configs, Year(y), &mut rng, &mut next_id);
            total += events.len();
        }
        let mean = total as f64 / years as f64;
        assert!(
            mean >= 1.5 && mean <= 2.5,
            "mean annual count {mean:.2} outside [1.5, 2.5]"
        );
    }

    /// All LossEventIds across 3 years must be unique.
    #[test]
    fn loss_event_ids_are_unique() {
        use std::collections::HashSet;
        let configs = default_peril_configs();
        let mut rng = rng();
        let mut next_id = 0u64;
        let mut seen = HashSet::new();
        for y in 1..=3u32 {
            let events = schedule_loss_events(&configs, Year(y), &mut rng, &mut next_id);
            for (_, e) in events {
                if let Event::LossEvent { event_id, .. } = e {
                    assert!(seen.insert(event_id.0), "duplicate LossEventId {}", event_id.0);
                }
            }
        }
    }

    /// All scheduled days must lie within [year_start+1, year_start+359].
    #[test]
    fn scheduled_days_within_year() {
        let configs = vec![PerilConfig {
            peril: Peril::Flood,
            region: "EU",
            annual_frequency: 10.0,
            damage_fraction: DamageFractionModel::LogNormal { mu: -2.5, sigma: 1.0 },
        }];
        let mut rng = rng();
        let mut next_id = 0u64;
        let year = Year(3);
        let year_start = Day::year_start(year);
        let events = schedule_loss_events(&configs, year, &mut rng, &mut next_id);

        assert!(!events.is_empty(), "expected at least one event with λ=10");
        for (day, _) in &events {
            assert!(
                day.0 > year_start.0 && day.0 <= year_start.0 + 359,
                "day {} outside [{}, {}]",
                day.0,
                year_start.0 + 1,
                year_start.0 + 359
            );
        }
    }

    /// Per-policy attritional scheduler must emit `InsuredLoss` events with
    /// `ground_up_loss ≤ sum_insured`.
    #[test]
    fn schedule_attritional_claims_for_policy_produces_bounded_insured_losses() {
        use std::collections::HashMap;

        use crate::events::{Risk};
        use crate::types::{InsuredId, PolicyId};

        let mut rng = rng();
        let risk = Risk {
            line_of_business: "property".to_string(),
            sum_insured: 2_000_000_000,
            territory: "UK".to_string(),
            limit: 200_000_000,
            attachment: 0,
            perils_covered: vec![Peril::Attritional],
        };
        // Use a very high rate so we reliably get claims in a single test run.
        let configs: HashMap<&'static str, AttritionalConfig> = [(
            "UK",
            AttritionalConfig {
                annual_rate: 50.0,
                damage_fraction: DamageFractionModel::LogNormal { mu: -4.0, sigma: 1.0 },
            },
        )]
        .into_iter()
        .collect();

        let events = schedule_attritional_claims_for_policy(
            PolicyId(0),
            InsuredId(1),
            &risk,
            Year(1),
            &mut rng,
            &configs,
        );

        assert!(!events.is_empty(), "expected InsuredLoss events with rate=50.0");

        for (_, e) in &events {
            match e {
                Event::InsuredLoss { ground_up_loss, policy_id, insured_id, peril, .. } => {
                    assert_eq!(*policy_id, Some(PolicyId(0)));
                    assert_eq!(*insured_id, InsuredId(1));
                    assert_eq!(*peril, Peril::Attritional);
                    assert!(
                        *ground_up_loss <= risk.sum_insured,
                        "ground_up_loss {ground_up_loss} exceeds sum_insured {}",
                        risk.sum_insured
                    );
                }
                _ => panic!("unexpected event type {e:?}"),
            }
        }
    }

    /// Attritional scheduler returns empty vec when policy does not cover Attritional.
    #[test]
    fn schedule_attritional_no_events_for_non_attritional_policy() {
        use std::collections::HashMap;

        use crate::events::{Risk};
        use crate::types::{InsuredId, PolicyId};

        let mut rng = rng();
        let risk = Risk {
            line_of_business: "property".to_string(),
            sum_insured: 2_000_000_000,
            territory: "UK".to_string(),
            limit: 200_000_000,
            attachment: 0,
            perils_covered: vec![Peril::WindstormAtlantic], // not Attritional
        };
        let configs: HashMap<&'static str, AttritionalConfig> = [(
            "UK",
            AttritionalConfig {
                annual_rate: 50.0,
                damage_fraction: DamageFractionModel::LogNormal { mu: -4.0, sigma: 1.0 },
            },
        )]
        .into_iter()
        .collect();

        let events = schedule_attritional_claims_for_policy(
            PolicyId(0),
            InsuredId(1),
            &risk,
            Year(1),
            &mut rng,
            &configs,
        );

        assert!(events.is_empty(), "non-Attritional policy must produce no attritional claims");
    }

    /// Attritional scheduler with damage_fraction=1.0 must produce
    /// ground_up_loss == sum_insured exactly.
    #[test]
    fn attritional_damage_fraction_one_gives_sum_insured() {
        use std::collections::HashMap;

        use crate::events::Risk;
        use crate::types::{InsuredId, PolicyId};

        // Force damage_fraction=1.0 by using a constant LogNormal(mu=1e6, sigma=0)
        // which is invalid... instead use a very high mu so every sample rounds to 1.0.
        // Actually use Pareto with scale=1.0 which always returns exactly 1.0 when clipped.
        // Pareto(scale=1.0, shape=2.0) always produces values ≥ 1.0, so clipped to 1.0.
        let configs: HashMap<&'static str, AttritionalConfig> = [(
            "UK",
            AttritionalConfig {
                annual_rate: 10.0,
                damage_fraction: DamageFractionModel::Pareto { scale: 1.0, shape: 2.0 },
            },
        )]
        .into_iter()
        .collect();

        let risk = Risk {
            line_of_business: "property".to_string(),
            sum_insured: 500_000_000,
            territory: "UK".to_string(),
            limit: 200_000_000,
            attachment: 0,
            perils_covered: vec![Peril::Attritional],
        };

        let mut rng = rng();
        let events = schedule_attritional_claims_for_policy(
            PolicyId(0),
            InsuredId(1),
            &risk,
            Year(1),
            &mut rng,
            &configs,
        );

        assert!(!events.is_empty());
        for (_, e) in &events {
            if let Event::InsuredLoss { ground_up_loss, .. } = e {
                assert_eq!(
                    *ground_up_loss, risk.sum_insured,
                    "damage_fraction=1.0 must yield ground_up_loss == sum_insured"
                );
            }
        }
    }

    /// Attritional InsuredLoss events come before ClaimSettled in a real simulation
    /// — verified at the integration level in simulation.rs tests.
    /// Here we verify the scheduler only emits InsuredLoss (not ClaimSettled).
    #[test]
    fn attritional_emits_only_insured_loss_not_claim_settled() {
        use std::collections::HashMap;

        use crate::events::Risk;
        use crate::types::{InsuredId, PolicyId};

        let configs: HashMap<&'static str, AttritionalConfig> = [(
            "UK",
            AttritionalConfig {
                annual_rate: 20.0,
                damage_fraction: DamageFractionModel::LogNormal { mu: -4.0, sigma: 1.0 },
            },
        )]
        .into_iter()
        .collect();

        let risk = Risk {
            line_of_business: "property".to_string(),
            sum_insured: 2_000_000_000,
            territory: "UK".to_string(),
            limit: 200_000_000,
            attachment: 0,
            perils_covered: vec![Peril::Attritional],
        };

        let mut rng = rng();
        let events = schedule_attritional_claims_for_policy(
            PolicyId(0),
            InsuredId(1),
            &risk,
            Year(1),
            &mut rng,
            &configs,
        );

        assert!(!events.is_empty(), "expected events with rate=20.0");
        for (_, e) in &events {
            assert!(
                matches!(e, Event::InsuredLoss { .. }),
                "scheduler must only emit InsuredLoss, got {e:?}"
            );
        }
    }

    // ── schedule_attritional_claims_for_insured tests ─────────────────────────

    /// Per-insured attritional scheduler must emit `InsuredLoss { policy_id: None }` with
    /// `ground_up_loss ≤ sum_insured`.
    #[test]
    fn schedule_attritional_claims_for_insured_produces_bounded_insured_losses() {
        use std::collections::HashMap;

        use crate::events::Risk;
        use crate::types::InsuredId;

        let mut rng = rng();
        let risk = Risk {
            line_of_business: "property".to_string(),
            sum_insured: 2_000_000_000,
            territory: "UK".to_string(),
            limit: 200_000_000,
            attachment: 0,
            perils_covered: vec![Peril::Attritional],
        };
        let configs: HashMap<&'static str, AttritionalConfig> = [(
            "UK",
            AttritionalConfig {
                annual_rate: 50.0,
                damage_fraction: DamageFractionModel::LogNormal { mu: -4.0, sigma: 1.0 },
            },
        )]
        .into_iter()
        .collect();

        let events = schedule_attritional_claims_for_insured(
            InsuredId(7),
            &risk,
            Year(1),
            &mut rng,
            &configs,
        );

        assert!(!events.is_empty(), "expected InsuredLoss events with rate=50.0");

        for (_, e) in &events {
            match e {
                Event::InsuredLoss { policy_id, insured_id, peril, ground_up_loss } => {
                    assert_eq!(*policy_id, None, "per-insured scheduler must emit policy_id: None");
                    assert_eq!(*insured_id, InsuredId(7));
                    assert_eq!(*peril, Peril::Attritional);
                    assert!(
                        *ground_up_loss <= risk.sum_insured,
                        "ground_up_loss {ground_up_loss} exceeds sum_insured {}",
                        risk.sum_insured
                    );
                }
                _ => panic!("unexpected event type {e:?}"),
            }
        }
    }

    /// Attritional-insured scheduler returns empty vec when risk does not cover Attritional.
    #[test]
    fn schedule_attritional_claims_for_insured_no_events_for_non_attritional_risk() {
        use std::collections::HashMap;

        use crate::events::Risk;
        use crate::types::InsuredId;

        let mut rng = rng();
        let risk = Risk {
            line_of_business: "property".to_string(),
            sum_insured: 2_000_000_000,
            territory: "UK".to_string(),
            limit: 200_000_000,
            attachment: 0,
            perils_covered: vec![Peril::WindstormAtlantic], // not Attritional
        };
        let configs: HashMap<&'static str, AttritionalConfig> = [(
            "UK",
            AttritionalConfig {
                annual_rate: 50.0,
                damage_fraction: DamageFractionModel::LogNormal { mu: -4.0, sigma: 1.0 },
            },
        )]
        .into_iter()
        .collect();

        let events = schedule_attritional_claims_for_insured(
            InsuredId(1),
            &risk,
            Year(1),
            &mut rng,
            &configs,
        );

        assert!(events.is_empty(), "non-Attritional risk must produce no events");
    }

    /// Attritional-insured scheduler returns empty vec for unknown territory.
    #[test]
    fn schedule_attritional_claims_for_insured_unknown_territory_produces_no_events() {
        use std::collections::HashMap;

        use crate::events::Risk;
        use crate::types::InsuredId;

        let mut rng = rng();
        let risk = Risk {
            line_of_business: "property".to_string(),
            sum_insured: 1_000_000,
            territory: "JP".to_string(), // not in configs
            limit: 500_000,
            attachment: 0,
            perils_covered: vec![Peril::Attritional],
        };
        let configs: HashMap<&'static str, AttritionalConfig> = [(
            "UK",
            AttritionalConfig {
                annual_rate: 50.0,
                damage_fraction: DamageFractionModel::LogNormal { mu: -4.0, sigma: 1.0 },
            },
        )]
        .into_iter()
        .collect();

        let events = schedule_attritional_claims_for_insured(
            InsuredId(1),
            &risk,
            Year(1),
            &mut rng,
            &configs,
        );

        assert!(events.is_empty(), "unknown territory must produce no events");
    }
}
