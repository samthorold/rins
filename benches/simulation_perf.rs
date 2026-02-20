mod fixtures;

use std::cmp::Reverse;
use std::collections::BinaryHeap;

use criterion::{BatchSize, BenchmarkId, Criterion, Throughput, criterion_group, criterion_main};

use rins::events::{Event, Peril, SimEvent};
use rins::market::Market;
use rins::types::{Day, SyndicateId};

use fixtures::{LARGE, MEDIUM, SMALL, build_simulation, prepopulate_policies};

// ── Group 1: loss_distribution — policy count scaling ───────────────────────

fn bench_loss_distribution(c: &mut Criterion) {
    let mut group = c.benchmark_group("loss_distribution");
    let panel_size = 5usize;
    for &policy_count in &[100usize, 500, 1_000, 5_000, 10_000] {
        group.throughput(Throughput::Elements((policy_count * panel_size) as u64));
        group.bench_with_input(
            BenchmarkId::from_parameter(policy_count),
            &policy_count,
            |b, &pc| {
                b.iter_batched(
                    || {
                        let mut market = Market::new();
                        prepopulate_policies(&mut market, pc, panel_size);
                        market
                    },
                    |market| market.on_loss_event(Day(180), "US-SE", Peril::WindstormAtlantic, 5_000_000),
                    BatchSize::LargeInput,
                )
            },
        );
    }
    group.finish();
}

// ── Group 2: loss_panel_size — panel size scaling ────────────────────────────

fn bench_loss_panel_size(c: &mut Criterion) {
    let mut group = c.benchmark_group("loss_panel_size");
    let policy_count = 1_000usize;
    for &panel_size in &[3usize, 5, 10] {
        group.throughput(Throughput::Elements((policy_count * panel_size) as u64));
        group.bench_with_input(
            BenchmarkId::from_parameter(panel_size),
            &panel_size,
            |b, &ps| {
                b.iter_batched(
                    || {
                        let mut market = Market::new();
                        prepopulate_policies(&mut market, policy_count, ps);
                        market
                    },
                    |market| market.on_loss_event(Day(180), "US-SE", Peril::WindstormAtlantic, 5_000_000),
                    BatchSize::LargeInput,
                )
            },
        );
    }
    group.finish();
}

// ── Group 3: full_year — end-to-end single year ──────────────────────────────

fn bench_full_year(c: &mut Criterion) {
    let mut group = c.benchmark_group("full_year");
    for (name, scenario) in [("small", &SMALL), ("medium", &MEDIUM), ("large", &LARGE)] {
        if name == "large" {
            group.sample_size(10);
        }
        group.throughput(Throughput::Elements(
            (scenario.brokers * scenario.submissions_per_broker) as u64,
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

// ── Group 4: multi_year — year-over-year scaling ─────────────────────────────

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

// ── Group 5: event_queue — BinaryHeap in isolation ───────────────────────────

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
                        // Pre-build interleaved days (deterministic, no RNG) to ensure
                        // the heap has real reordering work to do.
                        (0..n)
                            .map(|i| {
                                let day = if i % 2 == 0 { i as u64 } else { (n - i) as u64 };
                                Reverse(SimEvent {
                                    day: Day(day),
                                    event: Event::SyndicateEntered {
                                        syndicate_id: SyndicateId(i as u64),
                                    },
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

// ── Group 6: syndicate_lookup — O(n) find cost ───────────────────────────────

fn bench_syndicate_lookup(c: &mut Criterion) {
    let mut group = c.benchmark_group("syndicate_lookup");
    for &pool_size in &[5usize, 20, 40, 80] {
        group.bench_with_input(
            BenchmarkId::from_parameter(pool_size),
            &pool_size,
            |b, &n| {
                let syndicates = fixtures::make_syndicates(n, 50_000_000, 500);
                let target = SyndicateId(n as u64); // last element — worst case
                b.iter(|| syndicates.iter().find(|s| s.id == target))
            },
        );
    }
    group.finish();
}

criterion_group!(
    benches,
    bench_loss_distribution,
    bench_loss_panel_size,
    bench_full_year,
    bench_multi_year,
    bench_event_queue,
    bench_syndicate_lookup,
);
criterion_main!(benches);
