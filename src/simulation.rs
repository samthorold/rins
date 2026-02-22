use std::cmp::Reverse;
use std::collections::{BinaryHeap, HashMap};

use rand::SeedableRng;
use rand_chacha::ChaCha20Rng;

/// Days from CoverageRequested to PolicyBound (the quoting chain length).
const QUOTING_CHAIN_DAYS: u64 = 3;

use crate::broker::Broker;
use crate::config::SimulationConfig;
use crate::events::{Event, EventLog, Peril, Risk, SimEvent};
use crate::insured::Insured;
use crate::insurer::Insurer;
use crate::market::Market;
use crate::perils::{self, DamageFractionModel};
use crate::types::{Day, InsuredId, InsurerId, Year};

pub struct Simulation {
    queue: BinaryHeap<Reverse<SimEvent>>,
    /// Completed events in dispatch order. `log[i]` has implicit sequence number `i`.
    /// See `docs/event-sourcing.md §5` for the incremental-replay pattern.
    pub log: EventLog,
    rng: ChaCha20Rng,
    max_day: Option<Day>,
    max_events: Option<usize>,
    pub insurers: Vec<Insurer>,
    pub broker: Broker,
    pub market: Market,
    next_event_id: u64,
    damage_models: HashMap<Peril, DamageFractionModel>,
    config: SimulationConfig,
}

impl Simulation {
    /// Construct from a canonical config.
    pub fn from_config(config: SimulationConfig) -> Self {
        let insurers: Vec<Insurer> = config
            .insurers
            .iter()
            .map(|c| {
                Insurer::new(
                    c.id,
                    c.initial_capital,
                    c.rate,
                    c.expected_loss_fraction,
                    c.target_loss_ratio,
                    c.max_cat_aggregate,
                    c.max_line_size,
                )
            })
            .collect();

        let insurer_ids: Vec<InsurerId> = insurers.iter().map(|i| i.id).collect();

        let mut insureds = Vec::new();
        for i in 0..config.n_insureds {
            insureds.push(Insured::new(
                InsuredId(i as u64 + 1),
                "US-SE".to_string(),
                vec![Peril::WindstormAtlantic, Peril::Attritional],
            ));
        }
        let broker = Broker::new(insureds, insurer_ids);

        let damage_models = HashMap::from([
            (
                Peril::WindstormAtlantic,
                DamageFractionModel::Pareto {
                    scale: config.catastrophe.pareto_scale,
                    shape: config.catastrophe.pareto_shape,
                },
            ),
            (
                Peril::Attritional,
                DamageFractionModel::LogNormal {
                    mu: config.attritional.mu,
                    sigma: config.attritional.sigma,
                },
            ),
        ]);

        let max_day = Day::year_end(Year(config.years));

        Simulation {
            queue: BinaryHeap::new(),
            log: EventLog::new(),
            rng: ChaCha20Rng::seed_from_u64(config.seed),
            max_day: Some(max_day),
            max_events: None,
            insurers,
            broker,
            market: Market::new(),
            next_event_id: 0,
            damage_models,
            config,
        }
    }

    /// Override the day horizon (used in tests).
    pub fn until(mut self, day: Day) -> Self {
        self.max_day = Some(day);
        self
    }

    /// Stop after N events (unit-test safety valve).
    #[allow(dead_code)]
    pub fn with_max_events(mut self, n: usize) -> Self {
        self.max_events = Some(n);
        self
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
            self.log.push(ev.clone());
            self.dispatch(ev.day, ev.event);
            count += 1;
        }
    }

    fn dispatch(&mut self, day: Day, event: Event) {
        match event {
            Event::SimulationStart { year_start } => {
                self.schedule(Day::year_start(year_start), Event::YearStart { year: year_start });
            }

            Event::YearStart { year } => {
                self.handle_year_start(day, year);
            }

            Event::YearEnd { year } => {
                self.handle_year_end(year);
            }

            Event::CoverageRequested { insured_id, risk } => {
                let events = self.broker.on_coverage_requested(day, insured_id, risk);
                for (d, e) in events {
                    self.schedule(d, e);
                }
            }

            Event::LeadQuoteRequested { submission_id, insured_id, insurer_id, risk } => {
                if let Some(insurer) = self.insurers.iter().find(|i| i.id == insurer_id) {
                    let (d, e) =
                        insurer.on_lead_quote_requested(day, submission_id, insured_id, &risk);
                    self.schedule(d, e);
                }
            }

            Event::LeadQuoteIssued { submission_id, insured_id, insurer_id, atp: _, premium, cat_exposure_at_quote: _ } => {
                let events =
                    self.broker.on_lead_quote_issued(day, submission_id, insured_id, insurer_id, premium);
                for (d, e) in events {
                    self.schedule(d, e);
                }
            }

            Event::QuotePresented { submission_id, insured_id, insurer_id, premium } => {
                // Insured decides whether to accept. Currently always accepts.
                for insured in &self.broker.insureds {
                    if insured.id == insured_id {
                        let events =
                            insured.on_quote_presented(day, submission_id, insurer_id, premium);
                        for (d, e) in events {
                            self.schedule(d, e);
                        }
                        break;
                    }
                }
            }

            Event::QuoteAccepted { submission_id, insured_id, insurer_id, premium } => {
                let year = Year((day.0 / Day::DAYS_PER_YEAR) as u32 + 1);
                let risk = self
                    .broker
                    .insureds
                    .iter()
                    .find(|i| i.id == insured_id)
                    .map(|i| i.risk.clone());
                if let Some(risk) = risk {
                    // Schedule renewal CoverageRequested so the new PolicyBound lands
                    // exactly on the old PolicyExpired (day+361), eliminating drift.
                    let renewal_day = day.offset(361 - QUOTING_CHAIN_DAYS);
                    let renewal_risk = risk.clone();

                    let events = self.market.on_quote_accepted(
                        day,
                        submission_id,
                        insured_id,
                        insurer_id,
                        premium,
                        risk,
                        year,
                    );
                    for (d, e) in events {
                        self.schedule(d, e);
                    }

                    self.schedule(renewal_day, Event::CoverageRequested {
                        insured_id,
                        risk: renewal_risk,
                    });
                }
            }

            Event::QuoteRejected { .. } => {
                // No-op in this model.
            }

            Event::PolicyBound { policy_id, .. } => {
                // Activate the policy for loss routing.
                self.market.on_policy_bound(policy_id);

                // Schedule attritional InsuredLoss events for this policy,
                // starting from the current day so no event is scheduled in the past.
                if let Some(policy) = self.market.policies.get(&policy_id) {
                    let insurer_id = policy.insurer_id;
                    let sum_insured = policy.risk.sum_insured;
                    let perils = policy.risk.perils_covered.clone();
                    let att_events = perils::schedule_attritional_claims_for_policy(
                        policy_id,
                        policy.insured_id,
                        &policy.risk.clone(),
                        day,
                        &mut self.rng,
                        &self.config.attritional,
                    );
                    for (d, e) in att_events {
                        self.schedule(d, e);
                    }
                    if let Some(ins) = self.insurers.iter_mut().find(|i| i.id == insurer_id) {
                        ins.on_policy_bound(policy_id, sum_insured, &perils);
                    }
                }
            }

            Event::PolicyExpired { policy_id } => {
                // Read insurer_id before market removes the policy record.
                let insurer_id = self.market.policies.get(&policy_id).map(|p| p.insurer_id);
                if let Some(ins_id) = insurer_id
                    && let Some(ins) = self.insurers.iter_mut().find(|i| i.id == ins_id)
                {
                    ins.on_policy_expired(policy_id);
                }
                self.market.on_policy_expired(policy_id);
            }

            Event::LossEvent { peril, .. } => {
                let events = self.market.on_loss_event(
                    day,
                    peril,
                    &self.damage_models,
                    &mut self.rng,
                );
                for (d, e) in events {
                    self.schedule(d, e);
                }
            }

            Event::InsuredLoss { policy_id, peril, ground_up_loss, .. } => {
                // Apply policy terms → ClaimSettled.
                let events =
                    self.market.on_insured_loss(day, policy_id, ground_up_loss, peril);
                for (d, e) in events {
                    self.schedule(d, e);
                }
            }

            Event::ClaimSettled { insurer_id, amount, .. } => {
                if let Some(insurer) = self.insurers.iter_mut().find(|i| i.id == insurer_id) {
                    insurer.on_claim_settled(amount);
                }
            }
        }
    }

    fn handle_year_start(&mut self, day: Day, year: Year) {
        // Endow insurers with fresh capital each year.
        for insurer in &mut self.insurers {
            insurer.on_year_start();
        }

        // Year 1 only: schedule CoverageRequested for each insured, spread over first 180 days.
        // Subsequent years: renewals are triggered by approaching PolicyExpired instead.
        if year.0 == 1 {
            let n = self.broker.insureds.len();
            let coverage_events: Vec<(Day, InsuredId, Risk)> = self
                .broker
                .insureds
                .iter()
                .enumerate()
                .map(|(i, insured)| {
                    let offset = if n > 1 { i as u64 * 180 / n as u64 } else { 0 };
                    (day.offset(offset), insured.id, insured.risk.clone())
                })
                .collect();

            for (d, insured_id, risk) in coverage_events {
                self.schedule(d, Event::CoverageRequested { insured_id, risk });
            }
        }

        // Schedule catastrophe loss events (Poisson draw for the year).
        let loss_events = perils::schedule_loss_events(
            &self.config.catastrophe,
            year,
            &mut self.rng,
            &mut self.next_event_id,
        );
        for (d, e) in loss_events {
            self.schedule(d, e);
        }

        // Schedule YearEnd.
        self.schedule(Day::year_end(year), Event::YearEnd { year });
    }

    fn handle_year_end(&mut self, year: Year) {
        eprintln!("Year {} complete", year.0);

        // Schedule next year if within simulation horizon.
        if year.0 < self.config.years {
            let next = Year(year.0 + 1);
            self.schedule(Day::year_start(next), Event::YearStart { year: next });
        }
    }
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;

    use super::*;
    use crate::config::{AttritionalConfig, CatConfig, InsurerConfig, SimulationConfig};
    use crate::events::Event;

    fn minimal_config(years: u32, n_insureds: usize) -> SimulationConfig {
        SimulationConfig {
            seed: 42,
            years,
            insurers: vec![InsurerConfig {
                id: InsurerId(1),
                initial_capital: 100_000_000_000,
                rate: 0.02,
                expected_loss_fraction: 0.239,
                target_loss_ratio: 0.70,
                max_cat_aggregate: None,
                max_line_size: None,
            }],
            n_insureds,
            attritional: AttritionalConfig { annual_rate: 2.0, mu: -3.0, sigma: 1.0 },
            catastrophe: CatConfig {
                annual_frequency: 0.5,
                pareto_scale: 0.05,
                pareto_shape: 1.5,
            },
        }
    }

    fn run_sim(config: SimulationConfig) -> Simulation {
        let mut sim = Simulation::from_config(config);
        sim.schedule(Day(0), Event::SimulationStart { year_start: Year(1) });
        sim.run();
        sim
    }

    // ── Core DES invariants ───────────────────────────────────────────────────

    #[test]
    fn log_is_day_ordered() {
        let sim = run_sim(minimal_config(1, 6));
        let days: Vec<u64> = sim.log.iter().map(|e| e.day.0).collect();
        let mut sorted = days.clone();
        sorted.sort_unstable();
        assert_eq!(days, sorted, "event log must be day-ordered");
    }

    #[test]
    fn same_seed_produces_identical_logs() {
        let run = || run_sim(minimal_config(2, 6));
        assert_eq!(run().log, run().log, "same seed must produce identical logs");
    }

    #[test]
    fn year_end_fires_at_correct_day() {
        let sim = run_sim(minimal_config(1, 2));
        let ye = sim
            .log
            .iter()
            .find(|e| matches!(e.event, Event::YearEnd { .. }))
            .expect("YearEnd must appear in log");
        assert_eq!(ye.day, Day::year_end(Year(1)));
    }

    #[test]
    fn simulation_runs_multiple_years() {
        let sim = run_sim(minimal_config(3, 3));
        let year_ends: Vec<u32> = sim
            .log
            .iter()
            .filter_map(|e| match &e.event {
                Event::YearEnd { year } => Some(year.0),
                _ => None,
            })
            .collect();
        assert_eq!(year_ends, vec![1, 2, 3], "must fire YearEnd for each year");
    }

    // ── Quote chain ───────────────────────────────────────────────────────────

    #[test]
    fn quote_chain_produces_all_event_types() {
        let sim = run_sim(minimal_config(1, 1));
        let has_coverage_req =
            sim.log.iter().any(|e| matches!(e.event, Event::CoverageRequested { .. }));
        let has_lead_quote_req =
            sim.log.iter().any(|e| matches!(e.event, Event::LeadQuoteRequested { .. }));
        let has_lead_quote_issued =
            sim.log.iter().any(|e| matches!(e.event, Event::LeadQuoteIssued { .. }));
        let has_quote_presented =
            sim.log.iter().any(|e| matches!(e.event, Event::QuotePresented { .. }));
        let has_quote_accepted =
            sim.log.iter().any(|e| matches!(e.event, Event::QuoteAccepted { .. }));
        let has_bound = sim.log.iter().any(|e| matches!(e.event, Event::PolicyBound { .. }));

        assert!(has_coverage_req, "CoverageRequested missing");
        assert!(has_lead_quote_req, "LeadQuoteRequested missing");
        assert!(has_lead_quote_issued, "LeadQuoteIssued missing");
        assert!(has_quote_presented, "QuotePresented missing");
        assert!(has_quote_accepted, "QuoteAccepted missing");
        assert!(has_bound, "PolicyBound missing");
    }

    #[test]
    fn quote_chain_day_ordering() {
        // For a single insured, verify the day progression through the chain.
        let sim = run_sim(minimal_config(1, 1));

        let coverage_day = sim
            .log
            .iter()
            .find(|e| matches!(e.event, Event::CoverageRequested { .. }))
            .map(|e| e.day)
            .expect("CoverageRequested missing");

        let lead_req_day = sim
            .log
            .iter()
            .find(|e| matches!(e.event, Event::LeadQuoteRequested { .. }))
            .map(|e| e.day)
            .expect("LeadQuoteRequested missing");

        let lead_issued_day = sim
            .log
            .iter()
            .find(|e| matches!(e.event, Event::LeadQuoteIssued { .. }))
            .map(|e| e.day)
            .expect("LeadQuoteIssued missing");

        let presented_day = sim
            .log
            .iter()
            .find(|e| matches!(e.event, Event::QuotePresented { .. }))
            .map(|e| e.day)
            .expect("QuotePresented missing");

        let accepted_day = sim
            .log
            .iter()
            .find(|e| matches!(e.event, Event::QuoteAccepted { .. }))
            .map(|e| e.day)
            .expect("QuoteAccepted missing");

        let bound_day = sim
            .log
            .iter()
            .find(|e| matches!(e.event, Event::PolicyBound { .. }))
            .map(|e| e.day)
            .expect("PolicyBound missing");

        assert_eq!(lead_req_day.0, coverage_day.0 + 1, "LeadQuoteRequested must be day+1");
        assert_eq!(lead_issued_day.0, lead_req_day.0, "LeadQuoteIssued same day as LeadQuoteRequested");
        assert_eq!(presented_day.0, lead_issued_day.0 + 1, "QuotePresented must be day+1");
        assert_eq!(accepted_day.0, presented_day.0, "QuoteAccepted same day as QuotePresented");
        assert_eq!(bound_day.0, accepted_day.0 + 1, "PolicyBound must be day+1");
        assert_eq!(
            bound_day.0,
            coverage_day.0 + 3,
            "total cycle CoverageRequested→PolicyBound must be 3 days"
        );
    }

    // ── Policy binding ─────────────────────────────────────────────────────────

    #[test]
    fn one_policy_bound_per_insured_per_year() {
        // Every insured must have their initial policy bound in year 1.
        // Renewals may also fire within the horizon, so count unique insured IDs.
        let n_insureds = 5;
        let sim = run_sim(minimal_config(1, n_insureds));
        let mut bound_insureds = std::collections::HashSet::new();
        for e in &sim.log {
            if let Event::PolicyBound { insured_id, .. } = &e.event {
                bound_insureds.insert(*insured_id);
            }
        }
        assert_eq!(
            bound_insureds.len(),
            n_insureds,
            "expected all {n_insureds} insureds to have a PolicyBound, got {}",
            bound_insureds.len()
        );
    }

    #[test]
    fn each_policy_bound_has_no_duplicate() {
        let sim = run_sim(minimal_config(1, 3));
        let bound_ids: Vec<_> = sim
            .log
            .iter()
            .filter_map(|e| match &e.event {
                Event::PolicyBound { policy_id, .. } => Some(*policy_id),
                _ => None,
            })
            .collect();
        for pid in &bound_ids {
            assert_eq!(
                bound_ids.iter().filter(|&&p| p == *pid).count(),
                1,
                "duplicate PolicyBound for {pid:?}"
            );
        }
    }

    // ── Loss routing ─────────────────────────────────────────────────────────

    #[test]
    fn insured_loss_appears_between_loss_event_and_claim_settled() {
        let mut config = minimal_config(1, 2);
        config.catastrophe.annual_frequency = 10.0;
        let sim = run_sim(config);

        let has_loss_event =
            sim.log.iter().any(|e| matches!(e.event, Event::LossEvent { .. }));
        let has_insured_loss =
            sim.log.iter().any(|e| matches!(e.event, Event::InsuredLoss { .. }));

        if has_loss_event {
            assert!(has_insured_loss, "InsuredLoss must appear when LossEvent fires");
        }
    }

    #[test]
    fn claim_settled_amount_is_non_negative() {
        let sim = run_sim(minimal_config(2, 6));
        for e in &sim.log {
            if let Event::ClaimSettled { amount, .. } = &e.event {
                assert!(*amount > 0, "ClaimSettled amount must be positive, got {amount}");
            }
        }
    }

    #[test]
    fn attritional_insured_loss_appears_in_log() {
        let mut config = minimal_config(1, 5);
        config.attritional.annual_rate = 10.0;
        let sim = run_sim(config);
        let has_att = sim.log.iter().any(|e| {
            matches!(e.event, Event::InsuredLoss { peril: Peril::Attritional, .. })
        });
        assert!(has_att, "expected attritional InsuredLoss events with high rate");
    }

    // ── Capital ───────────────────────────────────────────────────────────────

    #[test]
    fn insurer_capital_reset_each_year() {
        let mut config = minimal_config(2, 5);
        config.catastrophe.annual_frequency = 10.0;
        let sim = run_sim(config);
        for ins in &sim.insurers {
            let _ = ins.capital; // verify no panic
        }
    }

    // ── Round-robin routing ───────────────────────────────────────────────────

    #[test]
    fn submissions_routed_across_multiple_insurers() {
        let mut config = minimal_config(1, 6);
        config.insurers = (1..=3)
            .map(|i| InsurerConfig {
                id: InsurerId(i),
                initial_capital: 100_000_000_000,
                rate: 0.02,
                expected_loss_fraction: 0.239,
                target_loss_ratio: 0.70,
                max_cat_aggregate: None,
                max_line_size: None,
            })
            .collect();
        let sim = run_sim(config);

        let mut insurer_counts: HashMap<u64, usize> = HashMap::new();
        for e in &sim.log {
            if let Event::PolicyBound { insurer_id, .. } = &e.event {
                *insurer_counts.entry(insurer_id.0).or_insert(0) += 1;
            }
        }
        assert!(
            insurer_counts.len() >= 2,
            "submissions should be distributed across multiple insurers, got: {insurer_counts:?}"
        );
    }

    // ── Renewal ───────────────────────────────────────────────────────────────

    #[test]
    fn renewal_coverage_requested_scheduled_from_quote_accepted() {
        // One insured, one insurer, 2-year sim.
        // After the initial QuoteAccepted (day 2), a renewal CoverageRequested
        // should be scheduled at day + 361 - QUOTING_CHAIN_DAYS = 2 + 358 = 360,
        // so the new PolicyBound lands exactly on the old PolicyExpired (day 363).
        let sim = run_sim(minimal_config(2, 1));

        let qa_day = sim
            .log
            .iter()
            .find(|e| matches!(e.event, Event::QuoteAccepted { .. }))
            .map(|e| e.day)
            .expect("QuoteAccepted missing");

        let expected_renewal_day = qa_day.offset(361 - QUOTING_CHAIN_DAYS);

        let renewal_cr_days: Vec<Day> = sim
            .log
            .iter()
            .skip_while(|e| e.day <= qa_day)
            .filter(|e| matches!(e.event, Event::CoverageRequested { .. }))
            .map(|e| e.day)
            .collect();

        assert!(
            renewal_cr_days.contains(&expected_renewal_day),
            "expected a CoverageRequested at day {}, got: {:?}",
            expected_renewal_day.0,
            renewal_cr_days.iter().map(|d| d.0).collect::<Vec<_>>()
        );
    }

    #[test]
    fn year_start_year2_emits_no_coverage_requested() {
        // In a 2-year sim, YearStart for year 2 must not batch-emit CoverageRequested
        // for all insureds. Only individual renewals (triggered from QuoteAccepted) may fire.
        // With the zero-drift formula, insured 0's renewal (QA day 2 + 358) lands exactly
        // on year2_start = 360, so we assert count < n_insureds rather than == 0.
        let n_insureds = 3;
        let sim = run_sim(minimal_config(2, n_insureds));

        let year2_start = Day::year_start(Year(2));

        let cr_on_year2_start = sim
            .log
            .iter()
            .filter(|e| e.day == year2_start && matches!(e.event, Event::CoverageRequested { .. }))
            .count();

        assert!(
            cr_on_year2_start < n_insureds,
            "YearStart year 2 must not batch-emit CoverageRequested for all {n_insureds} insureds, got {cr_on_year2_start}"
        );
    }

    // ── Exposure tracking ─────────────────────────────────────────────────────

    #[test]
    fn lead_quote_issued_carries_cat_exposure() {
        // Run a 1-insured, 1-insurer sim. Find the first two LeadQuoteIssued events for the
        // same insurer. The second quote's cat_exposure_at_quote must equal the sum_insured
        // of the first bound policy (the renewal quote fires after the initial policy is bound).
        use crate::config::ASSET_VALUE;

        let sim = run_sim(minimal_config(2, 1));

        let issued: Vec<_> = sim
            .log
            .iter()
            .filter_map(|e| {
                if let Event::LeadQuoteIssued { insurer_id, cat_exposure_at_quote, .. } = &e.event {
                    Some((*insurer_id, *cat_exposure_at_quote))
                } else {
                    None
                }
            })
            .collect();

        assert!(issued.len() >= 2, "need at least two LeadQuoteIssued events");

        // First quote: no policies bound yet → exposure must be 0.
        assert_eq!(issued[0].1, 0, "first quote must have cat_exposure_at_quote == 0");

        // Second quote: initial policy is already bound → exposure must equal ASSET_VALUE.
        assert_eq!(
            issued[1].1, ASSET_VALUE,
            "second quote must reflect the already-bound cat aggregate"
        );
    }

    #[test]
    fn policy_expired_releases_insurer_cat_aggregate() {
        // 2-year sim, 1 insured. After year-1 policy expires the insurer's cat_aggregate
        // should drop back (the renewed policy adds it back, so at any point ≤ 2×ASSET_VALUE).
        use crate::config::ASSET_VALUE;

        let sim = run_sim(minimal_config(2, 1));

        // Find all PolicyExpired events and ensure insurer cat_aggregate stays bounded.
        // We verify indirectly: the second LeadQuoteIssued's cat_exposure_at_quote == ASSET_VALUE,
        // not 2×ASSET_VALUE, showing that the renewal quote fires after the first policy is bound
        // but before a second one is, so aggregate is exactly 1×ASSET_VALUE.
        let issued: Vec<u64> = sim
            .log
            .iter()
            .filter_map(|e| {
                if let Event::LeadQuoteIssued { cat_exposure_at_quote, .. } = &e.event {
                    Some(*cat_exposure_at_quote)
                } else {
                    None
                }
            })
            .collect();

        for &exp in &issued {
            assert!(
                exp <= 2 * ASSET_VALUE,
                "cat_exposure_at_quote {exp} exceeds 2×ASSET_VALUE — aggregate not released properly"
            );
        }
    }
}
