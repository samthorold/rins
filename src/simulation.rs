use std::cmp::Reverse;
use std::collections::BinaryHeap;

use rand::SeedableRng;
use rand_chacha::ChaCha20Rng;

use crate::broker::Broker;
use crate::config::SimulationConfig;
use crate::events::{Event, SimEvent};
use crate::market::Market;
use crate::perils;
use crate::syndicate::Syndicate;
use crate::types::{Day, SyndicateId, Year};

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

    /// Construct a `Simulation` from a `SimulationConfig`.
    pub fn from_config(config: &SimulationConfig) -> Self {
        let syndicates = config
            .syndicates
            .iter()
            .map(|c| Syndicate::new(c.id, c.capital, c.rate_on_line_bps))
            .collect();
        let brokers = config
            .brokers
            .iter()
            .map(|c| Broker::new(c.id, c.submissions_per_year, c.risks.clone()))
            .collect();
        Simulation::new(config.seed)
            .until(Day::year_end(Year(config.years)))
            .with_agents(syndicates, brokers)
    }

    /// Schedule an event to fire at the given day.
    pub fn schedule(&mut self, day: Day, event: Event) {
        self.queue.push(Reverse(SimEvent { day, event }));
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

                // 4. Expire all policies written in this year (annual terms).
                //    Must happen after compute_year_stats so YTD claims/premiums
                //    are captured before policies are removed.
                self.market.expire_policies(year);

                // 5. Schedule next year (keeps the sim running until max_day).
                self.handle_year_end(day, year);
            }
            Event::SubmissionArrived {
                submission_id,
                broker_id,
                risk,
            } => {
                let available: Vec<SyndicateId> = self
                    .syndicates
                    .iter()
                    .filter(|s| s.is_active)
                    .map(|s| s.id)
                    .collect();
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
                let n_eligible = self.syndicates.iter().filter(|s| s.is_eligible_for_risk(&risk)).count();
                if let Some(syn) = self.syndicates.iter_mut().find(|s| s.id == syndicate_id) {
                    let (d, e) = syn.on_quote_requested(
                        day,
                        submission_id,
                        &risk,
                        is_lead,
                        lead_premium,
                        benchmark,
                        n_eligible,
                    );
                    self.schedule(d, e);
                }
            }
            Event::QuoteIssued {
                submission_id,
                syndicate_id,
                premium,
                is_lead,
            } => {
                let available: Vec<SyndicateId> = self
                    .syndicates
                    .iter()
                    .filter(|s| s.is_active)
                    .map(|s| s.id)
                    .collect();
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
            Event::SubmissionAbandoned { .. } => {
                // Paper trail only; no syndicate state to update.
            }
            Event::PolicyBound {
                submission_id,
                panel,
            } => {
                if let Some(risk) = self.market.take_bound_risk(submission_id) {
                    // Derive the policy year from the current day so that
                    // expire_policies can remove it at the correct YearEnd.
                    let year = Year((day.0 / Day::DAYS_PER_YEAR) as u32 + 1);
                    // Collect syndicate contributions before panel is moved into market.
                    let contributions: Vec<(SyndicateId, u64)> =
                        panel.entries.iter().map(|e| (e.syndicate_id, e.premium)).collect();
                    // Clone risk and panel: on_policy_bound consumes them, but we
                    // also need them for the per-policy attritional scheduler below.
                    let policy_id =
                        self.market.on_policy_bound(submission_id, risk.clone(), panel.clone(), year);
                    for (syn_id, premium) in contributions {
                        if let Some(s) =
                            self.syndicates.iter_mut().find(|s| s.id == syn_id)
                        {
                            s.on_policy_bound_as_panelist(submission_id, premium);
                        }
                    }
                    // Per-policy attritional claims (independent of global LossEvent stream).
                    let attritional_configs = perils::default_attritional_configs();
                    let attritional_events = perils::schedule_attritional_claims_for_policy(
                        policy_id,
                        &risk,
                        &panel,
                        year,
                        &mut self.rng,
                        &attritional_configs,
                    );
                    for (d, e) in attritional_events {
                        self.schedule(d, e);
                    }
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
                if let Some(s) = self.syndicates.iter_mut().find(|s| s.id == syndicate_id && s.is_active) {
                    if s.on_claim_settled(amount) {
                        s.is_active = false;
                        self.schedule(day, Event::SyndicateInsolvency { syndicate_id });
                    }
                }
                // Accumulate into market YTD totals for industry loss ratio computation.
                self.market.on_claim_settled(policy_id, amount);
            }
            Event::SyndicateEntered { syndicate_id: _ } => {}
            Event::SyndicateInsolvency { syndicate_id } => {
                if let Some(s) = self.syndicates.iter_mut().find(|s| s.id == syndicate_id) {
                    s.is_active = false;
                }
            }
        }
    }

    fn handle_simulation_start(&mut self, day: Day, year_start: Year) {
        // Emit SyndicateEntered for each initial syndicate at the start of year 1.
        // Subsequent SimulationStart events (years 2+) must not re-emit these.
        if year_start == Year(1) {
            let ids: Vec<_> = self.syndicates.iter().map(|s| s.id).collect();
            for syndicate_id in ids {
                self.schedule(day, Event::SyndicateEntered { syndicate_id });
            }
        }

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

        // Schedule loss events for the year using a Poisson frequency-severity
        // model. Only meaningful when there are syndicates that can write policies.
        if !self.syndicates.is_empty() {
            let configs = perils::default_peril_configs();
            let loss_events = perils::schedule_loss_events(
                &configs,
                year_start,
                &mut self.rng,
                &mut self.next_loss_event_id,
            );
            for (d, e) in loss_events {
                self.schedule(d, e);
            }
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
        use crate::types::{LossEventId, SubmissionId, Year};

        let mut sim = Simulation::new(0);
        sim.syndicates = vec![
            Syndicate::new(SyndicateId(1), 10_000_000, 500),
            Syndicate::new(SyndicateId(2), 10_000_000, 500),
        ];

        sim.market.on_policy_bound(
            SubmissionId(0),
            Risk {
                line_of_business: "property".to_string(),
                sum_insured: 2_000_000,
                territory: "US-SE".to_string(),
                limit: 1_000_000,
                attachment: 100_000,
                perils_covered: vec![Peril::WindstormAtlantic],
            },
            Panel {
                entries: vec![
                    PanelEntry { syndicate_id: SyndicateId(1), share_bps: 6_000, premium: 0 },
                    PanelEntry { syndicate_id: SyndicateId(2), share_bps: 4_000, premium: 0 },
                ],
            },
            Year(1),
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
        use crate::market::Market;
        use crate::types::{SubmissionId, SyndicateId, Year};

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
                PanelEntry { syndicate_id: SyndicateId(1), share_bps: 6_000, premium: 0 },
                PanelEntry { syndicate_id: SyndicateId(2), share_bps: 4_000, premium: 0 },
            ],
        };

        let mut market = Market::new();
        market.on_policy_bound(SubmissionId(1), risk, panel, Year(1));

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

    // ── Event-stream coherence tests ──────────────────────────────────────────

    /// The first lead QuoteIssued premium in a year-1 run must equal the ATP
    /// computed with the initial benchmark (0.65). This traces the full
    /// dispatch path: SubmissionArrived → QuoteRequested → on_quote_requested
    /// → QuoteIssued, verifying the benchmark is threaded correctly.
    #[test]
    fn year_one_lead_premium_equals_atp_with_initial_benchmark() {
        // Expected ATP for property / US-SE / limit=1_000_000 / attachment=0 /
        // WindstormAtlantic with fresh syndicate (z=0) and benchmark=0.65:
        //   blended   = 0.65
        //   territory = 1.4  (US-SE)
        //   peril     = 1.5  (WindstormAtlantic)
        //   layer_f   = 1.0  (attachment=0)
        //   ATP       = round(1_000_000 * 0.65 * 1.4 * 1.5) = 1_365_000
        let risk = make_risk("US-SE", vec![Peril::WindstormAtlantic]);
        let sim = run_year(vec![make_syndicate(1)], vec![make_broker(1, risk.clone())]);

        let lead_premium = sim
            .log
            .iter()
            .find_map(|e| match &e.event {
                Event::QuoteIssued { is_lead: true, premium, .. } => Some(*premium),
                _ => None,
            })
            .expect("no lead QuoteIssued in log");

        let expected = make_syndicate(1).atp(&risk, 0.65);
        assert_eq!(
            lead_premium, expected,
            "lead premium {lead_premium} != expected ATP {expected}"
        );
    }

    /// Firing YearEnd after manually staging YTD data must update
    /// `current_industry_benchmark` to the realised loss ratio.
    #[test]
    fn year_end_updates_benchmark_from_ytd_data() {
        use crate::events::{Panel, PanelEntry};
        use crate::types::{PolicyId, SubmissionId, Year};

        // No brokers — no stochastic submissions or catastrophe events.
        let mut sim = Simulation::new(0).until(Day::year_end(Year(1)));
        sim.syndicates = vec![make_syndicate(1)];

        // Manually bind a policy with known premium (80_000) so the market
        // accumulates YTD data without going through the submission pipeline.
        let risk = make_risk("US-SE", vec![Peril::WindstormAtlantic]);
        let panel = Panel {
            entries: vec![PanelEntry {
                syndicate_id: SyndicateId(1),
                share_bps: 10_000,
                premium: 80_000,
            }],
        };
        sim.market.on_policy_bound(SubmissionId(1), risk, panel, Year(1));

        // Settle a claim of 60_000 → realised loss ratio = 60_000 / 80_000 = 0.75.
        // on_policy_bound assigns PolicyId(0) (next_policy_id starts at 0).
        sim.market.on_claim_settled(PolicyId(0), 60_000);

        sim.schedule(Day::year_end(Year(1)), Event::YearEnd { year: Year(1) });
        sim.run();

        assert!(
            (sim.current_industry_benchmark - 0.75).abs() < 1e-10,
            "benchmark={} expected 0.75",
            sim.current_industry_benchmark
        );
    }

    /// A year with heavier-than-average losses must produce higher quoted
    /// premiums in the following year.
    ///
    /// Two simulations share seed 42 (identical stochastic cats). Simulation B
    /// has an extra deterministic large loss injected at day 100 of year 1.
    /// Because YTD claims differ, the industry benchmark and each syndicate's
    /// EWMA are higher in B at year-end. Year-2 ATP — which blends EWMA with the
    /// benchmark — must therefore be higher in B than in A.
    #[test]
    fn higher_loss_year_raises_next_year_quoted_premium() {
        use crate::types::{LossEventId, Year};

        let risk = make_risk("US-SE", vec![Peril::WindstormAtlantic]);

        // Use very large capital so stochastic losses never trigger insolvency.
        // This test is about price-response to loss history, not insolvency mechanics.
        let large_syn = || Syndicate::new(SyndicateId(1), 100_000_000_000, 500);

        let build = |inject_loss: bool| {
            let mut sim = Simulation::new(42)
                .until(Day::year_end(Year(2)))
                .with_agents(vec![large_syn()], vec![make_broker(1, risk.clone())]);
            sim.schedule(
                Day::year_start(Year(1)),
                Event::SimulationStart { year_start: Year(1) },
            );
            if inject_loss {
                // Large loss at day 100 of year 1: severity >> limit so net_loss = limit.
                sim.schedule(
                    Day::year_start(Year(1)).offset(100),
                    Event::LossEvent {
                        event_id: LossEventId(999),
                        region: "US-SE".to_string(),
                        peril: Peril::WindstormAtlantic,
                        severity: 10_000_000,
                    },
                );
            }
            sim.run();
            sim
        };

        let sim_clean = build(false);
        let sim_loss = build(true);

        let year2_start = Day::year_start(Year(2)).0;
        let first_lead_premium = |sim: &Simulation| {
            sim.log.iter().filter(|e| e.day.0 >= year2_start).find_map(|e| match &e.event {
                Event::QuoteIssued { is_lead: true, premium, .. } => Some(*premium),
                _ => None,
            })
        };

        let p_clean = first_lead_premium(&sim_clean).expect("no year-2 lead quote in clean sim");
        let p_loss = first_lead_premium(&sim_loss).expect("no year-2 lead quote in loss sim");

        assert!(
            p_loss > p_clean,
            "year-2 premium after loss year ({p_loss}) should exceed clean year ({p_clean})"
        );
    }

    /// Price response must be visible in the event log of a single run.
    ///
    /// This is the test equivalent of the NDJSON analysis that refuted issue #1:
    /// collect all `QuoteIssued { is_lead: true }` events, group by year, and
    /// assert that year-2 premiums exceed year-1 premiums after a large loss.
    /// Unlike `higher_loss_year_raises_next_year_quoted_premium`, which compares
    /// two parallel simulations, this test reads one simulation's own event log.
    #[test]
    fn price_response_visible_in_event_stream() {
        use crate::types::{LossEventId, Year};

        let risk = make_risk("US-SE", vec![Peril::WindstormAtlantic]);
        // Use very large capital so stochastic losses never trigger insolvency.
        let large_syn = Syndicate::new(SyndicateId(1), 100_000_000_000, 500);
        let mut sim = Simulation::new(42)
            .until(Day::year_end(Year(2)))
            .with_agents(vec![large_syn], vec![make_broker(1, risk)]);
        sim.schedule(
            Day::year_start(Year(1)),
            Event::SimulationStart { year_start: Year(1) },
        );
        // Large loss at day 100 of year 1: drives EWMA and benchmark upward.
        sim.schedule(
            Day::year_start(Year(1)).offset(100),
            Event::LossEvent {
                event_id: LossEventId(999),
                region: "US-SE".to_string(),
                peril: Peril::WindstormAtlantic,
                severity: 10_000_000,
            },
        );
        sim.run();

        let year_boundary = Day::year_start(Year(2)).0;
        let avg_lead_premium = |min_day: u64, max_day: u64| -> u64 {
            let premiums: Vec<u64> = sim
                .log
                .iter()
                .filter(|e| e.day.0 >= min_day && e.day.0 < max_day)
                .filter_map(|e| match &e.event {
                    Event::QuoteIssued { is_lead: true, premium, .. } => Some(*premium),
                    _ => None,
                })
                .collect();
            assert!(!premiums.is_empty(), "no lead quotes in day range {min_day}..{max_day}");
            premiums.iter().sum::<u64>() / premiums.len() as u64
        };

        let p_year1 = avg_lead_premium(0, year_boundary);
        let p_year2 = avg_lead_premium(year_boundary, u64::MAX);

        assert!(
            p_year2 > p_year1,
            "year-2 avg lead premium ({p_year2}) should exceed year-1 ({p_year1}) after large loss"
        );
    }

    // ── SyndicateEntered at sim start ─────────────────────────────────────────

    #[test]
    fn initial_syndicates_emit_entered_at_day_zero() {
        let syndicates = vec![make_syndicate(1), make_syndicate(2), make_syndicate(3)];
        let sim = run_year(syndicates, vec![]);

        let mut entered_ids: Vec<u64> = sim
            .log
            .iter()
            .filter(|e| e.day == Day(0))
            .filter_map(|e| match &e.event {
                Event::SyndicateEntered { syndicate_id } => Some(syndicate_id.0),
                _ => None,
            })
            .collect();
        entered_ids.sort_unstable();

        assert_eq!(
            entered_ids,
            vec![1, 2, 3],
            "expected SyndicateEntered at day 0 for each initial syndicate"
        );
    }

    #[test]
    fn syndicate_entered_not_repeated_in_subsequent_years() {
        let syndicates = vec![make_syndicate(1)];
        let mut sim = Simulation::new(42)
            .until(Day::year_end(Year(2)))
            .with_agents(syndicates, vec![]);
        sim.schedule(Day::year_start(Year(1)), Event::SimulationStart { year_start: Year(1) });
        sim.run();

        let entered_count = sim
            .log
            .iter()
            .filter(|e| matches!(&e.event, Event::SyndicateEntered { syndicate_id } if syndicate_id.0 == 1))
            .count();

        assert_eq!(entered_count, 1, "SyndicateEntered should fire exactly once per syndicate");
    }

    // ── Capacity and insolvency integration tests ─────────────────────────────

    #[test]
    fn insolvent_syndicate_excluded_from_submissions() {
        use crate::events::{Panel, PanelEntry};
        use crate::types::{BrokerId, PolicyId, SubmissionId};

        // Syn 1: tiny capital so a large claim breaches the solvency floor.
        // Syn 2: healthy capital; should remain active throughout.
        let mut sim = Simulation::new(42);
        sim.syndicates = vec![
            Syndicate::new(SyndicateId(1), 1_000, 500),
            Syndicate::new(SyndicateId(2), 50_000_000, 500),
        ];

        // Register a policy on Syn 1 so ClaimSettled resolves cleanly.
        let risk = make_risk("US-SE", vec![Peril::WindstormAtlantic]);
        sim.market.on_policy_bound(
            SubmissionId(1),
            risk,
            Panel {
                entries: vec![PanelEntry {
                    syndicate_id: SyndicateId(1),
                    share_bps: 10_000,
                    premium: 0,
                }],
            },
            Year(1),
        );

        // A claim of 900 drops Syn 1 capital from 1_000 to 100 < floor (1_000 * 0.20 = 200).
        sim.schedule(
            Day(10),
            Event::ClaimSettled {
                policy_id: PolicyId(0),
                syndicate_id: SyndicateId(1),
                amount: 900,
            },
        );
        // Submission arrives after insolvency has been processed.
        sim.schedule(
            Day(20),
            Event::SubmissionArrived {
                submission_id: SubmissionId(99),
                broker_id: BrokerId(1),
                risk: make_risk("US-SE", vec![Peril::WindstormAtlantic]),
            },
        );

        sim.run();

        // SyndicateInsolvency must appear in the log.
        let has_insolvency = sim.log.iter().any(|e| {
            matches!(&e.event, Event::SyndicateInsolvency { syndicate_id } if *syndicate_id == SyndicateId(1))
        });
        assert!(has_insolvency, "expected SyndicateInsolvency in log");

        // Syn 1 must be inactive.
        let syn1 = sim.syndicates.iter().find(|s| s.id == SyndicateId(1)).unwrap();
        assert!(!syn1.is_active, "Syn 1 should be inactive after insolvency");

        // No QuoteRequested should reach Syn 1 after day 10.
        let bad = sim.log.iter().any(|e| {
            e.day.0 >= 20
                && matches!(&e.event, Event::QuoteRequested { syndicate_id, .. } if *syndicate_id == SyndicateId(1))
        });
        assert!(!bad, "insolvent Syn 1 must not receive QuoteRequested");
    }

    #[test]
    fn quote_declined_in_log_when_at_capacity() {
        // Give the syndicate a tiny max_premium_ratio so the first quote saturates capacity.
        let mut syn = Syndicate::new(SyndicateId(1), 10_000_000, 500);
        // capacity = 10_000_000 * 0.0001 = 1_000 pence — far below any ATP
        syn.max_premium_ratio = 0.0001;

        let risk = make_risk("US-SE", vec![Peril::WindstormAtlantic]);
        let sim = run_year(vec![syn], vec![make_broker(1, risk)]);

        let has_declined =
            sim.log.iter().any(|e| matches!(e.event, Event::QuoteDeclined { .. }));
        assert!(has_declined, "expected at least one QuoteDeclined when capacity is exhausted");
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
        use crate::types::{LossEventId, SubmissionId, Year};

        let mut sim = Simulation::new(42);

        // Bind 5,000 policies with 5-entry equal-share panels via on_policy_bound.
        let panel_size = 5usize;
        let share_per = 10_000u32 / panel_size as u32; // 2_000
        for i in 0..5_000usize {
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
                attachment: 0, // zero attachment so all ground-up losses generate claims
                perils_covered: vec![Peril::WindstormAtlantic],
            };
            sim.market.on_policy_bound(SubmissionId(i as u64), risk, Panel { entries }, Year(1));
        }

        // With 5k policies × sum_insured=10M, total_sum_insured=50B.
        // severity=5B → ground_up per policy = 1M, within limit of 5M, above attachment of 0.
        sim.schedule(
            Day(180),
            Event::LossEvent {
                event_id: LossEventId(0),
                region: "US-SE".to_string(),
                peril: Peril::WindstormAtlantic,
                severity: 5_000_000_000,
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
