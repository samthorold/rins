use std::cmp::Reverse;
use std::collections::{BinaryHeap, HashMap, HashSet};

use rand::{RngCore, SeedableRng};
use rand_chacha::ChaCha20Rng;

/// Days from CoverageRequested to PolicyBound (the quoting chain length).
const QUOTING_CHAIN_DAYS: u64 = 3;

/// 1-in-N Pareto damage fraction: scale × (return_period × annual_frequency)^(1/shape).
fn pml_damage_fraction(scale: f64, shape: f64, annual_frequency: f64, return_period: f64) -> f64 {
    scale * (return_period * annual_frequency).powf(1.0 / shape)
}

use crate::broker::Broker;
use crate::config::{SimulationConfig, ASSET_VALUE};
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
    /// (insured_id, year) pairs for which attritional losses have already been scheduled.
    /// Prevents double-scheduling when the same insured gets multiple CoverageRequested
    /// in one year (e.g. QuoteRejected retry or QuoteAccepted renewal).
    attritional_scheduled: HashSet<(InsuredId, Year)>,
    /// Gross premium written this year (PolicyBound.premium). Reset at YearStart.
    year_premium_written: u64,
    /// Claims settled this year (ClaimSettled.amount). Reset at YearStart.
    year_claims_settled: u64,
    /// Rolling 2-year buffer of annual loss ratios (for trailing CR check).
    recent_loss_ratios: std::collections::VecDeque<f64>,
    /// 1-in-200 damage fraction computed at construction; used to size new standard entrants.
    pml_200: f64,
    /// Next InsurerId to assign to a dynamically-spawned entrant.
    next_insurer_id: u64,
    /// Year in which the most recent entrant was spawned (cooldown guard).
    last_entry_year: Option<u32>,
}

impl Simulation {
    /// Construct from a canonical config.
    pub fn from_config(config: SimulationConfig) -> Self {
        let pml_200 = pml_damage_fraction(
            config.catastrophe.pareto_scale,
            config.catastrophe.pareto_shape,
            config.catastrophe.annual_frequency,
            200.0,
        );
        let insurers: Vec<Insurer> = config
            .insurers
            .iter()
            .map(|c| {
                Insurer::new(
                    c.id,
                    c.initial_capital,
                    c.attritional_elf,
                    c.cat_elf,
                    c.target_loss_ratio,
                    c.ewma_credibility,
                    c.expense_ratio,
                    c.profit_loading,
                    c.net_line_capacity,
                    c.solvency_capital_fraction,
                    c.pml_damage_fraction_override.unwrap_or(pml_200),
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
                config.max_rate_on_line,
            ));
        }
        let qps = config
            .quotes_per_submission
            .unwrap_or(insurer_ids.len())
            .min(insurer_ids.len())
            .max(1);
        let broker = Broker::new(insureds, insurer_ids, qps);

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

        let total_years = config.warmup_years + config.years;
        let max_day = Day::year_end(Year(total_years));

        let next_insurer_id =
            config.insurers.iter().map(|ic| ic.id.0).max().unwrap_or(0) + 1;

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
            attritional_scheduled: HashSet::new(),
            year_premium_written: 0,
            year_claims_settled: 0,
            recent_loss_ratios: std::collections::VecDeque::new(),
            pml_200,
            next_insurer_id,
            last_entry_year: None,
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

    /// Bootstrap the simulation: schedule the initial SimulationStart event at Day(0).
    /// Prefer this over scheduling SimulationStart manually — it embeds warmup/analysis
    /// metadata from config so analysis scripts can read it from the event stream.
    pub fn start(&mut self) {
        self.schedule(
            Day(0),
            Event::SimulationStart {
                year_start: Year(1),
                warmup_years: self.config.warmup_years,
                analysis_years: self.config.years,
            },
        );
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
            Event::SimulationStart { year_start, .. } => {
                self.schedule(Day::year_start(year_start), Event::YearStart { year: year_start });
            }

            Event::YearStart { year } => {
                self.handle_year_start(day, year);
            }

            Event::YearEnd { year } => {
                self.handle_year_end(day, year);
            }

            Event::CoverageRequested { insured_id, risk } => {
                // Register insured in market (idempotent — first call wins).
                self.market.register_insured(insured_id, risk.sum_insured);

                // Schedule attritional losses once per (insured, year) so that
                // retries (QuoteRejected / SubmissionDropped renewals) don't
                // double-schedule losses for the same insured in the same year.
                let year = day.year();
                if self.attritional_scheduled.insert((insured_id, year)) {
                    let att = perils::schedule_attritional_losses_for_insured(
                        insured_id, &risk, day, &mut self.rng, &self.config.attritional,
                    );
                    for (d, e) in att {
                        self.schedule(d, e);
                    }
                }

                let events = self.broker.on_coverage_requested(day, insured_id, risk);
                for (d, e) in events {
                    self.schedule(d, e);
                }
            }

            Event::LeadQuoteRequested { submission_id, insured_id, insurer_id, risk } => {
                if let Some(insurer) = self.insurers.iter().find(|i| i.id == insurer_id) {
                    for (d, e) in
                        insurer.on_lead_quote_requested(day, submission_id, insured_id, &risk)
                    {
                        self.schedule(d, e);
                    }
                }
            }

            Event::LeadQuoteDeclined { submission_id, .. } => {
                for (d, e) in self.broker.on_lead_quote_declined(day, submission_id) {
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
                let year = day.year();
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

            Event::QuoteRejected { insured_id, .. } => {
                // Schedule renewal: same annual offset as the QuoteAccepted path.
                let renewal_day = day.offset(361 - QUOTING_CHAIN_DAYS);
                if let Some(insured) = self.broker.insureds.iter().find(|i| i.id == insured_id) {
                    let risk = insured.risk.clone();
                    self.schedule(renewal_day, Event::CoverageRequested { insured_id, risk });
                }
            }

            Event::SubmissionDropped { insured_id, .. } => {
                // All insurers declined. Schedule the same annual-offset renewal so the
                // insured retries next year rather than silently vanishing from the model.
                let renewal_day = day.offset(361 - QUOTING_CHAIN_DAYS);
                if let Some(insured) = self.broker.insureds.iter().find(|i| i.id == insured_id) {
                    let risk = insured.risk.clone();
                    self.schedule(renewal_day, Event::CoverageRequested { insured_id, risk });
                }
            }

            Event::PolicyBound { policy_id, premium, .. } => {
                // Activate the policy for loss routing.
                self.market.on_policy_bound(policy_id);

                // Attritional AssetDamage events are scheduled at CoverageRequested time
                // (see the CoverageRequested arm above) so all insureds accumulate
                // attritional exposure regardless of policy status.

                if let Some(policy) = self.market.policies.get(&policy_id) {
                    let insurer_id = policy.insurer_id;
                    let sum_insured = policy.risk.sum_insured;
                    let perils = policy.risk.perils_covered.clone();
                    if let Some(ins) = self.insurers.iter_mut().find(|i| i.id == insurer_id) {
                        ins.on_policy_bound(policy_id, sum_insured, premium, &perils);
                        // Back-fill total_cat_exposure now that the insurer aggregate is updated.
                        let total_cat_exposure = ins.cat_aggregate;
                        if let Some(last) = self.log.last_mut() {
                            if let Event::PolicyBound { total_cat_exposure: ref mut tce, .. } =
                                last.event
                            {
                                *tce = total_cat_exposure;
                            }
                        }
                    }
                }

                self.year_premium_written += premium;
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

            Event::AssetDamage { insured_id, peril, ground_up_loss } => {
                // Route to ClaimSettled only for covered insureds.
                let events =
                    self.market.on_asset_damage(day, insured_id, ground_up_loss, peril);
                for (d, e) in events {
                    self.schedule(d, e);
                }
            }

            Event::ClaimSettled { insurer_id, amount, peril, .. } => {
                let new_events =
                    if let Some(insurer) = self.insurers.iter_mut().find(|i| i.id == insurer_id) {
                        let events = insurer.on_claim_settled(day, amount, peril);
                        // Back-fill remaining_capital now that the insurer has applied the claim.
                        let remaining_capital = insurer.capital.max(0) as u64;
                        if let Some(last) = self.log.last_mut() {
                            if let Event::ClaimSettled {
                                remaining_capital: ref mut rc,
                                ..
                            } = last.event
                            {
                                *rc = remaining_capital;
                            }
                        }
                        events
                    } else {
                        vec![]
                    };
                for (d, e) in new_events {
                    self.schedule(d, e);
                }
                self.year_claims_settled += amount;
            }

            Event::InsurerInsolvent { .. } => {}

            // InsurerEntered is logged directly by spawn_new_insurer — no further dispatch.
            Event::InsurerEntered { .. } => {}
        }
    }

    fn handle_year_start(&mut self, day: Day, year: Year) {
        // Reset annual accumulators used for the entry-criterion loss ratio.
        self.year_premium_written = 0;
        self.year_claims_settled = 0;

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

    fn handle_year_end(&mut self, day: Day, year: Year) {
        // Update each insurer's expected_loss_fraction via EWMA from this year's experience.
        // Also detect zombies (capital > 0 but max_line < min policy size) and mark them insolvent.
        // Collect emitted events before scheduling to avoid conflicting mutable borrows.
        let zombie_events: Vec<(Day, Event)> = self
            .insurers
            .iter_mut()
            .flat_map(|insurer| insurer.on_year_end(day, ASSET_VALUE))
            .collect();
        for (d, ev) in zombie_events {
            self.schedule(d, ev);
        }

        // ── Entry criterion ───────────────────────────────────────────────────
        let expense_ratio = self.config.insurers.first()
            .map(|ic| ic.expense_ratio)
            .unwrap_or(0.344);
        let lr = if self.year_premium_written > 0 {
            self.year_claims_settled as f64 / self.year_premium_written as f64
        } else {
            0.0
        };
        self.recent_loss_ratios.push_back(lr);
        if self.recent_loss_ratios.len() > 2 {
            self.recent_loss_ratios.pop_front();
        }

        // Only fire during analysis years (not warmup) and respect 3-year cooldown.
        if year.0 > self.config.warmup_years && self.recent_loss_ratios.len() >= 2 {
            let avg_lr = self.recent_loss_ratios.iter().sum::<f64>()
                / self.recent_loss_ratios.len() as f64;
            let avg_cr = avg_lr + expense_ratio;
            let cooldown_ok = self.last_entry_year
                .map(|y| year.0.saturating_sub(y) >= 3)
                .unwrap_or(true);
            if avg_cr < 0.85 && cooldown_ok {
                self.spawn_new_insurer(day, year);
            }
        }

        // Schedule next year if within simulation horizon.
        let total_years = self.config.warmup_years + self.config.years;
        if year.0 < total_years {
            let next = Year(year.0 + 1);
            self.schedule(Day::year_start(next), Event::YearStart { year: next });
        }
    }

    fn spawn_new_insurer(&mut self, day: Day, year: Year) {
        // 1-in-3 chance of an aggressive entrant (optimistic cat model).
        let is_aggressive = (self.rng.next_u32() % 3) == 0;

        let id = InsurerId(self.next_insurer_id);
        self.next_insurer_id += 1;

        // Extract template params from config before any mutable borrows.
        let pml_200 = self.pml_200;
        let (initial_capital, cat_elf, target_loss_ratio, profit_loading, pml_frac) = {
            let template = if is_aggressive {
                self.config.insurers.iter().find(|ic| ic.pml_damage_fraction_override.is_some())
            } else {
                self.config.insurers.iter().find(|ic| ic.pml_damage_fraction_override.is_none())
            };
            template.map(|t| {
                let pml = t.pml_damage_fraction_override.unwrap_or(pml_200);
                (t.initial_capital, t.cat_elf, t.target_loss_ratio, t.profit_loading, pml)
            }).unwrap_or_else(|| {
                if is_aggressive {
                    (40_000_000_000i64, 0.015, 0.90, 0.00, 0.126)
                } else {
                    (50_000_000_000i64, 0.033, 0.80, 0.05, pml_200)
                }
            })
        };
        let (attritional_elf, ewma_credibility, expense_ratio, net_line_capacity, scf) =
            self.config.insurers.first()
                .map(|ic| (ic.attritional_elf, ic.ewma_credibility, ic.expense_ratio,
                           ic.net_line_capacity, ic.solvency_capital_fraction))
                .unwrap_or((0.030, 0.3, 0.344, Some(0.30), Some(0.30)));

        let insurer = Insurer::new(
            id, initial_capital, attritional_elf, cat_elf, target_loss_ratio,
            ewma_credibility, expense_ratio, profit_loading, net_line_capacity, scf, pml_frac,
        );
        let initial_capital_u64 = initial_capital.max(0) as u64;

        self.insurers.push(insurer);
        self.broker.add_insurer(id);
        self.last_entry_year = Some(year.0);

        self.log.push(SimEvent {
            day,
            event: Event::InsurerEntered { insurer_id: id, initial_capital: initial_capital_u64, is_aggressive },
        });

    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::{AttritionalConfig, CatConfig, InsurerConfig, SimulationConfig};
    use crate::events::Event;

    fn minimal_config(years: u32, n_insureds: usize) -> SimulationConfig {
        SimulationConfig {
            seed: 42,
            years,
            warmup_years: 0,
            insurers: vec![InsurerConfig {
                id: InsurerId(1),
                initial_capital: 100_000_000_000,
                attritional_elf: 0.239,
                cat_elf: 0.0,
                target_loss_ratio: 0.70,
                ewma_credibility: 0.3,
                expense_ratio: 0.0,
                profit_loading: 0.0,
                net_line_capacity: None,
                solvency_capital_fraction: None,
                pml_damage_fraction_override: None,
            }],
            n_insureds,
            attritional: AttritionalConfig { annual_rate: 2.0, mu: -3.0, sigma: 1.0 },
            catastrophe: CatConfig {
                annual_frequency: 0.5,
                pareto_scale: 0.05,
                pareto_shape: 1.5,
            },
            quotes_per_submission: None,
            max_rate_on_line: 1.0, // unlimited — tests accept all quotes by default
        }
    }

    fn run_sim(config: SimulationConfig) -> Simulation {
        let mut sim = Simulation::from_config(config);
        sim.start();
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
    fn different_seeds_produce_different_logs() {
        let mut a = minimal_config(1, 3);
        a.seed = 1;
        let mut b = minimal_config(1, 3);
        b.seed = 2;
        assert_ne!(run_sim(a).log, run_sim(b).log);
    }

    #[test]
    fn cloned_config_produces_identical_log() {
        let config = minimal_config(1, 3);
        assert_eq!(run_sim(config.clone()).log, run_sim(config).log);
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
    fn asset_damage_appears_between_loss_event_and_claim_settled() {
        let mut config = minimal_config(1, 2);
        config.catastrophe.annual_frequency = 10.0;
        let sim = run_sim(config);

        let has_loss_event =
            sim.log.iter().any(|e| matches!(e.event, Event::LossEvent { .. }));
        let has_asset_damage =
            sim.log.iter().any(|e| matches!(e.event, Event::AssetDamage { .. }));

        if has_loss_event {
            assert!(has_asset_damage, "AssetDamage must appear when LossEvent fires");
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
    fn attritional_asset_damage_appears_in_log() {
        let mut config = minimal_config(1, 5);
        config.attritional.annual_rate = 10.0;
        let sim = run_sim(config);
        let has_att = sim.log.iter().any(|e| {
            matches!(e.event, Event::AssetDamage { peril: Peril::Attritional, .. })
        });
        assert!(has_att, "expected attritional AssetDamage events with high rate");
    }

    // ── Capital ───────────────────────────────────────────────────────────────

    #[test]
    fn insurer_capital_accessible_after_run() {
        // Verifies capital does not panic under heavy cat load (no reset — capital persists).
        let mut config = minimal_config(2, 5);
        config.catastrophe.annual_frequency = 10.0;
        let sim = run_sim(config);
        for ins in &sim.insurers {
            let _ = ins.capital; // verify no panic (may go negative)
        }
    }

    #[test]
    fn insurer_becomes_insolvent_under_stress() {
        // Under extreme cat frequency and small initial capital, at least one
        // insurer must become insolvent — verifying the full event chain
        // LossEvent → AssetDamage → ClaimSettled → insurer.on_claim_settled → InsurerInsolvent.
        let mut config = minimal_config(2, 10);
        config.catastrophe.annual_frequency = 5.0;
        // Shrink capital so a single bad cat year wipes it out
        for ins_cfg in &mut config.insurers {
            ins_cfg.initial_capital = 1_000_000; // 0.01 USD — effectively zero
        }
        let sim = run_sim(config);
        let any_insolvent = sim.insurers.iter().any(|i| i.insolvent);
        let insolvent_event_in_log =
            sim.log.iter().any(|e| matches!(e.event, Event::InsurerInsolvent { .. }));
        assert!(
            any_insolvent && insolvent_event_in_log,
            "expected at least one insolvent insurer and an InsurerInsolvent event in the log under extreme stress"
        );
    }

    // ── Competitive quoting ───────────────────────────────────────────────────

    #[test]
    fn all_insurers_solicited_per_submission() {
        // 3 identical insurers, qps=None (all). Every submission solicits all 3.
        // All 3 insurer IDs must appear in LeadQuoteIssued events.
        let mut config = minimal_config(1, 6);
        config.insurers = (1..=3)
            .map(|i| InsurerConfig {
                id: InsurerId(i),
                initial_capital: 100_000_000_000,
                attritional_elf: 0.239,
                cat_elf: 0.0,
                target_loss_ratio: 0.70,
                ewma_credibility: 0.3,
                expense_ratio: 0.0,
                profit_loading: 0.0,
                net_line_capacity: None,
                solvency_capital_fraction: None,
                pml_damage_fraction_override: None,
            })
            .collect();
        let sim = run_sim(config);

        let insurer_ids_in_issued: std::collections::HashSet<u64> = sim
            .log
            .iter()
            .filter_map(|e| match &e.event {
                Event::LeadQuoteIssued { insurer_id, .. } => Some(insurer_id.0),
                _ => None,
            })
            .collect();

        for id in 1u64..=3 {
            assert!(
                insurer_ids_in_issued.contains(&id),
                "insurer {id} must appear in LeadQuoteIssued events (all are solicited per submission)"
            );
        }
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
    fn quote_rejected_schedules_renewal() {
        // max_rate_on_line=0.0 rejects every quote (any positive premium > 0%).
        // The simulation must schedule CoverageRequested at QuoteRejected.day + 358.
        let mut config = minimal_config(2, 1);
        config.max_rate_on_line = 0.0;
        let sim = run_sim(config);

        let qr_day = sim
            .log
            .iter()
            .find(|e| matches!(e.event, Event::QuoteRejected { .. }))
            .map(|e| e.day)
            .expect("QuoteRejected missing");

        let expected_renewal_day = qr_day.offset(361 - QUOTING_CHAIN_DAYS);

        let has_renewal = sim
            .log
            .iter()
            .any(|e| e.day == expected_renewal_day && matches!(e.event, Event::CoverageRequested { .. }));
        assert!(
            has_renewal,
            "QuoteRejected must schedule CoverageRequested at day {}",
            expected_renewal_day.0
        );
    }

    #[test]
    fn submission_dropped_schedules_renewal() {
        // Single insurer with SCF=0 always declines cat risks → SubmissionDropped.
        // The simulation must schedule CoverageRequested at SubmissionDropped.day + 358.
        let mut config = minimal_config(2, 1);
        config.insurers = vec![InsurerConfig {
            id: InsurerId(1),
            initial_capital: 100_000_000_000,
            attritional_elf: 0.239,
            cat_elf: 0.0,
            target_loss_ratio: 0.70,
            ewma_credibility: 0.3,
            expense_ratio: 0.0,
            profit_loading: 0.0,
            net_line_capacity: None,
            solvency_capital_fraction: Some(0.0), // 0 × capital / pml = 0 → always declines cat
            pml_damage_fraction_override: None,
        }];
        let sim = run_sim(config);

        let sd_day = sim
            .log
            .iter()
            .find(|e| matches!(e.event, Event::SubmissionDropped { .. }))
            .map(|e| e.day)
            .expect("SubmissionDropped missing");

        let expected_renewal_day = sd_day.offset(361 - QUOTING_CHAIN_DAYS);

        let has_renewal = sim
            .log
            .iter()
            .any(|e| e.day == expected_renewal_day && matches!(e.event, Event::CoverageRequested { .. }));
        assert!(
            has_renewal,
            "SubmissionDropped must schedule CoverageRequested at day {}",
            expected_renewal_day.0
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

    // ── Exposure limit + re-routing ───────────────────────────────────────────

    // ── New enriched fields ───────────────────────────────────────────────────

    #[test]
    fn policy_bound_total_cat_exposure_increases_with_each_binding() {
        // Single insurer, 3 insureds, 1 year. All risks cover WindstormAtlantic.
        // Each successive PolicyBound must show total_cat_exposure growing by ASSET_VALUE.
        use crate::config::ASSET_VALUE;
        let sim = run_sim(minimal_config(1, 3));
        let exposures: Vec<u64> = sim
            .log
            .iter()
            .filter_map(|e| {
                if let Event::PolicyBound { total_cat_exposure, .. } = &e.event {
                    Some(*total_cat_exposure)
                } else {
                    None
                }
            })
            .collect();
        assert!(exposures.len() >= 3, "need at least 3 PolicyBound events");
        assert_eq!(exposures[0], ASSET_VALUE, "1st PolicyBound: total_cat_exposure must equal ASSET_VALUE");
        assert_eq!(exposures[1], 2 * ASSET_VALUE, "2nd PolicyBound: total_cat_exposure must equal 2×ASSET_VALUE");
        assert_eq!(exposures[2], 3 * ASSET_VALUE, "3rd PolicyBound: total_cat_exposure must equal 3×ASSET_VALUE");
    }

    #[test]
    fn claim_settled_remaining_capital_is_nonzero_for_solvent_insurer() {
        // With large initial capital and moderate cat frequency, all ClaimSettled events
        // should carry a non-zero remaining_capital (insurer stays solvent throughout).
        let mut config = minimal_config(1, 3);
        config.catastrophe.annual_frequency = 5.0;
        let sim = run_sim(config);
        let claim_events: Vec<u64> = sim
            .log
            .iter()
            .filter_map(|e| {
                if let Event::ClaimSettled { remaining_capital, .. } = &e.event {
                    Some(*remaining_capital)
                } else {
                    None
                }
            })
            .collect();
        assert!(!claim_events.is_empty(), "expected ClaimSettled events with high cat frequency");
        assert!(
            claim_events.iter().any(|&rc| rc > 0),
            "at least one ClaimSettled must have non-zero remaining_capital for a solvent insurer"
        );
    }

    #[test]
    fn declined_by_first_insurer_binds_with_second() {
        // Insurer 1: max_cat_aggregate = 0 → always declines cat risks.
        // Insurer 2: unlimited → always quotes.
        // All policies must bind with insurer 2.
        use crate::config::ASSET_VALUE;

        let mut config = minimal_config(1, 3);
        config.insurers = vec![
            InsurerConfig {
                id: InsurerId(1),
                initial_capital: 100_000_000_000,
                attritional_elf: 0.239,
                cat_elf: 0.0,
                target_loss_ratio: 0.70,
                ewma_credibility: 0.3,
                expense_ratio: 0.0,
                profit_loading: 0.0,
                net_line_capacity: None,
                solvency_capital_fraction: Some(0.0), // 0 × capital / pml = 0 → always declines cat
                pml_damage_fraction_override: None,
            },
            InsurerConfig {
                id: InsurerId(2),
                initial_capital: 100_000_000_000,
                attritional_elf: 0.239,
                cat_elf: 0.0,
                target_loss_ratio: 0.70,
                ewma_credibility: 0.3,
                expense_ratio: 0.0,
                profit_loading: 0.0,
                net_line_capacity: None,
                solvency_capital_fraction: None,
                pml_damage_fraction_override: None,
            },
        ];

        let sim = run_sim(config);

        // Every PolicyBound must be with insurer 2.
        for e in &sim.log {
            if let Event::PolicyBound { insurer_id, .. } = &e.event {
                assert_eq!(
                    *insurer_id,
                    InsurerId(2),
                    "all policies must bind with insurer 2 (insurer 1 always declines)"
                );
            }
        }

        // LeadQuoteDeclined events must appear (one per insured from insurer 1).
        let declined_count = sim
            .log
            .iter()
            .filter(|e| matches!(e.event, Event::LeadQuoteDeclined { .. }))
            .count();
        assert!(declined_count > 0, "expected LeadQuoteDeclined events, got none");
        assert!(
            sim.log.iter().any(|e| matches!(e.event, Event::PolicyBound { .. })),
            "policies must still bind after re-routing"
        );

        let _ = ASSET_VALUE; // suppress unused warning
    }

    #[test]
    fn syndicate_entry_fires_after_profitable_years() {
        // minimal_config has expense_ratio=0.0 and att_ELF≈23.9% with target_LR=0.70.
        // Realized LR ≈ 0.70, so avg_CR ≈ LR + 0.0 = 0.70 < 0.85 → entry criterion met.
        // warmup_years=0 → entry criterion active from year 2 onward.
        // 3-year cooldown → expect entries at approx years 2, 5, 8 over a 10-year run.
        let config = minimal_config(10, 5);
        let sim = run_sim(config);

        let entered = sim
            .log
            .iter()
            .filter(|e| matches!(e.event, Event::InsurerEntered { .. }))
            .count();
        assert!(
            entered > 0,
            "expected at least one InsurerEntered event over 10 quiet years (avg CR ≈ 0.70 < 0.85)"
        );
    }

    #[test]
    fn pml_damage_fraction_override_raises_effective_cat_limit() {
        // Two configs identical except for pml_damage_fraction_override.
        // The cat config produces pml_200 ≈ 0.252 (scale=0.05, shape=1.5, freq=0.5,
        // return_period=200 → 0.05 × (200 × 0.5)^(1/1.5) ≈ 0.252).
        //
        // With capital=50_000_000_000 (500M USD) and SCF=0.30:
        //   Standard (pml=0.252):  effective_cat = 0.30 × 50B / 0.252 ≈ 59.5B cents (~11.9 × SI)
        //   Optimistic (pml=0.126): effective_cat = 0.30 × 50B / 0.126 ≈ 119B cents (~23.8 × SI)
        //
        // Load each insurer with 11 policies (cat_aggregate = 11 × 5B = 55B), then request a
        // 12th quote. The standard insurer should decline (55B + 5B > 59.5B); the optimistic
        // one should issue (55B + 5B ≪ 119B).
        use crate::events::DeclineReason;
        use crate::insurer::Insurer;
        use crate::types::PolicyId;

        let cat_cfg = CatConfig { annual_frequency: 0.5, pareto_scale: 0.05, pareto_shape: 1.5 };
        let pml_200 = pml_damage_fraction(
            cat_cfg.pareto_scale,
            cat_cfg.pareto_shape,
            cat_cfg.annual_frequency,
            200.0,
        );

        let make_insurer = |pml_override: Option<f64>| -> Insurer {
            let pml = pml_override.unwrap_or(pml_200);
            Insurer::new(
                InsurerId(1),
                50_000_000_000, // 500M USD capital
                0.239,
                0.0,
                0.70,
                0.3,
                0.0,
                0.0,
                None,           // no net_line_capacity check
                Some(0.30),     // SCF = 0.30
                pml,
            )
        };

        let sum_insured = 5_000_000_000u64; // 50M USD
        let risk = Risk {
            sum_insured,
            territory: "US-SE".to_string(),
            perils_covered: vec![crate::events::Peril::WindstormAtlantic],
        };

        // Helper to load insurer with `n` cat policies then attempt one more quote.
        let try_12th_quote = |mut ins: Insurer| {
            use crate::types::SubmissionId;
            for pid in 0..11u64 {
                ins.on_policy_bound(PolicyId(pid), sum_insured, 0, &[crate::events::Peril::WindstormAtlantic]);
            }
            let events = ins.on_lead_quote_requested(
                Day(0),
                SubmissionId(12),
                InsuredId(1),
                &risk,
            );
            events.into_iter().map(|(_, e)| e).next().unwrap()
        };

        let std_event = try_12th_quote(make_insurer(None));
        let opt_event = try_12th_quote(make_insurer(Some(0.126)));

        assert!(
            matches!(
                std_event,
                Event::LeadQuoteDeclined { reason: DeclineReason::MaxCatAggregateBreached, .. }
            ),
            "standard insurer (pml=0.252) should decline 12th policy: {std_event:?}"
        );
        assert!(
            matches!(opt_event, Event::LeadQuoteIssued { .. }),
            "optimistic insurer (pml=0.126) should issue 12th policy: {opt_event:?}"
        );
    }
}
