use rand::Rng;
use rand_distr::{Distribution, LogNormal, Pareto, Poisson};

use crate::events::{Event, Panel, Peril, Risk};
use crate::types::{Day, LossEventId, PolicyId, Year};

pub enum SeverityModel {
    /// Log-normal severity; ln-space params.
    /// E[X] = exp(mu + sigma²/2) pence.
    LogNormal { mu: f64, sigma: f64 },
    /// Pareto severity: `scale` = minimum value pence, `shape` = tail index α.
    /// E[X] = scale * shape / (shape − 1)  (requires shape > 1).
    Pareto { scale: f64, shape: f64 },
}

impl SeverityModel {
    pub fn sample(&self, rng: &mut impl Rng) -> u64 {
        match self {
            SeverityModel::LogNormal { mu, sigma } => {
                let dist = LogNormal::new(*mu, *sigma).expect("invalid LogNormal params");
                dist.sample(rng) as u64
            }
            SeverityModel::Pareto { scale, shape } => {
                let dist = Pareto::new(*scale, *shape).expect("invalid Pareto params");
                dist.sample(rng) as u64
            }
        }
    }
}

pub struct PerilConfig {
    pub peril: Peril,
    pub region: &'static str,
    /// Poisson λ: expected number of events per year.
    pub annual_frequency: f64,
    pub severity: SeverityModel,
}

/// Default per-peril configurations.
/// All numeric values are PLACEHOLDER calibration — expect them to change
/// as empirical data and literature review improve the estimates.
pub fn default_peril_configs() -> Vec<PerilConfig> {
    vec![
        // ── Catastrophe perils — Pareto severity, heavy-tailed ──────────────
        // shape=3.0 eliminates infinite-variance artefacts and prevents total wipeouts.
        // E[X] = scale * shape/(shape-1) = scale * 1.5 pence.
        // Scales are 1000× the original placeholder values.
        PerilConfig {
            peril: Peril::WindstormAtlantic,
            region: "US-SE",
            annual_frequency: 0.5, // PLACEHOLDER
            severity: SeverityModel::Pareto { scale: 2_000_000_000.0, shape: 3.0 }, // E[X] ≈ £30M
        },
        PerilConfig {
            peril: Peril::WindstormEuropean,
            region: "EU",
            annual_frequency: 0.8, // PLACEHOLDER
            severity: SeverityModel::Pareto { scale: 1_500_000_000.0, shape: 3.0 }, // E[X] ≈ £22M
        },
        PerilConfig {
            peril: Peril::EarthquakeUS,
            region: "US-CA",
            annual_frequency: 0.2, // PLACEHOLDER
            severity: SeverityModel::Pareto { scale: 5_000_000_000.0, shape: 3.0 }, // E[X] ≈ £75M
        },
        PerilConfig {
            peril: Peril::EarthquakeJapan,
            region: "JP",
            annual_frequency: 0.3, // PLACEHOLDER
            severity: SeverityModel::Pareto { scale: 4_000_000_000.0, shape: 3.0 }, // E[X] ≈ £60M
        },
        PerilConfig {
            peril: Peril::Flood,
            region: "EU",
            annual_frequency: 1.5, // PLACEHOLDER
            severity: SeverityModel::Pareto { scale: 800_000_000.0, shape: 3.0 }, // E[X] ≈ £12M
        },
        PerilConfig {
            peril: Peril::Flood,
            region: "US-SE",
            annual_frequency: 1.5, // PLACEHOLDER
            severity: SeverityModel::Pareto { scale: 800_000_000.0, shape: 3.0 }, // E[X] ≈ £12M
        },
    ]
}

/// Per-territory attritional claim configuration for per-policy Poisson scheduling.
pub struct AttritionalConfig {
    /// Expected number of claims per policy per year.
    pub annual_rate: f64,
    /// Per-occurrence severity distribution.
    pub severity: SeverityModel,
}

/// Per-territory attritional claim rates and severities.
/// These are PLACEHOLDER calibration values — tune against desired LR targets.
///
/// mu=15.5, sigma=1.0 → E[X] ≈ exp(16.0) ≈ £89K per claim.
/// With rate=3.0 and uk_property attachment=£200K, penetration is low;
/// attritional LR ~7%. US rates are higher given frequency exposure.
pub fn default_attritional_configs() -> std::collections::HashMap<&'static str, AttritionalConfig> {
    [
        ("UK",    AttritionalConfig { annual_rate: 3.0, severity: SeverityModel::LogNormal { mu: 15.5, sigma: 1.0 } }),
        ("EU",    AttritionalConfig { annual_rate: 3.0, severity: SeverityModel::LogNormal { mu: 15.5, sigma: 1.0 } }),
        ("US-SE", AttritionalConfig { annual_rate: 4.0, severity: SeverityModel::LogNormal { mu: 16.0, sigma: 1.0 } }),
        ("US-CA", AttritionalConfig { annual_rate: 4.0, severity: SeverityModel::LogNormal { mu: 16.0, sigma: 1.0 } }),
    ]
    .into_iter()
    .collect()
}

/// Generate attritional `ClaimSettled` events for one newly bound policy.
///
/// Called at `PolicyBound` time. Draws a Poisson count from the territory's
/// `annual_rate`, then for each claim draws a random day and severity. Claims
/// below attachment or in unknown territories produce no events.
pub fn schedule_attritional_claims_for_policy(
    policy_id: PolicyId,
    risk: &Risk,
    panel: &Panel,
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
        let severity = config.severity.sample(rng);
        let gross = severity.min(risk.limit);
        let net = gross.saturating_sub(risk.attachment);
        if net == 0 {
            continue;
        }
        for entry in &panel.entries {
            let amount = net * entry.share_bps as u64 / 10_000;
            if amount == 0 {
                continue;
            }
            out.push((
                day,
                Event::ClaimSettled {
                    policy_id,
                    syndicate_id: entry.syndicate_id,
                    amount,
                },
            ));
        }
    }
    out
}

/// Schedule `LossEvent`s for `year` across all `configs`.
///
/// Each config runs an independent Poisson process: draw a count from
/// Poisson(λ), then for each event draw a uniform day-offset within the
/// year and a severity from the config's distribution.
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
            let severity = config.severity.sample(rng);
            let event_id = LossEventId(*next_id);
            *next_id += 1;
            out.push((
                year_start.offset(offset),
                Event::LossEvent {
                    event_id,
                    region: config.region.to_string(),
                    peril: config.peril,
                    severity,
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

    /// LogNormal(mu=11.5, sigma=1.2): E[X] = exp(11.5 + 1.44/2) ≈ 165_000.
    /// 10k samples must land within ±20 % of that.
    #[test]
    fn severity_lognormal_mean_in_expected_range() {
        let model = SeverityModel::LogNormal { mu: 11.5, sigma: 1.2 };
        let mut rng = rng();
        let n = 10_000;
        let mean: f64 = (0..n).map(|_| model.sample(&mut rng) as f64).sum::<f64>() / n as f64;
        let expected = (11.5_f64 + 1.2_f64 * 1.2 / 2.0).exp(); // ≈ 165_000
        let lo = expected * 0.80;
        let hi = expected * 1.20;
        assert!(
            mean >= lo && mean <= hi,
            "LogNormal mean {mean:.0} outside [{lo:.0}, {hi:.0}]"
        );
    }

    /// Pareto with shape=1.5, scale=100_000 has a heavier right tail than
    /// LogNormal with the same approximate median. Compare 99th percentiles
    /// from 10k samples each.
    #[test]
    fn severity_pareto_tail_heavier_than_lognormal() {
        let pareto = SeverityModel::Pareto { scale: 100_000.0, shape: 1.5 };
        // LogNormal(ln(100_000), 0.5) has median ≈ 100_000
        let lognorm = SeverityModel::LogNormal { mu: (100_000_f64).ln(), sigma: 0.5 };

        let mut rng = rng();
        let n = 10_000usize;

        let mut pareto_samples: Vec<u64> = (0..n).map(|_| pareto.sample(&mut rng)).collect();
        let mut lognorm_samples: Vec<u64> = (0..n).map(|_| lognorm.sample(&mut rng)).collect();

        pareto_samples.sort_unstable();
        lognorm_samples.sort_unstable();

        let p99_pareto = pareto_samples[n * 99 / 100];
        let p99_lognorm = lognorm_samples[n * 99 / 100];

        assert!(
            p99_pareto > p99_lognorm,
            "Pareto 99th pct {p99_pareto} should exceed LogNormal 99th pct {p99_lognorm}"
        );
    }

    /// Single-config schedule: every LossEvent must carry the config's peril and region.
    #[test]
    fn schedule_returns_correct_peril_region_pairs() {
        let configs = vec![PerilConfig {
            peril: Peril::EarthquakeUS,
            region: "US-CA",
            annual_frequency: 5.0,
            severity: SeverityModel::Pareto { scale: 1_000_000.0, shape: 1.5 },
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
            severity: SeverityModel::Pareto { scale: 1_000_000.0, shape: 1.5 },
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
            severity: SeverityModel::LogNormal { mu: 11.5, sigma: 1.2 },
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

    /// Per-policy attritional scheduler must produce ClaimSettled events with
    /// amounts bounded by (limit − attachment) × share_bps / 10_000.
    #[test]
    fn schedule_attritional_claims_for_policy_produces_bounded_claims() {
        use std::collections::HashMap;

        use crate::events::{Panel, PanelEntry, Risk};
        use crate::types::{PolicyId, SyndicateId};

        let mut rng = rng();
        let risk = Risk {
            line_of_business: "property".to_string(),
            sum_insured: 2_000_000_000,
            territory: "UK".to_string(),
            limit: 200_000_000,
            attachment: 0, // zero attachment ensures all claims penetrate
            perils_covered: vec![Peril::Attritional],
        };
        let panel = Panel {
            entries: vec![PanelEntry {
                syndicate_id: SyndicateId(1),
                share_bps: 10_000,
                premium: 50_000,
            }],
        };
        // Use a very high rate so we reliably get claims in a single test run.
        let configs: HashMap<&'static str, AttritionalConfig> = [(
            "UK",
            AttritionalConfig {
                annual_rate: 50.0,
                severity: SeverityModel::LogNormal { mu: 15.5, sigma: 1.0 },
            },
        )]
        .into_iter()
        .collect();

        let events = schedule_attritional_claims_for_policy(
            PolicyId(0),
            &risk,
            &panel,
            Year(1),
            &mut rng,
            &configs,
        );

        assert!(!events.is_empty(), "expected ClaimSettled events with rate=50.0");

        let max_per_claim = risk.limit - risk.attachment; // attachment=0
        for (_, e) in &events {
            match e {
                Event::ClaimSettled { amount, .. } => {
                    assert!(
                        *amount <= max_per_claim,
                        "amount {amount} exceeds limit−attachment {max_per_claim}"
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

        use crate::events::{Panel, PanelEntry, Risk};
        use crate::types::{PolicyId, SyndicateId};

        let mut rng = rng();
        let risk = Risk {
            line_of_business: "property".to_string(),
            sum_insured: 2_000_000_000,
            territory: "UK".to_string(),
            limit: 200_000_000,
            attachment: 0,
            perils_covered: vec![Peril::WindstormAtlantic], // not Attritional
        };
        let panel = Panel {
            entries: vec![PanelEntry {
                syndicate_id: SyndicateId(1),
                share_bps: 10_000,
                premium: 0,
            }],
        };
        let configs: HashMap<&'static str, AttritionalConfig> = [(
            "UK",
            AttritionalConfig {
                annual_rate: 50.0,
                severity: SeverityModel::LogNormal { mu: 15.5, sigma: 1.0 },
            },
        )]
        .into_iter()
        .collect();

        let events = schedule_attritional_claims_for_policy(
            PolicyId(0),
            &risk,
            &panel,
            Year(1),
            &mut rng,
            &configs,
        );

        assert!(events.is_empty(), "non-Attritional policy must produce no attritional claims");
    }
}
