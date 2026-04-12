# rins

A simulation of the Lloyd's of London insurance market, built from the ground up in Rust.

The goal is simple: drop a handful of independent agents — insurers, brokers, and asset owners — into a virtual marketplace and watch realistic market behaviour emerge on its own. Booms, busts, catastrophe-driven crises, and herd mentality are never programmed in; they arise naturally from each agent following its own rules.

## What it simulates

**Lloyd's of London** is one of the oldest and most distinctive insurance markets in the world. Specialist syndicates (insurers) compete to cover risks brought to market by brokers on behalf of their clients. rins recreates this process as a discrete-event simulation: time jumps from one meaningful event to the next rather than ticking forward in fixed steps.

A typical simulated year plays out like this:

1. **Asset owners** (the insured) request coverage for their property.
2. A **broker** shops each request to a shortlist of insurers, favouring those it has worked with before.
3. Each **insurer** prices the risk using its own loss history, capital position, and a read on the broader market. It can accept or decline.
4. The broker presents the best quote to the asset owner, who accepts or rejects based on affordability.
5. If accepted, a **policy is bound** — coverage is live.
6. Throughout the year, **losses happen**: routine attritional damage (a warehouse fire, a burst pipe) and occasional catastrophes (a major Atlantic windstorm hitting an entire territory).
7. Claims are settled. Insurers pay out, and their capital shrinks. If an insurer runs out of money, it becomes **insolvent**.
8. At year-end, every agent learns from what happened. Insurers update their pricing models. Brokers adjust their relationship preferences. If the market is highly profitable, **new insurers enter**.

Over many simulated years, this loop produces recognisable patterns from the real insurance world — underwriting cycles where prices swing between too-cheap and too-expensive, catastrophe shocks that wipe out undercapitalised players, and concentration effects where a few dominant insurers capture most of the market.

## Project components

| Component | What it does |
|---|---|
| **Insurer** | Prices risks, manages capital, tracks exposure limits, learns from loss experience via a credibility-weighted moving average. Each insurer has its own risk appetite and capital base. |
| **Broker** | Routes coverage requests to insurers based on relationship scores. Incumbents get priority, creating realistic switching costs and loyalty effects. |
| **Insured** | Owns assets, requests coverage each year, and decides whether a quoted premium is affordable. Remembers recent losses and adjusts willingness to pay. |
| **Market coordinator** | The referee. Routes losses to the right policies, handles insolvency, manages insurer entry and exit, and broadcasts market-wide signals. Makes no pricing decisions. |
| **Peril models** | Generates losses. Attritional losses (small, frequent) use a log-normal model. Catastrophes (rare, large) use a Pareto-tailed model with shared damage across a territory. |
| **Event log** | Every action in the simulation is recorded as an immutable event. This log is the single source of truth — the entire simulation can be reconstructed from it. |
| **Analysis engine** | Runs 15 invariant checks on the event log to catch simulation errors, then produces a year-by-year summary table of key market metrics. |

## Getting started

### Prerequisites

- [Rust](https://www.rust-lang.org/tools/install) (2024 edition)

### Build and run

```bash
# Build
cargo build

# Run a simulation (writes events.ndjson to the current directory)
cargo run

# Run with options
cargo run -- --years 30 --seed 42 --quiet
```

### Analyse the output

```bash
# Run the analysis binary — checks 15 invariants and prints a year-by-year summary
cargo run --release --bin analyse
```

The simulation writes its event log to `events.ndjson` — one JSON object per line. Each event records what happened, when (in simulation days), and which agents were involved. You can inspect this file directly or feed it into your own analysis scripts.

### Run multiple simulations

```bash
# Run 100 simulations in parallel with different seeds, output to a directory
cargo run -- --runs 100 --output-dir results/ --csv
```

This produces per-seed event logs and a CSV summary useful for statistical analysis across runs.

### Other commands

```bash
cargo test           # Run the test suite
cargo clippy         # Lint
cargo fmt            # Format code
cargo bench          # Run performance benchmarks
```

## Documentation

Detailed documentation lives in `docs/`:

- **phenomena.md** — the emergent behaviours the simulation aims to reproduce, with progress status
- **market-mechanics.md** — the institutional rules governing how the market operates
- **event-flow.md** — a diagram and index of every event type, who produces it, and who consumes it
- **event-sourcing.md** — the architectural pattern underpinning the simulation
- **calibration.md** — where the numbers come from
- **roadmap.md** — what's next
