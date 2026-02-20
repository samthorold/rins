# CLAUDE.md

This file provides guidance to Claude Code (claude.ai/code) when working with code in this repository.

## Commands

```bash
cargo build          # Debug build
cargo build --release  # Release build
cargo run            # Build and run
cargo test           # Run tests
cargo test <name>    # Run a single test by name
cargo clippy         # Lint
cargo fmt            # Format code
```

## Project

Rust project using the 2024 edition. Currently a minimal starter — the entry point is `src/main.rs`.

## Project Purpose

`rins` is a discrete event simulation (DES) of the Lloyd's of London insurance market. The goal is to reproduce emergent market phenomena — underwriting cycles, catastrophe-amplified crises, broker-syndicate network herding — from first-principles agent behaviour. Macro behaviour is never hardcoded; it must arise from agent interactions.

Target phenomena — emergent macro-level behaviours that the simulation must reproduce without hardcoding — are tracked in `docs/phenomena.md`. Structural rules, institutional procedures, and agent protocols that govern *how* the market operates are in `docs/market-mechanics.md`. Both are developed incrementally alongside the simulation.

Reference literature and calibration notes live in `~/Documents/Reference/ABM/` and `../insurance-market/catastrophe-calibration.md`.

## Architecture: DES + Event Sourcing

**DES (clockless time):** Time advances by pulling the lowest-timestamp event from a priority queue. There is no fixed-tick loop. Events schedule future events.

**Event sourcing:** The event stream is the ground truth. It is immutable. Agent state (capital, portfolio, relationship scores, actuarial estimates) is derived by replaying events. Where full event sourcing creates unacceptable complexity (e.g., floating-point EWMA accumulation), mutable in-agent state is acceptable — but the agent must be reconstructible from its event slice.

**Randomness:** All randomness flows through a seeded RNG passed explicitly. Simulations must be reproducible given the same seed.

## Agent Design Philosophy

Prefer agents with complex internal logic over decomposing that logic into sub-agents or strategy objects.

**Syndicate** owns all pricing logic via two channels that share state and must coexist in a single agent — do not separate them into sub-agents or strategy traits. Syndicate heterogeneity is expressed through parameters; values are calibration concerns, not architectural ones — expect them to vary across experiments. Capital management (exposure limits, solvency floor, concentration limits) is internal to `Syndicate`. The coordinator never overrides a syndicate's pricing or acceptance decision.

**Broker** tracks relationship strength with syndicates across lines of business and uses this to route risks and assemble panels. Heterogeneity is in specialism parameters, not subtyping.

**Market (Coordinator)** orchestrates cross-agent interactions that cannot belong to a single agent: quoting rounds, loss distribution, insolvency processing, syndicate entry/exit, and industry-aggregate statistics. It makes no pricing decisions.

Market mechanics — the structural rules and institutional invariants governing how the market operates — are documented in `docs/market-mechanics.md`. The document describes *what* the market does, not how the simulation implements it; implementation choices and calibration values belong in code and calibration notes, not here.

**`docs/event-flow.md` must be kept in sync.** Update it whenever you add, remove, or rename an event variant in `src/events.rs`, change which agent produces or consumes an event, or alter the day-offset logic in `src/simulation.rs`, `src/market.rs`, or any agent handler.

## Performance

Criterion benchmarks are in `benches/simulation_perf.rs`. Baselines, findings, and
guidance on when to act on them are in `docs/performance.md`.

```bash
cargo bench                             # run all benchmark groups
cargo bench -- --save-baseline initial  # save a named baseline
cargo bench -- --baseline initial       # regression check against saved baseline
cargo test -- --ignored stress_scenario_completes_within_budget --nocapture
```

## Testing Approach

Test-first. Write a failing test before implementing any behaviour.

**Event injection pattern**: Tests build a synthetic historical event stream and feed it to an agent or coordinator to derive initial state. The test then fires a new event and asserts on output events or derived state. This avoids coupling tests to internal fields and exercises the event-sourcing path.

Use property tests (proptest) for monotonicity properties where meaningful.
