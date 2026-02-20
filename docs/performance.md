# Performance Baselines and Findings

Criterion benchmarks live in `benches/simulation_perf.rs`. Run them with:

```bash
cargo bench                              # all groups
cargo bench -- loss_distribution         # one group
cargo bench -- --save-baseline initial   # save a named baseline
cargo bench -- --baseline initial        # compare against saved baseline
```

HTML reports are written to `target/criterion/` (not committed).

## Baseline — 2026-02-20

Machine: Apple M-series (darwin 25.2.0), optimised build (`--release`).

### `loss_distribution` — `Market::on_loss_event`, policy count scaling (panel size = 5)

Measures the O(P × S) inner loop: iterate bound policies, filter by territory/peril,
emit one `ClaimSettled` per panel entry. Throughput is in ClaimSettled-events/second.

| policies | time | throughput |
|---|---|---|
| 100 | 8.3 µs | 60 Melem/s |
| 500 | 37 µs | 68 Melem/s |
| 1,000 | 79 µs | 63 Melem/s |
| 5,000 | 393 µs | 64 Melem/s |
| 10,000 | 1.46 ms | 34 Melem/s |

Throughput is flat from 500–5,000 policies (~64 Melem/s) then drops by half at 10,000.
The inflection is a cache miss: the `HashMap<PolicyId, BoundPolicy>` stops fitting in
L2/L3 around that size. See **Finding 3** below.

### `loss_panel_size` — inner-loop cost per panel entry (1,000 policies)

| panel size | time |
|---|---|
| 3 | 70 µs |
| 5 | 79 µs |
| 10 | 101 µs |

Doubling panel size from 5→10 adds ~28% wall time. Each additional entry is a
sequential read of a small struct — cache-hot and cheap.

### `full_year` — end-to-end single year (quoting pipeline + stochastic cats)

Throughput is in submissions/second.

| scenario | syndicates | brokers × subs/broker | time | throughput |
|---|---|---|---|---|
| small | 5 | 2 × 10 | 44 µs | 450 K/s |
| medium | 20 | 10 × 100 | 9.9 ms | 101 K/s |
| large | 80 | 25 × 500 | 627 ms | 19.9 K/s |

Large is ~14× slower than a linear extrapolation from medium. The cause is the O(n)
syndicate lookup in `dispatch` firing on every `QuoteRequested` event. See **Finding 2**.

### `multi_year` — medium scenario, 1/5/10 years

| years | total time | per-year |
|---|---|---|
| 1 | 9.4 ms | 9.4 ms |
| 5 | 64.8 ms | 13.0 ms |
| 10 | 145 ms | 14.5 ms |

Near-linear with a ~54% per-year overhead by year 10. Likely `log: Vec<SimEvent>`
growing and triggering periodic reallocations as the log accumulates across years.
Not alarming at current scales.

### `event_queue` — `BinaryHeap` push+drain in isolation

Establishes the queue-only floor, separate from dispatch overhead.

| events | time | throughput |
|---|---|---|
| 1,000 | 56 µs | 17.9 Melem/s |
| 10,000 | 740 µs | 13.5 Melem/s |
| 100,000 | 9.98 ms | 10.0 Melem/s |
| 1,000,000 | 136 ms | 7.3 Melem/s |

Throughput degrades gradually with heap size (heap sort becomes less cache-friendly
as the working set grows), but there is no sharp cliff.

### `syndicate_lookup` — `iter().find()` worst-case, O(n)

| pool size | time |
|---|---|
| 5 | 2.2 ns |
| 20 | 7.0 ns |
| 40 | 13.5 ns |
| 80 | 34 ns |

Perfectly linear at ~0.43 ns/element. See **Finding 1**.

---

## Findings

### Finding 1 — syndicate lookup is not the bottleneck at current scales

`dispatch` finds the target syndicate with `syndicates.iter().find(|s| s.id == sid)`,
an O(n) scan. At 80 syndicates (worst case) this costs 34 ns per call. Even at large
scale this is negligible compared to the ~10 µs per submission for quoting pipeline
overhead. No HashMap index is warranted yet; revisit if syndicate count exceeds ~500.

### Finding 2 — `full_year/large` bottleneck is quoting pipeline dispatch, not lookup

The large scenario (80 syndicates, 25 brokers, 500 subs/broker) runs in 627 ms.
At 100% bind rate each submission triggers 2S + 2 events through the quoting pipeline
(S = 80). That is ~12,500 submissions × 162 events = ~2 M dispatches, many of which
touch the syndicate pool. The per-syndicate lookup (34 ns) across 12,500 lead quotes
accounts for only ~0.4 ms; the bulk of the 627 ms is quoting pipeline overhead
(risk cloning, HashMap operations in Market, queue churn). Profiling is needed to
identify the dominant term before optimising.

### Finding 3 — loss distribution cache cliff at ~10,000 policies

`on_loss_event` throughput drops from ~64 Melem/s to ~34 Melem/s between 5,000 and
10,000 policies. The `HashMap<PolicyId, BoundPolicy>` iteration becomes cache-unfriendly
once the map exceeds L3 capacity. If the simulation needs to handle >10k simultaneous
live policies, consider switching `policies` to a `Vec<BoundPolicy>` (sorted by
policy_id for O(log n) lookup) or maintaining a pre-filtered index by territory/peril
to skip non-matching policies without touching their data.
