mod fixtures;

use std::cmp::Reverse;
use std::collections::{BinaryHeap, HashMap};

use criterion::{BatchSize, BenchmarkId, Criterion, Throughput, criterion_group, criterion_main};
use rand::SeedableRng;
use rand_chacha::ChaCha20Rng;

use rins::events::{Event, Peril, SimEvent};
use rins::market::Market;
use rins::perils::DamageFractionModel;
use rins::types::{Day, InsurerId, Year};

use fixtures::{LARGE, MEDIUM, SMALL, build_simulation, prepopulate_policies};

fn default_damage_models() -> HashMap<Peril, DamageFractionModel> {
    HashMap::from([
        (Peril::WindstormAtlantic, DamageFractionModel::Pareto { scale: 0.05, shape: 1.5, cap: 1.0 }),
        (Peril::Attritional, DamageFractionModel::LogNormal { mu: -3.0, sigma: 1.0 }),
    ])
}

// ── Group 1: loss_distribution — policy count scaling ───────────────────────

fn bench_loss_distribution(c: &mut Criterion) {
    let mut group = c.benchmark_group("loss_distribution");
    for &policy_count in &[100usize, 500, 1_000, 5_000, 10_000] {
        group.throughput(Throughput::Elements(policy_count as u64));
        group.bench_with_input(
            BenchmarkId::from_parameter(policy_count),
            &policy_count,
            |b, &pc| {
                b.iter_batched(
                    || {
                        let mut market = Market::new();
                        prepopulate_policies(&mut market, pc);
                        let damage_models = default_damage_models();
                        let rng = ChaCha20Rng::seed_from_u64(42);
                        (market, damage_models, rng)
                    },
                    |(market, damage_models, mut rng)| {
                        market.on_loss_event(
                            Day(180),
                            Peril::WindstormAtlantic,
                            "US-SE",
                            &damage_models,
                            &mut rng,
                        )
                    },
                    BatchSize::LargeInput,
                )
            },
        );
    }
    group.finish();
}

// ── Group 2: full_year — end-to-end single year ──────────────────────────────

fn bench_full_year(c: &mut Criterion) {
    let mut group = c.benchmark_group("full_year");
    for (name, scenario) in [("small", &SMALL), ("medium", &MEDIUM), ("large", &LARGE)] {
        if name == "large" {
            group.sample_size(10);
        }
        group.throughput(Throughput::Elements(
            scenario.n_insureds as u64,
        ));
        group.bench_function(BenchmarkId::from_parameter(name), |b| {
            b.iter_batched(
                || build_simulation(scenario, 42, 1),
                |mut sim| sim.run(),
                BatchSize::LargeInput,
            )
        });
    }
    group.finish();
}

// ── Group 3: multi_year — year-over-year scaling ─────────────────────────────

fn bench_multi_year(c: &mut Criterion) {
    let mut group = c.benchmark_group("multi_year");
    group.sample_size(10);
    for &years in &[1u32, 5, 10] {
        group.bench_with_input(
            BenchmarkId::from_parameter(years),
            &years,
            |b, &y| {
                b.iter_batched(
                    || build_simulation(&MEDIUM, 42, y),
                    |mut sim| sim.run(),
                    BatchSize::LargeInput,
                )
            },
        );
    }
    group.finish();
}

// ── Group 4: event_queue — BinaryHeap in isolation ───────────────────────────

fn bench_event_queue(c: &mut Criterion) {
    let mut group = c.benchmark_group("event_queue");
    for &count in &[1_000usize, 10_000, 100_000, 1_000_000] {
        group.throughput(Throughput::Elements(count as u64));
        group.bench_with_input(
            BenchmarkId::from_parameter(count),
            &count,
            |b, &n| {
                b.iter_batched(
                    || {
                        (0..n)
                            .map(|i| {
                                let day = if i % 2 == 0 { i as u64 } else { (n - i) as u64 };
                                Reverse(SimEvent {
                                    day: Day(day),
                                    event: Event::YearEnd { year: Year(1) },
                                })
                            })
                            .collect::<Vec<_>>()
                    },
                    |items| {
                        let mut heap = BinaryHeap::with_capacity(items.len());
                        for item in items {
                            heap.push(item);
                        }
                        while let Some(v) = heap.pop() {
                            std::hint::black_box(v);
                        }
                    },
                    BatchSize::LargeInput,
                )
            },
        );
    }
    group.finish();
}

// ── Group 5: insurer_lookup — O(n) find cost ─────────────────────────────────

fn bench_insurer_lookup(c: &mut Criterion) {
    let mut group = c.benchmark_group("insurer_lookup");
    for &pool_size in &[5usize, 20, 40, 80] {
        group.bench_with_input(
            BenchmarkId::from_parameter(pool_size),
            &pool_size,
            |b, &n| {
                let insurers: Vec<rins::insurer::Insurer> = (1..=n)
                    .map(|i| {
                        rins::insurer::Insurer::new(InsurerId(i as u64), 100_000_000_000, 0.239, 0.0, 0.70, 0.3, 0.344, 0.0, None, None, 0.252, 0.0)
                    })
                    .collect();
                let target = InsurerId(n as u64); // last element — worst case
                b.iter(|| insurers.iter().find(|ins| ins.id == target))
            },
        );
    }
    group.finish();
}

criterion_group!(
    benches,
    bench_loss_distribution,
    bench_full_year,
    bench_multi_year,
    bench_event_queue,
    bench_insurer_lookup,
);
criterion_main!(benches);
