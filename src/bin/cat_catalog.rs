use std::collections::HashMap;
use std::env;

use rand::SeedableRng;
use rand_chacha::ChaCha20Rng;
use rins::config::SimulationConfig;
use rins::perils::generate_cat_catalog;

fn main() {
    let config = SimulationConfig::canonical();

    let n_years: u32 = env::args()
        .nth(1)
        .and_then(|s| s.parse().ok())
        .unwrap_or(config.years);

    let mut rng = ChaCha20Rng::seed_from_u64(config.seed);
    let entries = generate_cat_catalog(&config.catastrophe, n_years, &mut rng);

    // Write NDJSON to stdout.
    for entry in &entries {
        println!("{}", serde_json::to_string(entry).expect("serialisation failed"));
    }

    // Per-territory summary to stderr.
    let mut counts: HashMap<&str, usize> = HashMap::new();
    let mut sum_df: HashMap<&str, f64> = HashMap::new();
    let mut max_df: HashMap<&str, f64> = HashMap::new();
    for e in &entries {
        let t = e.territory.as_str();
        *counts.entry(t).or_insert(0) += 1;
        *sum_df.entry(t).or_insert(0.0) += e.damage_fraction;
        let cur = max_df.entry(t).or_insert(0.0);
        if e.damage_fraction > *cur {
            *cur = e.damage_fraction;
        }
    }

    eprintln!(
        "cat_catalog: {} years, {} total events (expected ~{:.1})",
        n_years,
        entries.len(),
        config.catastrophe.annual_frequency * n_years as f64
    );
    let mut territories: Vec<&str> = counts.keys().copied().collect();
    territories.sort_unstable();
    for t in territories {
        let n = counts[t];
        let mean = sum_df[t] / n as f64;
        let max = max_df[t];
        eprintln!("  {t:<12}  events={n:>4}  mean_df={mean:.4}  max_df={max:.4}");
    }
}
