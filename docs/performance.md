# Performance Baselines and Findings

Criterion benchmarks live in `benches/simulation_perf.rs`. Run them with:

```bash
cargo bench                              # all groups
cargo bench -- loss_distribution         # one group
cargo bench -- --save-baseline initial   # save a named baseline
cargo bench -- --baseline initial        # compare against saved baseline
```

HTML reports are written to `target/criterion/` (not committed).

## Baseline — 2026-02-20 (post-attritional)

Machine: Apple M-series (darwin 25.2.0), optimised build (`--release`).

Numbers below reflect the state after wiring attritional coverage: every broker
submission now includes `Peril::Attritional`, so bound policies generate attritional
claims against the per-region Poisson schedules (λ=12/year for US-SE and UK in the
benchmark fixture). This is the dominant workload change relative to the pre-attritional
baseline — see **Finding 4**.

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
| small | 5 | 2 × 10 | 212 µs | 94 K/s |
| medium | 20 | 10 × 100 | 35 ms | 28 K/s |
| large | 80 | 25 × 500 | ~2.3 s | ~5.4 K/s |

Large improved ~1.7× (from ~4 s) after adding the peril/territory index (Finding 4 fix).
Small and medium are essentially unchanged — at those scales most policies match the
loss region anyway, so the index saves little filtering work. The remaining large-scale
cost is queue churn and `ClaimSettled` dispatch; see **Finding 4** (updated).

### `multi_year` — medium scenario, 1/5/10 years

| years | total time | per-year |
|---|---|---|
| 1 | 33 ms | 33 ms |
| 5 | 423 ms | 85 ms |
| 10 | 1.85 s | 185 ms |

Per-year cost grows ~5.6× from year 1 to year 10 (was 2.7× pre-index). The absolute
numbers are much lower post-index, but per-year cost still grows super-linearly: the
`log: Vec<SimEvent>` grows across years, increasing push/reallocation overhead, and
later years have more bound policies (hence more claim fan-out). Not alarming at medium
scale; revisit at large scale.

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

### Finding 2 — `full_year/large` bottleneck is now attritional claim processing, not quoting

The large scenario (80 syndicates, 25 brokers, 500 subs/broker) runs in ~4 s
post-attritional (was 627 ms). The quoting pipeline cost is unchanged; the new
dominant term is `on_loss_event` called for each of ~24 attritional events per year
(λ=12 for US-SE and UK). Each call iterates all ~10,000 bound policies and emits one
`ClaimSettled` per matching panel entry (~3,300 matching × 80 entries = ~265,000
events per attritional event, ~6.4 M per year). At ~64 Melem/s throughput from the
`loss_distribution` benchmark, attritional processing alone accounts for ~100 ms at
large scale; the remaining cost is queue churn, `ClaimSettled` dispatch, and capital
updates across 80 syndicates per event. Profiling needed to identify the exact split.

### Finding 3 — loss distribution cache cliff at ~10,000 policies

`on_loss_event` throughput drops from ~64 Melem/s to ~34 Melem/s between 5,000 and
10,000 policies. The `HashMap<PolicyId, BoundPolicy>` iteration becomes cache-unfriendly
once the map exceeds L3 capacity. If the simulation needs to handle >10k simultaneous
live policies, consider switching `policies` to a `Vec<BoundPolicy>` (sorted by
policy_id for O(log n) lookup) or maintaining a pre-filtered index by territory/peril
to skip non-matching policies without touching their data.

### Finding 4 — attritional coverage is the dominant workload at medium-to-large scale

Wiring `Peril::Attritional` into every broker submission increased `full_year/medium`
from 9.9 ms to 34 ms (3.4×) and `full_year/large` from 627 ms to ~4 s (6.4×). The
asymmetric scaling arises because attritional event count is fixed per year (Poisson λ)
but claim volume scales with O(policies × panel_size). At large scale the attritional
claim fan-out dominates all other costs.

**Partially addressed (2026-02-20):** A `peril_territory_index: HashMap<(String, Peril),
Vec<PolicyId>>` was added to `Market`. `on_policy_bound` now registers each peril in
the index; `on_loss_event` does a single index lookup and iterates only matching
policies, eliminating the O(all_policies) scan. This reduced `full_year/large` from
~4 s to ~2.3 s (~1.7×) and `multi_year` proportionally. The remaining cost is the
O(matching_policies × panel_size) inner loop plus queue churn from the resulting
`ClaimSettled` events — the index cannot help with that. Further improvement would
require reducing attritional fan-out (e.g., aggregate attritional claims per syndicate
before emitting events, or decoupling attritional losses from the per-policy routing
path).
