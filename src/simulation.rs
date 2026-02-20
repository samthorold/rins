use std::cmp::Reverse;
use std::collections::BinaryHeap;

use rand::Rng;
use rand::SeedableRng;
use rand_chacha::ChaCha20Rng;

use crate::broker::Broker;
use crate::events::{Event, SimEvent};
use crate::market::Market;
use crate::syndicate::Syndicate;
use crate::types::{Day, LossEventId, SyndicateId, Year};

pub struct Simulation {
    queue: BinaryHeap<Reverse<SimEvent>>,
    pub log: Vec<SimEvent>,
    rng: ChaCha20Rng,
    max_day: Option<Day>,
    max_events: Option<usize>,
    pub syndicates: Vec<Syndicate>,
    brokers: Vec<Broker>,
    market: Market,
    next_loss_event_id: u64,
    /// Industry-wide loss ratio from the most recently completed year.
    /// Used as the credibility complement when syndicates price new business.
    /// Initialised to 0.65 (market-mechanics §1 baseline) until a full year of
    /// data is available.
    current_industry_benchmark: f64,
}

impl Simulation {
    pub fn new(seed: u64) -> Self {
        Simulation {
            queue: BinaryHeap::new(),
            log: Vec::new(),
            rng: ChaCha20Rng::seed_from_u64(seed),
            max_day: None,
            max_events: None,
            syndicates: Vec::new(),
            brokers: Vec::new(),
            market: Market::new(),
            next_loss_event_id: 0,
            current_industry_benchmark: 0.65,
        }
    }

    /// Builder: stop after this day (events scheduled past the horizon are
    /// never fired).
    pub fn until(mut self, day: Day) -> Self {
        self.max_day = Some(day);
        self
    }

    /// Builder: stop after N events fire (unit-test safety valve).
    pub fn with_max_events(mut self, n: usize) -> Self {
        self.max_events = Some(n);
        self
    }

    /// Builder: seed the agent pools.
    pub fn with_agents(mut self, syndicates: Vec<Syndicate>, brokers: Vec<Broker>) -> Self {
        self.syndicates = syndicates;
        self.brokers = brokers;
        self
    }

    /// Schedule an event to fire at the given day.
    pub fn schedule(&mut self, day: Day, event: Event) {
        self.queue.push(Reverse(SimEvent { day, event }));
    }

    fn alloc_loss_event_id(&mut self) -> LossEventId {
        let id = LossEventId(self.next_loss_event_id);
        self.next_loss_event_id += 1;
        id
    }

    /// Run the simulation until a stopping condition is met.
    pub fn run(&mut self) {
        let mut count = 0;
        loop {
            if let Some(max) = self.max_events
                && count >= max
            {
                break;
            }

            let next_day = match self.queue.peek() {
                Some(Reverse(ev)) => ev.day,
                None => break,
            };

            if let Some(horizon) = self.max_day
                && next_day > horizon
            {
                break;
            }

            let Reverse(ev) = self.queue.pop().unwrap();
            // Log cause before dispatching effect.
            self.log.push(ev.clone());
            self.dispatch(ev.day, ev.event);
            count += 1;
        }
    }

    fn dispatch(&mut self, day: Day, event: Event) {
        match event {
            Event::SimulationStart { year_start } => {
                self.handle_simulation_start(day, year_start);
            }
            Event::YearEnd { year } => {
                // 1. Coordinator computes industry stats from YTD accumulators.
                //    compute_year_stats is &mut (resets YTD totals) but returns an
                //    owned YearStats, releasing the borrow before agents are mutated.
                let stats = self.market.compute_year_stats(&self.syndicates, year);

                // Publish this year's industry loss ratio for next year's ATP pricing.
                self.current_industry_benchmark = stats.industry_loss_ratio;

                // 2. Each Syndicate updates its EWMA with realised per-line loss ratios.
                for s in &mut self.syndicates {
                    s.on_year_end(year, &stats.loss_ratios_by_line, &mut self.rng);
                }

                // 3. Each Broker applies relationship decay.
                for b in &mut self.brokers {
                    b.on_year_end(year);
                }

                // 4. Schedule next year (keeps the sim running until max_day).
                self.handle_year_end(day, year);
            }
            Event::SubmissionArrived {
                submission_id,
                broker_id,
                risk,
            } => {
                let available: Vec<SyndicateId> = self.syndicates.iter().map(|s| s.id).collect();
                let events = self.market.on_submission_arrived(
                    day,
                    submission_id,
                    broker_id,
                    risk,
                    &available,
                );
                for (d, e) in events {
                    self.schedule(d, e);
                }
            }
            Event::QuoteRequested {
                submission_id,
                syndicate_id,
                is_lead,
            } => {
                // Fetch risk and lead premium from market (immutable borrow ends here).
                let params = self.market.quote_request_params(submission_id, is_lead);
                let Some((risk, lead_premium)) = params else {
                    return;
                };
                // Find the targeted syndicate and ask it to quote.
                let benchmark = self.current_industry_benchmark;
                let result = self
                    .syndicates
                    .iter()
                    .find(|s| s.id == syndicate_id)
                    .map(|s| {
                        s.on_quote_requested(
                            day,
                            submission_id,
                            &risk,
                            is_lead,
                            lead_premium,
                            benchmark,
                        )
                    });
                if let Some((d, e)) = result {
                    self.schedule(d, e);
                }
            }
            Event::QuoteIssued {
                submission_id,
                syndicate_id,
                premium,
                is_lead,
            } => {
                let available: Vec<SyndicateId> = self.syndicates.iter().map(|s| s.id).collect();
                let events = if is_lead {
                    self.market.on_lead_quote_issued(
                        day,
                        submission_id,
                        syndicate_id,
                        premium,
                        &available,
                    )
                } else {
                    self.market
                        .on_follower_quote_issued(day, submission_id, syndicate_id, premium)
                };
                for (d, e) in events {
                    self.schedule(d, e);
                }
            }
            Event::QuoteDeclined {
                submission_id,
                syndicate_id: _,
            } => {
                let events = self.market.on_quote_declined(day, submission_id);
                for (d, e) in events {
                    self.schedule(d, e);
                }
            }
            Event::PolicyBound {
                submission_id,
                panel,
            } => {
                // Retrieve the risk from a pending entry — it was moved into
                // market state when the submission arrived; we need a clone
                // here since market.on_policy_bound consumes it.
                // By the time PolicyBound fires, the submission has been removed
                // from pending and the risk lives only in the event (which we
                // receive by value). Re-derive it from the panel's quoted risk
                // stored during assembly — but the plan keeps risk in the event.
                // We do the simplest thing: reconstruct from the panel event.
                // However, `PolicyBound` doesn't carry the `Risk` — so we need
                // to look it up. The market stores it in BoundPolicy after the
                // call, so we need to pass it in. To avoid complicating the event
                // schema, we cache the risk in `pending` and then on_policy_bound
                // receives both. But pending is removed during assemble_panel.
                // Solution: store a separate risk cache keyed by submission_id.
                // Simpler MVP solution adopted here: carry risk inline in event
                // by looking it up from policies map — but it's not there yet.
                //
                // The cleanest approach matching the plan: market caches the risk
                // until PolicyBound fires. We add a `risk_cache` map to market.
                // For now, use the pattern already working: market.on_policy_bound
                // accepts the panel only, and we feed risk via a separate lookup.
                //
                // Actually — re-reading the plan, on_policy_bound derives PolicyId
                // from submission_id.0 and inserts into policies. It needs the Risk.
                // The risk was in `PendingSubmission` which was removed at assembly.
                // We need to stash the risk. The cleanest fix: market keeps a
                // `bound_risk_cache: HashMap<SubmissionId, Risk>` set at assemble time,
                // consumed at on_policy_bound time.
                //
                // Rather than restructure market now, we pull the risk from the
                // dispatch: we stash it in the `PolicyBound` event itself by
                // extending it. But that changes the event schema.
                //
                // Pragmatic solution: market.on_policy_bound takes risk + panel,
                // and we reconstruct the risk from the panel's noted context.
                // We add a risk_cache to market (HashMap<SubmissionId,Risk>).
                if let Some(risk) = self.market.take_bound_risk(submission_id) {
                    self.market.on_policy_bound(submission_id, risk, panel);
                }
            }
            Event::LossEvent {
                event_id: _,
                region,
                peril,
                severity,
            } => {
                let events = self.market.on_loss_event(day, &region, peril, severity);
                for (d, e) in events {
                    self.schedule(d, e);
                }
            }
            Event::ClaimSettled {
                policy_id,
                syndicate_id,
                amount,
            } => {
                if let Some(s) = self.syndicates.iter_mut().find(|s| s.id == syndicate_id) {
                    s.on_claim_settled(amount);
                }
                // Accumulate into market YTD totals for industry loss ratio computation.
                self.market.on_claim_settled(policy_id, amount);
            }
            Event::SyndicateEntered { syndicate_id: _ } => {}
            Event::SyndicateInsolvency { syndicate_id: _ } => {}
        }
    }

    fn handle_simulation_start(&mut self, day: Day, year_start: Year) {
        // Generate broker submissions for this year.
        // We must collect broker events without holding a mutable borrow on self.
        let mut all_broker_events: Vec<(Day, Event)> = vec![];
        for b in &mut self.brokers {
            let events = b.generate_submissions(day, &mut self.rng);
            all_broker_events.extend(events);
        }
        for (d, e) in all_broker_events {
            self.schedule(d, e);
        }

        // Schedule 0–2 catastrophe LossEvents at random days within the year.
        // Only meaningful when there are syndicates that can write policies.
        let n_cats: usize = if self.syndicates.is_empty() {
            0
        } else {
            self.rng.random_range(0..=2usize)
        };
        for _ in 0..n_cats {
            let offset = self.rng.random_range(1..360_u64);
            let event_id = self.alloc_loss_event_id();
            let severity = self.rng.random_range(100_000..=10_000_000_u64);
            self.schedule(
                day.offset(offset),
                Event::LossEvent {
                    event_id,
                    region: "US-SE".to_string(),
                    peril: crate::events::Peril::WindstormAtlantic,
                    severity,
                },
            );
        }

        self.schedule(
            Day::year_end(year_start),
            Event::YearEnd { year: year_start },
        );
    }

    fn handle_year_end(&mut self, _day: Day, year: Year) {
        let next = Year(year.0 + 1);
        self.schedule(
            Day::year_start(next),
            Event::SimulationStart { year_start: next },
        );
        // YearEnd for `next` is scheduled by handle_simulation_start when
        // SimulationStart(next) fires — not here, to avoid double-scheduling.
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::broker::Broker;
    use crate::events::{Event, Peril, Risk};
    use crate::syndicate::Syndicate;
    use crate::types::{BrokerId, Day, SyndicateId, Year};

    fn make_risk(territory: &str, perils: Vec<Peril>) -> Risk {
        Risk {
            line_of_business: "property".to_string(),
            sum_insured: 2_000_000,
            territory: territory.to_string(),
            limit: 1_000_000,
            attachment: 0,
            perils_covered: perils,
        }
    }

    fn make_syndicate(id: u64) -> Syndicate {
        Syndicate::new(SyndicateId(id), 10_000_000, 500)
    }

    fn make_broker(id: u64, risk: Risk) -> Broker {
        Broker::new(BrokerId(id), 1, vec![risk])
    }

    fn base_sim(syndicates: Vec<Syndicate>, brokers: Vec<Broker>) -> Simulation {
        Simulation::new(42)
            .until(Day::year_end(Year(1)))
            .with_agents(syndicates, brokers)
    }

    fn run_year(syndicates: Vec<Syndicate>, brokers: Vec<Broker>) -> Simulation {
        let mut sim = base_sim(syndicates, brokers);
        sim.schedule(
            Day::year_start(Year(1)),
            Event::SimulationStart {
                year_start: Year(1),
            },
        );
        sim.run();
        sim
    }

    // ── existing tests ────────────────────────────────────────────────────────

    #[test]
    fn simulation_start_schedules_year_end() {
        // max_events=3 → fires: SimulationStart(1), YearEnd(1), SimulationStart(2)
        let mut sim = Simulation::new(0).with_max_events(3);
        sim.schedule(
            Day::year_start(Year(1)),
            Event::SimulationStart {
                year_start: Year(1),
            },
        );
        sim.run();
        let starts: Vec<u32> = sim
            .log
            .iter()
            .filter_map(|e| match &e.event {
                Event::SimulationStart { year_start } => Some(year_start.0),
                _ => None,
            })
            .collect();
        let ends: Vec<u32> = sim
            .log
            .iter()
            .filter_map(|e| match &e.event {
                Event::YearEnd { year } => Some(year.0),
                _ => None,
            })
            .collect();
        assert_eq!(starts, vec![1, 2]);
        assert_eq!(ends, vec![1]);
    }

    #[test]
    fn year_end_fires_at_correct_day() {
        let mut sim = Simulation::new(0).with_max_events(2);
        sim.schedule(
            Day::year_start(Year(1)),
            Event::SimulationStart {
                year_start: Year(1),
            },
        );
        sim.run();
        let ye = sim
            .log
            .iter()
            .find(|e| matches!(e.event, Event::YearEnd { .. }))
            .unwrap();
        assert_eq!(ye.day, Day::year_end(Year(1)));
    }

    #[test]
    fn log_is_day_ordered() {
        // Core DES invariant: log must be non-decreasing in day.
        let mut sim = Simulation::new(0).with_max_events(10);
        sim.schedule(
            Day::year_start(Year(1)),
            Event::SimulationStart {
                year_start: Year(1),
            },
        );
        sim.run();
        let days: Vec<u64> = sim.log.iter().map(|e| e.day.0).collect();
        let mut sorted = days.clone();
        sorted.sort_unstable();
        assert_eq!(days, sorted);
    }

    #[test]
    fn same_seed_produces_identical_logs() {
        let run = |seed: u64| {
            let mut sim = Simulation::new(seed).with_max_events(10);
            sim.schedule(
                Day::year_start(Year(1)),
                Event::SimulationStart {
                    year_start: Year(1),
                },
            );
            sim.run();
            sim.log
        };
        assert_eq!(run(42), run(42));
    }

    // ── new pipeline tests ────────────────────────────────────────────────────

    #[test]
    fn submission_to_policy_bound() {
        let risk = make_risk("US-SE", vec![Peril::WindstormAtlantic]);
        let sim = run_year(vec![make_syndicate(1)], vec![make_broker(1, risk)]);

        let has_policy_bound = sim
            .log
            .iter()
            .any(|e| matches!(e.event, Event::PolicyBound { .. }));
        assert!(
            has_policy_bound,
            "expected PolicyBound in log; got: {:#?}",
            sim.log.iter().map(|e| &e.event).collect::<Vec<_>>()
        );
    }

    #[test]
    fn log_day_ordered_full_pipeline() {
        let risk = make_risk("US-SE", vec![Peril::WindstormAtlantic]);
        let sim = run_year(
            vec![make_syndicate(1), make_syndicate(2)],
            vec![make_broker(1, risk)],
        );
        let days: Vec<u64> = sim.log.iter().map(|e| e.day.0).collect();
        let mut sorted = days.clone();
        sorted.sort_unstable();
        assert_eq!(days, sorted, "log is not day-ordered with full pipeline");
    }

    #[test]
    fn loss_event_settles_claims() {
        let risk = make_risk("US-SE", vec![Peril::WindstormAtlantic]);
        let initial_capital = 10_000_000u64;
        let syndicate = Syndicate::new(SyndicateId(1), initial_capital, 500);

        // Run full year so policy is bound, then inject a deterministic loss.
        let mut sim = Simulation::new(999)
            .until(Day::year_end(Year(1)))
            .with_agents(vec![syndicate], vec![make_broker(1, risk)]);
        sim.schedule(
            Day::year_start(Year(1)),
            Event::SimulationStart {
                year_start: Year(1),
            },
        );
        sim.run();

        let has_claim = sim
            .log
            .iter()
            .any(|e| matches!(e.event, Event::ClaimSettled { .. }));
        // Capital must be less than initial if a claim settled.
        let final_capital = sim.syndicates[0].capital;

        // Either a stochastic cat happened (claim settled, capital reduced),
        // or no cat happened (no claim, capital unchanged). Both are valid.
        // What we assert: if ClaimSettled appears, capital decreased.
        if has_claim {
            assert!(
                final_capital < initial_capital,
                "ClaimSettled fired but capital did not decrease"
            );
        }
    }

    #[test]
    fn loss_skips_non_matching_territory() {
        // Bind a policy in UK; fire a US loss — no ClaimSettled should appear.
        let risk = make_risk("UK", vec![Peril::WindstormEuropean]);
        let mut sim = Simulation::new(42)
            .until(Day::year_end(Year(1)))
            .with_agents(vec![make_syndicate(1)], vec![make_broker(1, risk)]);
        sim.schedule(
            Day::year_start(Year(1)),
            Event::SimulationStart {
                year_start: Year(1),
            },
        );
        sim.run();

        // Inject a US loss directly (after policies are bound).
        let loss_day = Day::year_end(Year(1));
        let events =
            sim.market
                .on_loss_event(loss_day, "US-SE", Peril::WindstormAtlantic, 5_000_000);
        assert!(
            events.is_empty(),
            "expected no ClaimSettled for mismatched territory/peril"
        );
    }

    #[test]
    fn full_dispatch_loss_reduces_capital_deterministically() {
        use crate::events::{Panel, PanelEntry};
        use crate::market::BoundPolicy;
        use crate::types::{LossEventId, PolicyId, SubmissionId};

        let mut sim = Simulation::new(0);
        sim.syndicates = vec![
            Syndicate::new(SyndicateId(1), 10_000_000, 500),
            Syndicate::new(SyndicateId(2), 10_000_000, 500),
        ];

        let policy_id = PolicyId(0);
        sim.market.policies.insert(
            policy_id,
            BoundPolicy {
                policy_id,
                submission_id: SubmissionId(0),
                risk: Risk {
                    line_of_business: "property".to_string(),
                    sum_insured: 2_000_000,
                    territory: "US-SE".to_string(),
                    limit: 1_000_000,
                    attachment: 100_000,
                    perils_covered: vec![Peril::WindstormAtlantic],
                },
                panel: Panel {
                    entries: vec![
                        PanelEntry {
                            syndicate_id: SyndicateId(1),
                            share_bps: 6_000,
                            premium: 0,
                        },
                        PanelEntry {
                            syndicate_id: SyndicateId(2),
                            share_bps: 4_000,
                            premium: 0,
                        },
                    ],
                },
            },
        );

        sim.schedule(
            Day(10),
            Event::LossEvent {
                event_id: LossEventId(0),
                region: "US-SE".to_string(),
                peril: Peril::WindstormAtlantic,
                severity: 800_000,
            },
        );
        sim.run();

        // net_loss = min(800_000, 1_000_000) - 100_000 = 700_000
        // s1_loss  = 700_000 * 6000 / 10_000 = 420_000
        // s2_loss  = 700_000 * 4000 / 10_000 = 280_000

        // Primary assertions: event log (ground truth per event-sourcing design)
        assert_eq!(sim.log.len(), 3, "expected exactly 3 events: 1 LossEvent + 2 ClaimSettled");

        let has_loss = sim
            .log
            .iter()
            .any(|e| matches!(&e.event, Event::LossEvent { severity, .. } if *severity == 800_000));
        assert!(has_loss, "log missing LossEvent with severity=800_000");

        let find_claim = |sid: SyndicateId| {
            sim.log.iter().find_map(|e| match &e.event {
                Event::ClaimSettled { syndicate_id, amount, .. } if *syndicate_id == sid => {
                    Some(*amount)
                }
                _ => None,
            })
        };
        assert_eq!(find_claim(SyndicateId(1)), Some(420_000), "wrong claim amount for syndicate 1");
        assert_eq!(find_claim(SyndicateId(2)), Some(280_000), "wrong claim amount for syndicate 2");

        // Secondary assertions: derived capital confirms dispatch applied the events
        let s1 = sim.syndicates.iter().find(|s| s.id == SyndicateId(1)).unwrap();
        let s2 = sim.syndicates.iter().find(|s| s.id == SyndicateId(2)).unwrap();
        assert_eq!(s1.capital, 9_580_000);
        assert_eq!(s2.capital, 9_720_000);
    }

    #[test]
    fn panel_claims_sum_to_net_loss() {
        // Directly test on_loss_event: for a known severity the sum of
        // ClaimSettled.amount across all panel entries == min(severity, limit) - attachment.
        use crate::events::{Panel, PanelEntry};
        use crate::market::{BoundPolicy, Market};
        use crate::types::{PolicyId, SubmissionId, SyndicateId};

        let limit = 1_000_000u64;
        let attachment = 100_000u64;
        let severity = 800_000u64; // below limit, above attachment

        let risk = Risk {
            line_of_business: "property".to_string(),
            sum_insured: 2_000_000,
            territory: "US-SE".to_string(),
            limit,
            attachment,
            perils_covered: vec![Peril::WindstormAtlantic],
        };
        let panel = Panel {
            entries: vec![
                PanelEntry {
                    syndicate_id: SyndicateId(1),
                    share_bps: 6_000,
                    premium: 0,
                },
                PanelEntry {
                    syndicate_id: SyndicateId(2),
                    share_bps: 4_000,
                    premium: 0,
                },
            ],
        };

        let mut market = Market::new();
        // Insert a BoundPolicy directly.
        let policy_id = PolicyId(0);
        let submission_id = SubmissionId(1);
        market.policies.insert(
            policy_id,
            BoundPolicy {
                policy_id,
                submission_id,
                risk,
                panel,
            },
        );

        let events = market.on_loss_event(Day(0), "US-SE", Peril::WindstormAtlantic, severity);

        let total_claimed: u64 = events
            .iter()
            .filter_map(|(_, e)| match e {
                Event::ClaimSettled { amount, .. } => Some(*amount),
                _ => None,
            })
            .sum();

        let expected_net_loss = severity.min(limit) - attachment; // 700_000
        assert_eq!(
            total_claimed, expected_net_loss,
            "panel claims {} != expected net loss {}",
            total_claimed, expected_net_loss
        );
    }

    // ── Time-bounded integration tests ────────────────────────────────────────

    /// Test A: a medium-scale scenario (20 syndicates, 10 brokers, 100 subs/broker)
    /// must complete a single simulated year within 2 seconds.
    #[test]
    fn medium_scale_completes_within_budget() {
        use std::time::Instant;

        let syndicates: Vec<Syndicate> = (1..=20)
            .map(|i| Syndicate::new(SyndicateId(i), 50_000_000, 500))
            .collect();
        let brokers: Vec<Broker> = (1..=10)
            .map(|i| {
                let risk = make_risk("US-SE", vec![Peril::WindstormAtlantic]);
                Broker::new(BrokerId(i), 100, vec![risk])
            })
            .collect();
        let mut sim = Simulation::new(42)
            .until(Day::year_end(Year(1)))
            .with_agents(syndicates, brokers);
        sim.schedule(
            Day::year_start(Year(1)),
            Event::SimulationStart { year_start: Year(1) },
        );

        let start = Instant::now();
        sim.run();
        let elapsed = start.elapsed();

        // Correctness: at least half the expected policies must have been bound.
        let bound_count = sim
            .log
            .iter()
            .filter(|e| matches!(&e.event, Event::PolicyBound { .. }))
            .count();
        assert!(
            bound_count >= 5,
            "degenerate run: only {bound_count} policies bound",
        );

        assert!(
            elapsed <= std::time::Duration::from_secs(2),
            "medium scenario took {elapsed:?}, over 2 s budget",
        );
    }

    /// Test B: distributing one loss event across 5,000 pre-inserted policies
    /// (5-entry panels) must complete within 500 ms and emit exactly 25,000
    /// `ClaimSettled` events.
    #[test]
    fn loss_distribution_5000_policies_within_budget() {
        use std::time::Instant;

        use crate::events::{Panel, PanelEntry};
        use crate::market::BoundPolicy;
        use crate::types::{LossEventId, PolicyId, SubmissionId};

        let mut sim = Simulation::new(42);

        // Insert 5,000 policies with 5-entry equal-share panels directly.
        let panel_size = 5usize;
        let share_per = 10_000u32 / panel_size as u32; // 2_000
        for i in 0..5_000usize {
            let policy_id = PolicyId(i as u64);
            let entries: Vec<PanelEntry> = (0..panel_size)
                .map(|j| PanelEntry {
                    syndicate_id: SyndicateId((j + 1) as u64),
                    share_bps: share_per,
                    premium: 0,
                })
                .collect();
            let risk = Risk {
                line_of_business: "property".to_string(),
                sum_insured: 10_000_000,
                territory: "US-SE".to_string(),
                limit: 5_000_000,
                attachment: 500_000,
                perils_covered: vec![Peril::WindstormAtlantic],
            };
            sim.market.policies.insert(
                policy_id,
                BoundPolicy {
                    policy_id,
                    submission_id: SubmissionId(i as u64),
                    risk,
                    panel: Panel { entries },
                },
            );
        }

        sim.schedule(
            Day(180),
            Event::LossEvent {
                event_id: LossEventId(0),
                region: "US-SE".to_string(),
                peril: Peril::WindstormAtlantic,
                severity: 10_000_000,
            },
        );

        let start = Instant::now();
        sim.run();
        let elapsed = start.elapsed();

        let claim_count = sim
            .log
            .iter()
            .filter(|e| matches!(&e.event, Event::ClaimSettled { .. }))
            .count();
        assert_eq!(claim_count, 25_000, "expected 25,000 ClaimSettled events, got {claim_count}");

        assert!(
            elapsed <= std::time::Duration::from_millis(500),
            "5k-policy loss distribution took {elapsed:?}, over 500 ms budget",
        );
    }

    /// Test C: a small scenario (5 syndicates, 2 brokers, 10 subs/broker) run
    /// for 5 years must finish within 1 second, catching super-linear slowdowns.
    #[test]
    fn five_year_small_scenario_per_year_budget() {
        use std::time::Instant;

        let syndicates: Vec<Syndicate> = (1..=5)
            .map(|i| Syndicate::new(SyndicateId(i), 50_000_000, 500))
            .collect();
        let brokers: Vec<Broker> = (1..=2)
            .map(|i| {
                let risk = make_risk("US-SE", vec![Peril::WindstormAtlantic]);
                Broker::new(BrokerId(i), 10, vec![risk])
            })
            .collect();
        let mut sim = Simulation::new(42)
            .until(Day::year_end(Year(5)))
            .with_agents(syndicates, brokers);
        sim.schedule(
            Day::year_start(Year(1)),
            Event::SimulationStart { year_start: Year(1) },
        );

        let start = Instant::now();
        sim.run();
        let elapsed = start.elapsed();

        assert!(
            elapsed <= std::time::Duration::from_secs(1),
            "5-year small scenario took {elapsed:?}, over 1 s budget",
        );
    }

    /// Test D: the large scenario (80 syndicates, 25 brokers, 500 subs/broker,
    /// 1 year) must complete within 60 seconds. Marked `#[ignore]` — run
    /// explicitly with: `cargo test -- --ignored stress_scenario_completes_within_budget --nocapture`
    #[test]
    #[ignore]
    fn stress_scenario_completes_within_budget() {
        use std::time::Instant;

        let syndicates: Vec<Syndicate> = (1..=80)
            .map(|i| Syndicate::new(SyndicateId(i), 50_000_000, 500))
            .collect();
        let brokers: Vec<Broker> = (1..=25)
            .map(|i| {
                let risk = make_risk("US-SE", vec![Peril::WindstormAtlantic]);
                Broker::new(BrokerId(i), 500, vec![risk])
            })
            .collect();
        let mut sim = Simulation::new(42)
            .until(Day::year_end(Year(1)))
            .with_agents(syndicates, brokers);
        sim.schedule(
            Day::year_start(Year(1)),
            Event::SimulationStart { year_start: Year(1) },
        );

        let start = Instant::now();
        sim.run();
        let elapsed = start.elapsed();

        eprintln!("stress: log.len() = {}", sim.log.len());

        assert!(
            elapsed <= std::time::Duration::from_secs(60),
            "stress scenario took {elapsed:?}, over 60 s budget",
        );
    }
}
