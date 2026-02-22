use rand::Rng;
use rand_distr::{Distribution, LogNormal, Pareto, Poisson};

use crate::config::{AttritionalConfig, CatConfig};
use crate::events::{Event, Peril, Risk};
use crate::types::{Day, InsuredId, PolicyId, Year};

/// A damage fraction model: `sample()` returns a value in `[0.0, 1.0]`
/// representing the fraction of the insured asset's total value that is damaged.
/// Raw distribution outputs are clipped to 1.0.
pub enum DamageFractionModel {
    /// Log-normal damage fraction; ln-space params.
    /// E[X] = exp(mu + sigma²/2), clipped to 1.0.
    LogNormal { mu: f64, sigma: f64 },
    /// Pareto damage fraction: `scale` = minimum value (< 1.0), `shape` = tail index α.
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
    if cat.annual_frequency <= 0.0 {
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
            (year_start.offset(offset), Event::LossEvent { event_id, peril: Peril::WindstormAtlantic })
        })
        .collect()
}

/// Schedule attritional `InsuredLoss` events for a single bound policy.
///
/// Called at `PolicyBound` time — policy_id is known and non-optional.
/// `from_day` is the PolicyBound day; all losses are scheduled strictly after
/// it so the DES log remains day-ordered (no event is scheduled in the past).
/// Draws a Poisson count from `config.annual_rate`, then for each occurrence
/// draws a random day in `(from_day, year_end]` and a damage fraction.
pub fn schedule_attritional_claims_for_policy(
    policy_id: PolicyId,
    insured_id: InsuredId,
    risk: &Risk,
    from_day: Day,
    rng: &mut impl Rng,
    config: &AttritionalConfig,
) -> Vec<(Day, Event)> {
    if !risk.perils_covered.contains(&Peril::Attritional) {
        return vec![];
    }
    let year_num = from_day.0 / Day::DAYS_PER_YEAR + 1;
    let year_end = Day::year_end(Year(year_num as u32));
    if from_day >= year_end {
        return vec![];
    }
    let model = DamageFractionModel::LogNormal { mu: config.mu, sigma: config.sigma };
    let poisson = Poisson::new(config.annual_rate).expect("invalid Poisson rate");
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
                Event::InsuredLoss { policy_id, insured_id, peril: Peril::Attritional, ground_up_loss },
            ))
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use rand::SeedableRng;
    use rand_chacha::ChaCha20Rng;

    use super::*;
    use crate::config::{AttritionalConfig, CatConfig, SMALL_ASSET_VALUE};
    use crate::types::{Day, InsuredId, PolicyId, Year};

    fn rng() -> ChaCha20Rng {
        ChaCha20Rng::seed_from_u64(42)
    }

    fn att_config() -> AttritionalConfig {
        AttritionalConfig { annual_rate: 10.0, mu: -3.0, sigma: 1.0 }
    }

    fn cat_config() -> CatConfig {
        CatConfig { annual_frequency: 2.0, pareto_scale: 0.05, pareto_shape: 1.5 }
    }

    fn small_risk() -> Risk {
        Risk {
            sum_insured: SMALL_ASSET_VALUE,
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
        let cfg = CatConfig { annual_frequency: 2.0, pareto_scale: 0.05, pareto_shape: 1.5 };
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
        let cfg = CatConfig { annual_frequency: 10.0, pareto_scale: 0.05, pareto_shape: 1.5 };
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

    // ── schedule_attritional_claims_for_policy tests ──────────────────────────

    /// Scheduler emits InsuredLoss events with ground_up_loss ≤ sum_insured.
    #[test]
    fn attritional_produces_bounded_insured_losses() {
        let mut rng = rng();
        let risk = small_risk();
        let events = schedule_attritional_claims_for_policy(
            PolicyId(1),
            InsuredId(1),
            &risk,
            Day::year_start(Year(1)),
            &mut rng,
            &att_config(),
        );
        assert!(!events.is_empty(), "expected events with rate=10.0");
        for (_, e) in &events {
            if let Event::InsuredLoss { policy_id, ground_up_loss, peril, .. } = e {
                assert_eq!(*policy_id, PolicyId(1));
                assert_eq!(*peril, Peril::Attritional);
                assert!(
                    *ground_up_loss <= SMALL_ASSET_VALUE,
                    "gul {ground_up_loss} > sum_insured {SMALL_ASSET_VALUE}"
                );
            }
        }
    }

    /// Returns empty when risk does not cover Attritional.
    #[test]
    fn attritional_skips_non_attritional_risk() {
        let mut rng = rng();
        let risk = Risk {
            sum_insured: SMALL_ASSET_VALUE,
            territory: "US-SE".to_string(),
            perils_covered: vec![Peril::WindstormAtlantic], // no Attritional
        };
        let events = schedule_attritional_claims_for_policy(
            PolicyId(1),
            InsuredId(1),
            &risk,
            Day::year_start(Year(1)),
            &mut rng,
            &att_config(),
        );
        assert!(events.is_empty(), "must return no events when Attritional not covered");
    }

    /// Scheduler emits only InsuredLoss events (never ClaimSettled).
    #[test]
    fn attritional_emits_only_insured_loss() {
        let mut rng = rng();
        let events = schedule_attritional_claims_for_policy(
            PolicyId(0),
            InsuredId(1),
            &small_risk(),
            Day::year_start(Year(1)),
            &mut rng,
            &att_config(),
        );
        assert!(!events.is_empty());
        for (_, e) in &events {
            assert!(
                matches!(e, Event::InsuredLoss { .. }),
                "must only emit InsuredLoss, got {e:?}"
            );
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
        let events = schedule_attritional_claims_for_policy(
            PolicyId(0),
            InsuredId(1),
            &risk,
            Day::year_start(Year(1)),
            &mut rng,
            &config,
        );
        assert!(!events.is_empty());
        for (_, e) in &events {
            if let Event::InsuredLoss { ground_up_loss, .. } = e {
                assert_eq!(
                    *ground_up_loss, SMALL_ASSET_VALUE,
                    "with damage_fraction=1.0, gul must equal sum_insured"
                );
            }
        }
    }
}
