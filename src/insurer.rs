use std::collections::HashMap;

use crate::events::{DeclineReason, Event, Peril, Risk};
use crate::types::{Day, InsuredId, InsurerId, PolicyId, SubmissionId, YearAccumulator};

/// A single insurer in the minimal property market.
/// Writes 100% of each risk it quotes (lead-only, no follow market).
/// Capital is endowed once at construction and persists year-over-year; premiums add, claims deduct.
pub struct Insurer {
    pub id: InsurerId,
    /// Current capital (unsigned floor at zero; cannot go negative).
    pub capital: i64,
    /// Set to true the first time a claim drives capital to zero.
    /// An insolvent insurer declines all new quote requests but continues settling claims.
    pub insolvent: bool,
    /// Actuarial channel: E[attritional_loss] / sum_insured.
    /// Updated each YearEnd via EWMA from realized attritional burning cost.
    attritional_elf: f64,
    /// Actuarial channel: E[cat_loss] / sum_insured.
    /// Anchored — never updated from experience. Derived from the cat model.
    /// A quiet cat period is not evidence of a lower rate; EWMA would produce systematic
    /// soft-market erosion. Mirrors Lloyd's MS3 Technical Premium requirements.
    cat_elf: f64,
    /// Actuarial channel: ATP = (attritional_elf + cat_elf) / target_loss_ratio.
    target_loss_ratio: f64,
    /// EWMA credibility weight α: new_att_elf = α × realized_att_lf + (1-α) × old_att_elf.
    ewma_credibility: f64,
    /// Fraction of gross premium consumed by acquisition costs + overhead.
    expense_ratio: f64,
    /// Multiplicative loading above ATP: premium = ATP × (1 + profit_loading).
    profit_loading: f64,
    /// Year-to-date premium and claims accumulators; reset at each YearEnd.
    ytd: YearAccumulator,
    /// Exposure management: live WindstormAtlantic aggregate sum_insured.
    pub cat_aggregate: u64,
    /// Fraction of current capital committable to a single risk net line (None = unlimited).
    net_line_capacity: Option<f64>,
    /// Fraction of capital for the 1-in-200 cat scenario (None = unlimited).
    solvency_capital_fraction: Option<f64>,
    /// Pareto 1-in-200 damage fraction derived from cat model at construction.
    pml_damage_fraction_200: f64,
    /// Map from policy_id to its WindstormAtlantic sum_insured, for release on expiry.
    cat_policy_map: HashMap<PolicyId, u64>,
    /// Capital at construction — used to compute depletion ratio.
    initial_capital: i64,
    /// Sensitivity of capital-depletion adjustment: cap_depletion_adj = depletion × sensitivity.
    depletion_sensitivity: f64,
    /// Sensitivity of cat-aggregate utilisation adjustment.
    /// capacity_adj = clamp(utilisation × capacity_sensitivity, 0.0, 0.20)
    capacity_sensitivity: f64,
    /// EWMA of per-insurer annual combined ratios (α = 1/3, 5-year span).
    own_cr_ewma: Option<f64>,
    /// Number of years of own experience accumulated; drives credibility weight.
    own_years: u32,
    /// Exposure-weighted moving average of ytd.exposure (α = 0.3, ~3-year span).
    /// Used to compute vol_weight: dampens EWMA updates when current book is much smaller
    /// than the historical norm (e.g., after cat-year capacity depletion).
    exposure_ewma: f64,
    /// Multiplier on own_cr_signal before adding to own_factor.
    /// 1.0 = neutral (canonical). Randomised at entry U(0, 2.5).
    cr_sensitivity: f64,
    /// Per-insurer floor on market blend weight; replaces the hardcoded MARKET_FLOOR_WEIGHT.
    /// 0.30 = canonical. Randomised at entry U(0, 0.60).
    market_weight_floor: f64,
    /// Minimum own_ap_tp_factor at which this insurer writes a full line (pricing_line = 1.0).
    /// pricing_line = clamp((own_factor - floor_factor) / (1 - floor_factor), 0, 1).
    /// 0.0 = always full line (tests); 0.85 = canonical.
    floor_factor: f64,
    /// Fraction of annual underwriting profit distributed to Names at YearEnd.
    /// 0.0 = no distributions (tests). 0.70 = canonical.
    payout_ratio: f64,
    /// Multiplier on initial_capital that forms the distribution floor.
    /// Distributions are only paid when post-distribution capital ≥ initial_capital × multiple.
    /// 1.0 = current floor (distribute whenever capital ≥ initial_capital, tests).
    /// 1.5 = canonical — insurer must build a 50% surplus buffer before paying Names.
    distribution_floor_multiple: f64,
    /// Maximum fraction the lead writes as its own stamp.
    /// When this insurer acts as lead, capacity_line is capped at this value.
    /// Canonical: 0.25. Tests use 1.0 (preserve existing solo-writer behaviour).
    leader_participation_cap: f64,
}

/// EWMA smoothing factor for the per-insurer combined-ratio signal.
/// α = 2/(5+1) = 1/3 — equivalent to a 5-year exponentially-weighted span.
const OWN_CR_EWMA_ALPHA: f64 = 1.0 / 3.0;

impl Insurer {
    pub fn new(
        id: InsurerId,
        initial_capital: i64,
        attritional_elf: f64,
        cat_elf: f64,
        target_loss_ratio: f64,
        ewma_credibility: f64,
        expense_ratio: f64,
        profit_loading: f64,
        net_line_capacity: Option<f64>,
        solvency_capital_fraction: Option<f64>,
        pml_damage_fraction_200: f64,
        depletion_sensitivity: f64,
        capacity_sensitivity: f64,
        cr_sensitivity: f64,
        market_weight_floor: f64,
        floor_factor: f64,
        payout_ratio: f64,
        distribution_floor_multiple: f64,
        leader_participation_cap: f64,
    ) -> Self {
        Insurer {
            id,
            capital: initial_capital,
            insolvent: false,
            attritional_elf,
            cat_elf,
            target_loss_ratio,
            ewma_credibility,
            expense_ratio,
            profit_loading,
            ytd: YearAccumulator::default(),
            cat_aggregate: 0,
            net_line_capacity,
            solvency_capital_fraction,
            pml_damage_fraction_200,
            cat_policy_map: HashMap::new(),
            initial_capital,
            depletion_sensitivity,
            capacity_sensitivity,
            own_cr_ewma: None,
            own_years: 0,
            exposure_ewma: 0.0,
            cr_sensitivity,
            market_weight_floor,
            floor_factor,
            payout_ratio,
            distribution_floor_multiple,
            leader_participation_cap,
        }
    }

    /// Returns the insurer's CR sensitivity parameter (for observability).
    pub fn cr_sensitivity(&self) -> f64 { self.cr_sensitivity }

    /// Returns the insurer's capacity sensitivity parameter (for observability).
    pub fn capacity_sensitivity(&self) -> f64 { self.capacity_sensitivity }

    /// Returns the insurer's market weight floor (for observability).
    pub fn market_weight_floor(&self) -> f64 { self.market_weight_floor }

    /// Returns the insurer's own combined-ratio EWMA (for tests and observability).
    pub fn own_cr_ewma(&self) -> Option<f64> { self.own_cr_ewma }

    /// Called at each YearStart. Capital is NOT reset — it persists from prior year.
    pub fn on_year_start(&mut self) {}

    /// Price and issue a lead quote for a risk, or decline if an exposure limit is breached.
    /// Returns a single `LeadQuoteIssued` or `LeadQuoteDeclined` event.
    /// `market_ap_tp_factor`: coordinator-published AP/TP ratio; 1.0 = neutral.
    pub fn on_lead_quote_requested(
        &self,
        day: Day,
        submission_id: SubmissionId,
        insured_id: InsuredId,
        risk: &Risk,
        market_ap_tp_factor: f64,
    ) -> Vec<(Day, Event)> {
        if self.insolvent {
            return vec![(
                day,
                Event::LeadQuoteDeclined {
                    submission_id,
                    insured_id,
                    insurer_id: self.id,
                    reason: DeclineReason::Insolvent,
                },
            )];
        }
        if let Some(nlc) = self.net_line_capacity {
            let effective_line_limit = (nlc * self.capital.max(0) as f64) as u64;
            if risk.sum_insured > effective_line_limit {
                return vec![(
                    day,
                    Event::LeadQuoteDeclined {
                        submission_id,
                        insured_id,
                        insurer_id: self.id,
                        reason: DeclineReason::MaxLineSizeExceeded,
                    },
                )];
            }
        }
        if let Some(scf) = self.solvency_capital_fraction {
            let effective_cat_limit =
                (scf * self.capital.max(0) as f64 / self.pml_damage_fraction_200) as u64;
            if risk.perils_covered.contains(&Peril::WindstormAtlantic)
                && self.cat_aggregate + risk.sum_insured > effective_cat_limit
            {
                return vec![(
                    day,
                    Event::LeadQuoteDeclined {
                        submission_id,
                        insured_id,
                        insurer_id: self.id,
                        reason: DeclineReason::MaxCatAggregateBreached,
                    },
                )];
            }
        }
        let atp = self.actuarial_price(risk);
        let premium = self.underwriter_premium(risk, market_ap_tp_factor);
        let cat_exposure_at_quote = if risk.perils_covered.contains(&Peril::WindstormAtlantic) {
            self.cat_aggregate
        } else {
            0
        };
        let line_size = self.compute_line_size(risk, market_ap_tp_factor, true);
        vec![(
            day,
            Event::LeadQuoteIssued {
                submission_id,
                insured_id,
                insurer_id: self.id,
                atp,
                premium,
                cat_exposure_at_quote,
                line_size,
            },
        )]
    }

    /// Price-check a follower solicitation and issue or decline same day.
    ///
    /// Followers write at `lead_premium` (no independent pricing); the only gating checks are:
    /// 1. Insolvency
    /// 2. Net line capacity (single-risk exposure limit)
    /// 3. Cat aggregate (portfolio concentration limit)
    /// 4. TP check: if `lead_premium < own_tp` → `RateBelowTP`
    ///
    /// If all checks pass, `FollowerQuoteIssued` is emitted with capacity_line only
    /// (no `leader_participation_cap` and no `pricing_line` — followers take what they can).
    pub fn on_follower_quote_requested(
        &self,
        day: Day,
        submission_id: SubmissionId,
        insured_id: InsuredId,
        risk: &Risk,
        lead_premium: u64,
        _lead_atp: u64,
    ) -> Vec<(Day, Event)> {
        use crate::events::{DeclineReason, Event};

        if self.insolvent {
            return vec![(
                day,
                Event::FollowerQuoteDeclined {
                    submission_id,
                    insured_id,
                    insurer_id: self.id,
                    reason: DeclineReason::Insolvent,
                },
            )];
        }
        if let Some(nlc) = self.net_line_capacity {
            let effective_line_limit = (nlc * self.capital.max(0) as f64) as u64;
            if risk.sum_insured > effective_line_limit {
                return vec![(
                    day,
                    Event::FollowerQuoteDeclined {
                        submission_id,
                        insured_id,
                        insurer_id: self.id,
                        reason: DeclineReason::MaxLineSizeExceeded,
                    },
                )];
            }
        }
        if let Some(scf) = self.solvency_capital_fraction {
            let effective_cat_limit =
                (scf * self.capital.max(0) as f64 / self.pml_damage_fraction_200) as u64;
            if risk.perils_covered.contains(&Peril::WindstormAtlantic)
                && self.cat_aggregate + risk.sum_insured > effective_cat_limit
            {
                return vec![(
                    day,
                    Event::FollowerQuoteDeclined {
                        submission_id,
                        insured_id,
                        insurer_id: self.id,
                        reason: DeclineReason::MaxCatAggregateBreached,
                    },
                )];
            }
        }
        // TP check: follower only participates if the lead's rate ≥ own Technical Premium.
        let own_tp = (self.actuarial_price(risk) as f64 * (1.0 + self.profit_loading)).round() as u64;
        if lead_premium < own_tp {
            return vec![(
                day,
                Event::FollowerQuoteDeclined {
                    submission_id,
                    insured_id,
                    insurer_id: self.id,
                    reason: DeclineReason::RateBelowTP,
                },
            )];
        }
        // Followers write at capacity only; no leader_participation_cap, no pricing_line.
        let line_size = if let Some(nlc) = self.net_line_capacity {
            (nlc * self.capital.max(0) as f64 / risk.sum_insured as f64)
                .min(1.0)
                .max(0.0)
        } else {
            1.0
        };
        vec![(
            day,
            Event::FollowerQuoteIssued {
                submission_id,
                insured_id,
                insurer_id: self.id,
                line_size,
            },
        )]
    }

    /// Compute the fractional line this insurer will write on a risk.
    ///
    /// ```text
    /// raw_cap       = min(net_line_capacity * capital / sum_insured, 1.0)   (or 1.0 if no limit)
    /// capacity_line = if is_lead { raw_cap.min(leader_participation_cap) } else { raw_cap }
    /// pricing_line  = clamp((own_ap_tp_factor - floor_factor) / (1 - floor_factor), 0.0, 1.0)
    /// line_size     = min(capacity_line, pricing_line)
    /// ```
    fn compute_line_size(&self, risk: &Risk, market_ap_tp_factor: f64, is_lead: bool) -> f64 {
        let raw_cap = if let Some(nlc) = self.net_line_capacity {
            let dollar_limit = nlc * self.capital.max(0) as f64;
            (dollar_limit / risk.sum_insured as f64).min(1.0).max(0.0)
        } else {
            1.0
        };
        let capacity_line = if is_lead {
            raw_cap.min(self.leader_participation_cap)
        } else {
            raw_cap
        };

        let own_factor = self.own_ap_tp_factor(market_ap_tp_factor);
        let pricing_line = if self.floor_factor >= 1.0 {
            0.0 // floor_factor = 1.0 would cause division by zero; treat as always-zero
        } else {
            ((own_factor - self.floor_factor) / (1.0 - self.floor_factor)).clamp(0.0, 1.0)
        };

        capacity_line.min(pricing_line)
    }

    /// A policy has been bound. Credit this insurer's share of the net premium to capital,
    /// accumulate written exposure for EWMA; update cat aggregate scaled by line_share.
    pub fn on_policy_bound(
        &mut self,
        policy_id: PolicyId,
        sum_insured: u64,
        premium: u64,
        perils: &[Peril],
        line_share: f64,
    ) {
        let premium_share = (premium as f64 * line_share).round() as u64;
        let net_premium = (premium_share as f64 * (1.0 - self.expense_ratio)).round() as i64;
        self.capital += net_premium;
        let exposure_share = (sum_insured as f64 * line_share).round() as u64;
        self.ytd.exposure += exposure_share;
        self.ytd.premium += premium_share;
        if perils.contains(&Peril::WindstormAtlantic) {
            self.cat_aggregate += exposure_share;
            self.cat_policy_map.insert(policy_id, exposure_share);
        }
    }

    /// A policy has expired. Release its WindstormAtlantic aggregate contribution.
    pub fn on_policy_expired(&mut self, policy_id: PolicyId) {
        if let Some(sum_insured) = self.cat_policy_map.remove(&policy_id) {
            self.cat_aggregate = self.cat_aggregate.saturating_sub(sum_insured);
        }
    }

    /// Actuarial channel: (attritional_elf + cat_elf) × sum_insured / target_loss_ratio.
    /// cat_elf is anchored; attritional_elf drifts via EWMA.
    fn actuarial_price(&self, risk: &Risk) -> u64 {
        let elf = self.attritional_elf + self.cat_elf;
        (elf * risk.sum_insured as f64 / self.target_loss_ratio).round() as u64
    }

    /// Blend market factor with per-insurer capital state and loss history.
    ///
    /// Market weight starts at 1.0 for new entrants (no own experience) and falls as credibility
    /// accumulates, but never below `market_weight_floor`. This ensures that even mature insurers
    /// remain anchored to the market signal — mirroring how Lloyd's syndicates observe competitor
    /// pricing and PMD benchmarks regardless of own history.
    ///
    /// `credibility = min(own_years / 5, 1.0)`
    /// `market_weight = max(1 − credibility, market_weight_floor)`
    fn own_ap_tp_factor(&self, market_factor: f64) -> f64 {
        let credibility = (self.own_years as f64 / 5.0).min(1.0);
        let market_weight = (1.0 - credibility).max(self.market_weight_floor);

        let depletion = if self.initial_capital > 0 {
            (1.0 - self.capital as f64 / self.initial_capital as f64).max(0.0)
        } else {
            0.0
        };
        let cap_depletion_adj = (depletion * self.depletion_sensitivity).clamp(0.0, 0.30);

        let own_cr_signal = match self.own_cr_ewma {
            None => 0.0,
            Some(ewma_cr) => (ewma_cr - 1.0).clamp(-0.10, 0.80),
        };

        // Cat-aggregate utilisation: how full is the book relative to the SCF-based limit?
        // Fires only when solvency_capital_fraction is set (None = unlimited, adj = 0).
        let cat_utilisation = if let Some(scf) = self.solvency_capital_fraction {
            let effective_cat_limit =
                scf * self.capital.max(0) as f64 / self.pml_damage_fraction_200;
            if effective_cat_limit > 0.0 {
                (self.cat_aggregate as f64 / effective_cat_limit).min(1.0)
            } else {
                1.0
            }
        } else {
            0.0
        };
        let capacity_adj = (cat_utilisation * self.capacity_sensitivity).clamp(0.0, 0.20);

        let own_factor = 1.0 + (own_cr_signal * self.cr_sensitivity) + cap_depletion_adj + capacity_adj;
        (1.0 - market_weight) * own_factor + market_weight * market_factor
    }

    /// Underwriter channel: TP × own_ap_tp_factor (blend of market signal and own state).
    /// TP = ATP × (1 + profit_loading) — the per-insurer Technical Premium.
    fn underwriter_premium(&self, risk: &Risk, market_ap_tp_factor: f64) -> u64 {
        let tp = self.actuarial_price(risk) as f64 * (1.0 + self.profit_loading);
        (tp * self.own_ap_tp_factor(market_ap_tp_factor)).round() as u64
    }

    /// Deduct a settled claim from capital (floored at zero).
    /// Only attritional claims are accumulated for the EWMA; cat claims are excluded
    /// because cat_elf is anchored and not updated from experience.
    /// Returns `InsurerInsolvent` on the first crossing to zero; empty otherwise.
    pub fn on_claim_settled(&mut self, day: Day, amount: u64, peril: Peril) -> Vec<(Day, Event)> {
        let payable = amount.min(self.capital.max(0) as u64);
        self.capital -= payable as i64; // floors at 0 naturally
        if peril == Peril::Attritional {
            self.ytd.attritional_claims += payable;
        }
        self.ytd.total_claims += payable;

        if self.capital == 0 && !self.insolvent {
            self.insolvent = true;
            vec![(day, Event::InsurerInsolvent { insurer_id: self.id })]
        } else {
            vec![]
        }
    }

    /// Update attritional_elf via EWMA from this year's realized attritional burning cost,
    /// then reset YTD accumulators. cat_elf is never updated. No-op if no exposure written.
    /// Also detects "zombie" state: capital > 0 but max_line < min_sum_insured — the insurer
    /// can no longer write any new business. Marks it insolvent and emits InsurerInsolvent.
    pub fn on_year_end(&mut self, day: Day, min_sum_insured: u64) -> Vec<(Day, Event)> {
        // Volume weight: scale EWMA updates by current-year book size relative to the historical
        // norm. Prevents a brief period of low volume (e.g., post-cat market exit by competitors
        // forcing this insurer to also write fewer policies) from producing enormous EWMA swings
        // from an unrepresentative sample.
        // vol_weight = 1.0 on first year (no prior reference) and whenever ytd volume ≥ EWMA norm.
        let vol_weight = if self.exposure_ewma > 0.0 {
            (self.ytd.exposure as f64 / self.exposure_ewma).min(1.0)
        } else {
            1.0
        };
        if self.ytd.exposure > 0 {
            // Update exposure norm using prior vol_weight reference (before this year's data).
            self.exposure_ewma = 0.3 * self.ytd.exposure as f64 + 0.7 * self.exposure_ewma;
            let realized_att_lf = self.ytd.attritional_loss_fraction();
            let effective_alpha = self.ewma_credibility * vol_weight;
            self.attritional_elf = effective_alpha * realized_att_lf
                + (1.0 - effective_alpha) * self.attritional_elf;
        }
        // Accumulate per-insurer combined ratio into EWMA for own CR pricing signal.
        if self.ytd.premium > 0 {
            let own_lr = self.ytd.total_claims as f64 / self.ytd.premium as f64;
            let own_cr = own_lr + self.expense_ratio;
            let effective_alpha = OWN_CR_EWMA_ALPHA * vol_weight;
            self.own_cr_ewma = Some(match self.own_cr_ewma {
                // First year: blend toward neutral (1.0) on low volume; full weight when vol_weight=1.
                None       => vol_weight * own_cr + (1.0 - vol_weight) * 1.0,
                Some(prev) => effective_alpha * own_cr + (1.0 - effective_alpha) * prev,
            });
        }
        self.own_years += 1;

        // Distribute fraction of annual underwriting profit to Names.
        // net_written = ytd.premium × (1 - expense_ratio) — expenses already deducted at bind,
        // so this reconstructs the net capital credited from this year's written business.
        // year_profit = net_written − ytd.total_claims; floor at zero via saturating_sub.
        //
        // Capital floor: under Solvency II, distributions are prohibited if they would breach
        // the SCR. We proxy this with initial_capital — distributions are only paid when the
        // post-distribution capital would remain at or above initial_capital. An insurer whose
        // capital has been eroded by losses retains profits to rebuild rather than paying them
        // out. This matches Lloyd's practice: profit release requires that all liabilities are
        // provided for and that the member's FAL remains above the ECA floor.
        let mut events: Vec<(Day, Event)> = vec![];
        if !self.insolvent && self.payout_ratio > 0.0 {
            let net_written = (self.ytd.premium as f64 * (1.0 - self.expense_ratio)).round() as u64;
            let year_profit = net_written.saturating_sub(self.ytd.total_claims);
            if year_profit > 0 {
                let distributable = (year_profit as f64 * self.payout_ratio).round() as u64;
                let distribution_floor = (self.initial_capital as f64 * self.distribution_floor_multiple).round() as i64;
                if distributable > 0 && self.capital - distributable as i64 >= distribution_floor {
                    self.capital -= distributable as i64;
                    events.push((day, Event::CapitalDistributed {
                        insurer_id: self.id,
                        amount: distributable,
                        remaining_capital: self.capital.max(0) as u64,
                    }));
                }
            }
        }

        events.push((day, Event::YearEndCapital {
            insurer_id: self.id,
            capital: self.capital.max(0) as u64,
            initial_capital: self.initial_capital.max(0) as u64,
            ytd_premium: self.ytd.premium,
            ytd_claims: self.ytd.total_claims,
        }));

        self.ytd.reset();

        // Zombie check: capital > 0 but max_line < min writeable policy size.
        // Functionally equivalent to insolvency — no new business can be written.
        // Uses post-distribution capital so the distribution is visible to the check.
        if !self.insolvent {
            if let Some(nlc) = self.net_line_capacity {
                let max_line = (nlc * self.capital.max(0) as f64) as u64;
                if max_line < min_sum_insured {
                    self.insolvent = true;
                    events.push((day, Event::InsurerInsolvent { insurer_id: self.id }));
                    return events;
                }
            }
        }

        events
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::ASSET_VALUE;
    use crate::events::Peril;

    fn small_risk() -> Risk {
        Risk {
            sum_insured: ASSET_VALUE,
            territory: "US-SE".to_string(),
            perils_covered: vec![Peril::WindstormAtlantic, Peril::Attritional],
        }
    }

    fn make_insurer(id: InsurerId, capital: i64) -> Insurer {
        // attritional_elf=0.239, cat_elf=0.0, profit_loading=0.0, depletion_sensitivity=0.0
        // depletion_sensitivity=0.0 → no depletion effect; preserves all existing test behaviour.
        // runoff_cr_threshold=2.0 → never triggers exit in single-year tests.
        // capital_exit_floor=0.0 → floor always passes.
        // leader_participation_cap=1.0 → no leader cap → preserves existing test behaviour.
        Insurer::new(id, capital, 0.239, 0.0, 0.70, 0.3, 0.0, 0.0, None, None, 0.252, 0.0, 0.0, 1.0, 0.30, 0.0, 0.0, 1.0, 1.0)
    }

    /// Helper: quote and return the ATP for a standard small_risk().
    fn quote_atp(ins: &Insurer) -> u64 {
        let risk = Risk {
            sum_insured: ASSET_VALUE,
            territory: "US-SE".to_string(),
            perils_covered: vec![Peril::Attritional],
        };
        let events = ins.on_lead_quote_requested(Day(0), SubmissionId(1), InsuredId(1), &risk, 1.0);
        let (_, event) = events.into_iter().next().unwrap();
        if let Event::LeadQuoteIssued { atp, .. } = event { atp } else { panic!("expected LeadQuoteIssued") }
    }

    #[test]
    fn on_year_start_preserves_capital() {
        let mut ins = make_insurer(InsurerId(1), 1_000_000);
        ins.capital = 500_000; // depleted by claims
        ins.on_year_start();
        assert_eq!(ins.capital, 500_000, "on_year_start must not reset capital — it must persist from prior year");
    }

    #[test]
    fn on_claim_settled_reduces_capital() {
        let mut ins = make_insurer(InsurerId(1), 1_000_000);
        ins.on_claim_settled(Day(0), 300_000, Peril::Attritional);
        assert_eq!(ins.capital, 700_000);
    }

    #[test]
    fn on_claim_settled_floors_at_zero_and_emits_insolvent() {
        let mut ins = make_insurer(InsurerId(1), 100);
        let events = ins.on_claim_settled(Day(5), 1_000_000, Peril::Attritional);
        assert_eq!(ins.capital, 0, "capital must floor at zero");
        assert!(ins.insolvent, "insurer must be marked insolvent");
        assert_eq!(events.len(), 1, "must emit exactly one InsurerInsolvent event");
        assert!(
            matches!(events[0].1, Event::InsurerInsolvent { insurer_id } if insurer_id == InsurerId(1)),
            "event must be InsurerInsolvent with correct id"
        );
    }

    #[test]
    fn multiple_claims_exhaust_capital_and_insurer_becomes_insolvent() {
        // Two policy premiums partially offset initial capital, but further
        // claims exhaust it — capital must floor at zero and insurer must become insolvent.
        let initial_capital = 1_000_000i64;
        let gross_premium = 200_000u64;
        // expense_ratio=0.0 → net premium = gross premium
        let mut ins = Insurer::new(InsurerId(1), initial_capital, 0.239, 0.0, 0.55, 0.3, 0.0, 0.0, None, None, 0.252, 0.0, 0.0, 1.0, 0.30, 0.0, 0.0, 1.0, 1.0);
        ins.on_policy_bound(PolicyId(1), ASSET_VALUE, gross_premium, &[Peril::Attritional], 1.0);
        ins.on_policy_bound(PolicyId(2), ASSET_VALUE, gross_premium, &[Peril::Attritional], 1.0);
        let total_net_premiums = (gross_premium * 2) as i64;
        let total_available = initial_capital + total_net_premiums;
        // Two claims that together exceed total available funds
        let claim = (total_available as u64 / 2) + 1;
        let _ = ins.on_claim_settled(Day(0), claim, Peril::Attritional);
        let _ = ins.on_claim_settled(Day(0), claim, Peril::Attritional);
        assert_eq!(
            ins.capital, 0,
            "capital must floor at zero after cumulative claims exceed available funds; got {}",
            ins.capital
        );
        assert!(ins.insolvent, "insurer must be marked insolvent after capital is exhausted");
    }

    fn first_event(events: Vec<(Day, Event)>) -> (Day, Event) {
        events.into_iter().next().unwrap()
    }

    #[test]
    fn on_lead_quote_requested_always_quotes() {
        let ins = make_insurer(InsurerId(1), 1_000_000_000);
        let risk = small_risk();
        let (_, event) = first_event(ins.on_lead_quote_requested(Day(0), SubmissionId(1), InsuredId(1), &risk, 1.0));
        assert!(
            matches!(event, Event::LeadQuoteIssued { .. }),
            "insurer must always issue a lead quote, got {event:?}"
        );
    }

    #[test]
    fn premium_equals_atp_when_profit_loading_zero() {
        // make_insurer uses profit_loading=0.0, so premium = ATP × 1.0 = ATP.
        let ins = make_insurer(InsurerId(1), 1_000_000_000);
        let risk = small_risk();
        let (_, event) = first_event(ins.on_lead_quote_requested(Day(0), SubmissionId(1), InsuredId(1), &risk, 1.0));
        if let Event::LeadQuoteIssued { atp, premium, .. } = event {
            assert_eq!(premium, atp, "with profit_loading=0.0, premium must equal ATP");
        }
    }

    #[test]
    fn lead_quote_issued_carries_insured_id() {
        let ins = make_insurer(InsurerId(1), 1_000_000_000);
        let risk = small_risk();
        let (_, event) =
            first_event(ins.on_lead_quote_requested(Day(0), SubmissionId(5), InsuredId(42), &risk, 1.0));
        if let Event::LeadQuoteIssued { insured_id, submission_id, insurer_id, .. } = event {
            assert_eq!(insured_id, InsuredId(42));
            assert_eq!(submission_id, SubmissionId(5));
            assert_eq!(insurer_id, InsurerId(1));
        } else {
            panic!("expected LeadQuoteIssued");
        }
    }

    #[test]
    fn premium_scales_with_sum_insured() {
        let ins = make_insurer(InsurerId(1), 0);
        let small = Risk {
            sum_insured: ASSET_VALUE,
            territory: "US-SE".to_string(),
            perils_covered: vec![Peril::Attritional],
        };
        let large = Risk {
            sum_insured: ASSET_VALUE * 10,
            territory: "US-SE".to_string(),
            perils_covered: vec![Peril::Attritional],
        };
        let (_, e_small) =
            first_event(ins.on_lead_quote_requested(Day(0), SubmissionId(1), InsuredId(1), &small, 1.0));
        let (_, e_large) =
            first_event(ins.on_lead_quote_requested(Day(0), SubmissionId(2), InsuredId(2), &large, 1.0));
        let p_small =
            if let Event::LeadQuoteIssued { premium, .. } = e_small { premium } else { 0 };
        let p_large =
            if let Event::LeadQuoteIssued { premium, .. } = e_large { premium } else { 0 };
        assert!(
            p_large > p_small,
            "larger sum_insured must produce larger premium: {p_large} vs {p_small}"
        );
    }

    #[test]
    fn quote_premium_is_positive_for_nonzero_risk() {
        let ins = make_insurer(InsurerId(1), 1_000_000_000);
        let risk = small_risk();
        let (_, event) = first_event(ins.on_lead_quote_requested(Day(0), SubmissionId(1), InsuredId(1), &risk, 1.0));
        if let Event::LeadQuoteIssued { premium, .. } = event {
            assert!(premium > 0, "premium must be positive for a non-trivial risk");
        }
    }

    #[test]
    fn atp_equals_expected_loss_over_target_ratio() {
        let ins = make_insurer(InsurerId(1), 0);
        let risk = small_risk();
        let expected = (0.239 * ASSET_VALUE as f64 / 0.70).round() as u64;
        let (_, event) = first_event(ins.on_lead_quote_requested(Day(0), SubmissionId(1), InsuredId(1), &risk, 1.0));
        if let Event::LeadQuoteIssued { atp, .. } = event {
            assert_eq!(atp, expected, "ATP must equal expected_loss_fraction × sum_insured / target_loss_ratio");
        } else {
            panic!("expected LeadQuoteIssued");
        }
    }

    #[test]
    fn premium_equals_atp_times_loading() {
        // make_insurer uses attritional_elf=0.239, cat_elf=0.0, profit_loading=0.0.
        // premium = (0.239 + 0.0) × sum_insured / target_LR × (1 + 0.0) = ATP.
        let ins = make_insurer(InsurerId(1), 0);
        let risk = small_risk();
        let expected = (0.239 * ASSET_VALUE as f64 / 0.70).round() as u64;
        let (_, event) = first_event(ins.on_lead_quote_requested(Day(0), SubmissionId(1), InsuredId(1), &risk, 1.0));
        if let Event::LeadQuoteIssued { premium, .. } = event {
            assert_eq!(premium, expected, "premium must equal (attritional_elf + cat_elf) × sum_insured / target_loss_ratio × (1 + profit_loading)");
        } else {
            panic!("expected LeadQuoteIssued");
        }
    }

    // ── Exposure management ───────────────────────────────────────────────────

    fn cat_risk() -> Risk {
        Risk {
            sum_insured: ASSET_VALUE,
            territory: "US-SE".to_string(),
            perils_covered: vec![Peril::WindstormAtlantic],
        }
    }

    fn att_only_risk() -> Risk {
        Risk {
            sum_insured: ASSET_VALUE,
            territory: "US-SE".to_string(),
            perils_covered: vec![Peril::Attritional],
        }
    }

    #[test]
    fn on_policy_bound_increments_cat_aggregate() {
        let mut ins = make_insurer(InsurerId(1), 0);
        ins.on_policy_bound(PolicyId(1), ASSET_VALUE, 0, &[Peril::WindstormAtlantic], 1.0);
        assert_eq!(ins.cat_aggregate, ASSET_VALUE, "cat_aggregate must equal sum_insured after binding one cat policy");
    }

    #[test]
    fn on_policy_expired_releases_cat_aggregate() {
        let mut ins = make_insurer(InsurerId(1), 0);
        ins.on_policy_bound(PolicyId(1), ASSET_VALUE, 0, &[Peril::WindstormAtlantic], 1.0);
        assert_eq!(ins.cat_aggregate, ASSET_VALUE);
        ins.on_policy_expired(PolicyId(1));
        assert_eq!(ins.cat_aggregate, 0, "cat_aggregate must return to 0 after policy expiry");
    }

    #[test]
    fn non_cat_policy_does_not_affect_cat_aggregate() {
        let mut ins = make_insurer(InsurerId(1), 0);
        ins.on_policy_bound(PolicyId(1), ASSET_VALUE, 0, &[Peril::Attritional], 1.0);
        assert_eq!(ins.cat_aggregate, 0, "attritional-only policy must not affect cat_aggregate");
    }

    #[test]
    fn cat_exposure_at_quote_reflects_aggregate() {
        let mut ins = make_insurer(InsurerId(1), 0);
        // Bind a cat policy first.
        ins.on_policy_bound(PolicyId(1), ASSET_VALUE, 0, &[Peril::WindstormAtlantic], 1.0);

        // Quote a second cat risk — exposure_at_quote should reflect the already-bound aggregate.
        let risk = cat_risk();
        let (_, event) = first_event(ins.on_lead_quote_requested(Day(0), SubmissionId(2), InsuredId(2), &risk, 1.0));
        if let Event::LeadQuoteIssued { cat_exposure_at_quote, .. } = event {
            assert_eq!(
                cat_exposure_at_quote, ASSET_VALUE,
                "cat_exposure_at_quote must equal the already-bound cat aggregate"
            );
        } else {
            panic!("expected LeadQuoteIssued");
        }
    }

    #[test]
    fn cat_exposure_at_quote_is_zero_for_non_cat_risk() {
        let mut ins = make_insurer(InsurerId(1), 0);
        ins.on_policy_bound(PolicyId(1), ASSET_VALUE, 0, &[Peril::WindstormAtlantic], 1.0);

        let risk = att_only_risk();
        let (_, event) = first_event(ins.on_lead_quote_requested(Day(0), SubmissionId(2), InsuredId(2), &risk, 1.0));
        if let Event::LeadQuoteIssued { cat_exposure_at_quote, .. } = event {
            assert_eq!(
                cat_exposure_at_quote, 0,
                "cat_exposure_at_quote must be 0 for a risk that doesn't cover WindstormAtlantic"
            );
        } else {
            panic!("expected LeadQuoteIssued");
        }
    }

    // ── Exposure limit enforcement ────────────────────────────────────────────

    #[test]
    fn max_line_size_exceeded_emits_declined() {
        // capital=0 → effective_line = 0.30 × 0 = 0 < ASSET_VALUE → declines MaxLineSizeExceeded.
        let ins = Insurer::new(InsurerId(1), 0, 0.239, 0.0, 0.70, 0.3, 0.0, 0.0, Some(0.30), Some(0.30), 0.252, 0.0, 0.0, 1.0, 0.30, 0.0, 0.0, 1.0, 1.0);
        let risk = cat_risk(); // sum_insured = ASSET_VALUE > effective_line_limit (0)
        let (_, event) = first_event(ins.on_lead_quote_requested(Day(0), SubmissionId(1), InsuredId(1), &risk, 1.0));
        assert!(
            matches!(event, Event::LeadQuoteDeclined { reason: DeclineReason::MaxLineSizeExceeded, .. }),
            "expected LeadQuoteDeclined(MaxLineSizeExceeded), got {event:?}"
        );
    }

    #[test]
    fn max_cat_aggregate_breached_emits_declined() {
        // net_line_capacity=None skips the line check; capital=0 → effective_cat = 0 → declines MaxCatAggregateBreached.
        let ins = Insurer::new(InsurerId(1), 0, 0.239, 0.0, 0.70, 0.3, 0.0, 0.0, None, Some(0.30), 0.252, 0.0, 0.0, 1.0, 0.30, 0.0, 0.0, 1.0, 1.0);
        let risk = cat_risk(); // cat_aggregate(0) + sum_insured > effective_cat_limit(0)
        let (_, event) = first_event(ins.on_lead_quote_requested(Day(0), SubmissionId(1), InsuredId(1), &risk, 1.0));
        assert!(
            matches!(event, Event::LeadQuoteDeclined { reason: DeclineReason::MaxCatAggregateBreached, .. }),
            "expected LeadQuoteDeclined(MaxCatAggregateBreached), got {event:?}"
        );
    }

    #[test]
    fn within_limits_after_partial_fill_emits_quote_issued() {
        // capital=200M USD; effective_cat = 0.30 × 20B / 0.252 ≈ 23.8B > 2×ASSET_VALUE=10B → room for second policy.
        let mut ins = Insurer::new(InsurerId(1), 20_000_000_000, 0.239, 0.0, 0.70, 0.3, 0.0, 0.0, None, Some(0.30), 0.252, 0.0, 0.0, 1.0, 0.30, 0.0, 0.0, 1.0, 1.0);
        ins.on_policy_bound(PolicyId(1), ASSET_VALUE, 0, &[Peril::WindstormAtlantic], 1.0);
        // cat_aggregate = ASSET_VALUE; effective_cat ≈ 23.8B → still room for one more
        let risk = cat_risk();
        let (_, event) = first_event(ins.on_lead_quote_requested(Day(0), SubmissionId(2), InsuredId(2), &risk, 1.0));
        assert!(
            matches!(event, Event::LeadQuoteIssued { .. }),
            "still within limit — must emit LeadQuoteIssued, got {event:?}"
        );
    }

    // ── EWMA experience update ────────────────────────────────────────────────

    #[test]
    fn on_year_end_raises_atp_after_high_loss_year() {
        // Bind one policy; settle a claim equal to 100% of sum_insured.
        // Realized LF = 1.0 >> prior ELF = 0.239 → ATP must increase.
        let mut ins = make_insurer(InsurerId(1), ASSET_VALUE as i64 * 10);
        let atp_before = quote_atp(&ins);
        ins.on_policy_bound(PolicyId(1), ASSET_VALUE, 0, &[Peril::Attritional], 1.0);
        let _ = ins.on_claim_settled(Day(0), ASSET_VALUE, Peril::Attritional);
        let _ = ins.on_year_end(Day(0), ASSET_VALUE);
        let atp_after = quote_atp(&ins);
        assert!(atp_after > atp_before, "ATP must rise after a 100% LF year: {atp_after} vs {atp_before}");
    }

    #[test]
    fn on_year_end_lowers_atp_after_benign_year() {
        // Bind one policy; no claims. Realized LF = 0 < prior ELF = 0.239 → ATP must fall.
        let mut ins = make_insurer(InsurerId(1), 0);
        let atp_before = quote_atp(&ins);
        ins.on_policy_bound(PolicyId(1), ASSET_VALUE, 0, &[Peril::Attritional], 1.0);
        // no claims
        let _ = ins.on_year_end(Day(0), ASSET_VALUE);
        let atp_after = quote_atp(&ins);
        assert!(atp_after < atp_before, "ATP must fall after a 0% LF year: {atp_after} vs {atp_before}");
    }

    #[test]
    fn ewma_formula_matches_exact_calculation() {
        // α=0.3, realized LF = 0.5 (claim = ASSET_VALUE/2, exposure = ASSET_VALUE).
        // New ELF = 0.3 × 0.5 + 0.7 × 0.239 = 0.15 + 0.1673 = 0.3173.
        let mut ins = make_insurer(InsurerId(1), ASSET_VALUE as i64 * 10);
        ins.on_policy_bound(PolicyId(1), ASSET_VALUE, 0, &[Peril::Attritional], 1.0);
        let _ = ins.on_claim_settled(Day(0), ASSET_VALUE / 2, Peril::Attritional);
        let _ = ins.on_year_end(Day(0), ASSET_VALUE);
        let expected_elf = 0.3 * 0.5 + 0.7 * 0.239;
        let expected_atp = (expected_elf * ASSET_VALUE as f64 / 0.70).round() as u64;
        assert_eq!(quote_atp(&ins), expected_atp, "EWMA must match α × realized + (1-α) × prior");
    }

    #[test]
    fn on_year_end_with_no_exposure_leaves_atp_unchanged() {
        let mut ins = make_insurer(InsurerId(1), 0);
        let atp_before = quote_atp(&ins);
        let _ = ins.on_year_end(Day(0), ASSET_VALUE); // no policies bound, no claims
        assert_eq!(quote_atp(&ins), atp_before, "ATP must not change if no exposure was written");
    }

    #[test]
    fn on_year_end_resets_so_second_call_without_new_data_is_noop() {
        // After on_year_end resets counters, a second on_year_end with no new
        // policies or claims must leave ATP unchanged.
        let mut ins = make_insurer(InsurerId(1), ASSET_VALUE as i64 * 10);
        ins.on_policy_bound(PolicyId(1), ASSET_VALUE, 0, &[Peril::Attritional], 1.0);
        let _ = ins.on_claim_settled(Day(0), ASSET_VALUE, Peril::Attritional);
        let _ = ins.on_year_end(Day(0), ASSET_VALUE); // ELF updated, counters reset
        let atp_year1 = quote_atp(&ins);
        let _ = ins.on_year_end(Day(0), ASSET_VALUE); // no new data → noop
        assert_eq!(quote_atp(&ins), atp_year1, "second on_year_end with no data must be a noop");
    }

    // ── Capital distribution tests ────────────────────────────────────────────

    #[test]
    fn on_year_end_distributes_profit_in_profitable_year() {
        // expense_ratio=0.0, payout_ratio=0.70; bind premium=100_000, no claims.
        // net_written = 100_000; year_profit = 100_000; distributable = 70_000.
        let initial_capital = 1_000_000i64;
        let premium = 100_000u64;
        let mut ins = Insurer::new(
            InsurerId(1), initial_capital, 0.239, 0.0, 0.70, 0.3,
            0.0, 0.0, None, None, 0.252, 0.0, 0.0, 1.0, 0.30, 0.0, 0.70,
            1.0, 1.0,
        );
        ins.on_policy_bound(PolicyId(1), ASSET_VALUE, premium, &[Peril::Attritional], 1.0);
        // capital after bind = initial + premium (expense_ratio=0)
        let events = ins.on_year_end(Day(360), ASSET_VALUE);

        let distributed = events.iter().find_map(|(_, e)| {
            if let Event::CapitalDistributed { insurer_id, amount, remaining_capital } = e {
                assert_eq!(*insurer_id, InsurerId(1), "distribution must be for this insurer");
                Some((*amount, *remaining_capital))
            } else {
                None
            }
        });
        let (amount, remaining) = distributed.expect("CapitalDistributed must be emitted in a profitable year");
        let expected_amount = (premium as f64 * 0.70).round() as u64;
        assert_eq!(amount, expected_amount, "distribution must be 70% of net profit");
        let expected_remaining = (initial_capital + premium as i64 - amount as i64) as u64;
        assert_eq!(remaining, expected_remaining, "remaining_capital must equal capital after distribution");
        assert_eq!(ins.capital, expected_remaining as i64, "insurer capital reduced by distributable");
    }

    #[test]
    fn on_year_end_no_distribution_in_loss_year() {
        // expense_ratio=0.0, payout_ratio=0.70; premium=100_000, claims=200_000 → loss year.
        // year_profit = 100_000.saturating_sub(200_000) = 0 → no distribution.
        let initial_capital = 1_000_000_000i64; // large to survive the claim
        let premium = 100_000u64;
        let mut ins = Insurer::new(
            InsurerId(1), initial_capital, 0.239, 0.0, 0.70, 0.3,
            0.0, 0.0, None, None, 0.252, 0.0, 0.0, 1.0, 0.30, 0.0, 0.70,
            1.0, 1.0,
        );
        ins.on_policy_bound(PolicyId(1), ASSET_VALUE, premium, &[Peril::Attritional], 1.0);
        let _ = ins.on_claim_settled(Day(10), premium * 2, Peril::Attritional);
        let events = ins.on_year_end(Day(360), ASSET_VALUE);

        let has_distribution = events.iter().any(|(_, e)| matches!(e, Event::CapitalDistributed { .. }));
        assert!(!has_distribution, "no CapitalDistributed must be emitted in a loss year");
    }

    #[test]
    fn on_year_end_no_distribution_when_payout_zero() {
        // payout_ratio=0.0 → no distribution even in a profitable year.
        let initial_capital = 1_000_000i64;
        let premium = 100_000u64;
        let mut ins = Insurer::new(
            InsurerId(1), initial_capital, 0.239, 0.0, 0.70, 0.3,
            0.0, 0.0, None, None, 0.252, 0.0, 0.0, 1.0, 0.30, 0.0, 0.0,
            1.0, 1.0,
        );
        ins.on_policy_bound(PolicyId(1), ASSET_VALUE, premium, &[Peril::Attritional], 1.0);
        let events = ins.on_year_end(Day(360), ASSET_VALUE);

        let has_distribution = events.iter().any(|(_, e)| matches!(e, Event::CapitalDistributed { .. }));
        assert!(!has_distribution, "no CapitalDistributed must be emitted when payout_ratio=0.0");
    }

    #[test]
    fn on_year_end_no_distribution_when_capital_below_floor() {
        // Capital has been depleted below initial_capital by prior losses.
        // Even though this year is profitable, distribution would take capital further below
        // initial_capital, so it must be suppressed (Solvency II SCR floor proxy).
        let initial_capital = 1_000_000i64;
        let premium = 100_000u64;
        let mut ins = Insurer::new(
            InsurerId(1), initial_capital, 0.239, 0.0, 0.70, 0.3,
            0.0, 0.0, None, None, 0.252, 0.0, 0.0, 1.0, 0.30, 0.0, 0.70,
            1.0, 1.0,
        );
        // Manually deplete capital below initial_capital (simulate prior cat year losses).
        ins.capital = initial_capital - 50_000; // 950_000 < 1_000_000
        ins.on_policy_bound(PolicyId(1), ASSET_VALUE, premium, &[Peril::Attritional], 1.0);
        // profitable year: net_written=100_000, claims=0 → year_profit=100_000, distributable=70_000
        // capital_after_distribution = 950_000 + 100_000 - 70_000 = 980_000 < initial_capital=1_000_000
        // → floor check fails → no distribution
        let events = ins.on_year_end(Day(360), ASSET_VALUE);
        let has_distribution = events.iter().any(|(_, e)| matches!(e, Event::CapitalDistributed { .. }));
        assert!(!has_distribution, "no CapitalDistributed when post-distribution capital would fall below initial_capital");
    }

    #[test]
    fn on_year_end_distributes_only_when_capital_restored_above_floor() {
        // Capital is depleted but profit this year would fully restore it above initial_capital.
        // Distributable portion: only the surplus above initial_capital can be paid out.
        // Distribution fires because capital - distributable >= initial_capital.
        let initial_capital = 1_000_000i64;
        let premium = 200_000u64; // large profit year
        let mut ins = Insurer::new(
            InsurerId(1), initial_capital, 0.239, 0.0, 0.70, 0.3,
            0.0, 0.0, None, None, 0.252, 0.0, 0.0, 1.0, 0.30, 0.0, 0.70,
            1.0, 1.0,
        );
        ins.capital = initial_capital - 50_000; // 950_000 — depleted
        ins.on_policy_bound(PolicyId(1), ASSET_VALUE, premium, &[Peril::Attritional], 1.0);
        // capital after bind = 950_000 + 200_000 = 1_150_000 (expense_ratio=0 in test insurer)
        // year_profit = 200_000; distributable = 140_000
        // capital_after = 1_150_000 - 140_000 = 1_010_000 >= initial_capital=1_000_000 → distributes
        let events = ins.on_year_end(Day(360), ASSET_VALUE);
        let has_distribution = events.iter().any(|(_, e)| matches!(e, Event::CapitalDistributed { .. }));
        assert!(has_distribution, "CapitalDistributed must fire when post-distribution capital stays at or above initial_capital");
    }

    #[test]
    fn ewma_compounds_over_multiple_years() {
        // Two consecutive high-loss years should push ELF higher than one.
        let mut ins = make_insurer(InsurerId(1), ASSET_VALUE as i64 * 10);
        ins.on_policy_bound(PolicyId(1), ASSET_VALUE, 0, &[Peril::Attritional], 1.0);
        let _ = ins.on_claim_settled(Day(0), ASSET_VALUE, Peril::Attritional);
        let _ = ins.on_year_end(Day(0), ASSET_VALUE);
        let atp_after_year1 = quote_atp(&ins);

        ins.on_policy_bound(PolicyId(2), ASSET_VALUE, 0, &[Peril::Attritional], 1.0);
        let _ = ins.on_claim_settled(Day(0), ASSET_VALUE, Peril::Attritional);
        let _ = ins.on_year_end(Day(0), ASSET_VALUE);
        let atp_after_year2 = quote_atp(&ins);

        assert!(atp_after_year2 > atp_after_year1, "consecutive bad years must compound ELF upward");
    }

    #[test]
    fn on_policy_bound_credits_net_premium_to_capital() {
        // expense_ratio=0.25 → net = 75% of gross premium.
        let mut ins = Insurer::new(InsurerId(1), 1_000_000, 0.239, 0.0, 0.55, 0.3, 0.25, 0.0, None, None, 0.252, 0.0, 0.0, 1.0, 0.30, 0.0, 0.0, 1.0, 1.0);
        let gross_premium = 400_000u64;
        ins.on_policy_bound(PolicyId(1), ASSET_VALUE, gross_premium, &[Peril::Attritional], 1.0);
        let expected_net = (gross_premium as f64 * 0.75).round() as i64;
        assert_eq!(
            ins.capital,
            1_000_000 + expected_net,
            "capital must increase by net premium (gross × (1 − expense_ratio))"
        );
    }

    // ── Zombie insurer detection ──────────────────────────────────────────────

    #[test]
    fn zombie_insurer_marked_insolvent_at_year_end() {
        // capital = 80M USD → max_line = 0.30 × 80M = 24M < ASSET_VALUE (25M) → zombie
        let capital_cents = 8_000_000_000i64; // 80M USD
        let mut ins = Insurer::new(
            InsurerId(1), capital_cents,
            0.239, 0.0, 0.70, 0.3, 0.0, 0.0,
            Some(0.30), None, 0.252, 0.0, 0.0, 1.0, 0.30, 0.0, 0.0,
            1.0, 1.0,
        );
        let events = ins.on_year_end(Day(360), ASSET_VALUE);
        assert!(ins.insolvent, "zombie insurer must be marked insolvent");
        // YearEndCapital is always emitted, InsurerInsolvent is appended on zombie detection.
        assert_eq!(events.len(), 2);
        assert!(matches!(
            events[1].1,
            Event::InsurerInsolvent { insurer_id } if insurer_id == InsurerId(1)
        ));
    }

    #[test]
    fn insurer_at_max_line_threshold_not_marked_insolvent() {
        // capital = ceil(ASSET_VALUE / 0.30) cents → max_line = 0.30 × capital ≥ ASSET_VALUE → not zombie
        let capital_cents = (ASSET_VALUE as f64 / 0.30).ceil() as i64;
        let mut ins = Insurer::new(
            InsurerId(1), capital_cents,
            0.239, 0.0, 0.70, 0.3, 0.0, 0.0,
            Some(0.30), None, 0.252, 0.0, 0.0, 1.0, 0.30, 0.0, 0.0,
            1.0, 1.0,
        );
        let events = ins.on_year_end(Day(360), ASSET_VALUE);
        assert!(!ins.insolvent, "insurer at threshold must not be marked insolvent");
        // YearEndCapital is always emitted; no InsurerInsolvent here.
        assert_eq!(events.len(), 1);
        assert!(matches!(events[0].1, Event::YearEndCapital { .. }));
    }

    // ── Heterogeneous experience divergence ───────────────────────────────────

    #[test]
    fn two_insurers_diverge_in_atp_after_asymmetric_attritional_loss() {
        // Both start identical. ins_a incurs a 100% attritional loss; ins_b has a benign year.
        // After on_year_end the EWMA update must push ins_a's ATP above ins_b's.
        let capital = ASSET_VALUE as i64 * 10;
        let mut ins_a = make_insurer(InsurerId(1), capital);
        let mut ins_b = make_insurer(InsurerId(2), capital);

        ins_a.on_policy_bound(PolicyId(1), ASSET_VALUE, 0, &[Peril::Attritional], 1.0);
        ins_b.on_policy_bound(PolicyId(2), ASSET_VALUE, 0, &[Peril::Attritional], 1.0);

        // ins_a: 100% loss; ins_b: no claims
        let _ = ins_a.on_claim_settled(Day(0), ASSET_VALUE, Peril::Attritional);

        let _ = ins_a.on_year_end(Day(360), ASSET_VALUE);
        let _ = ins_b.on_year_end(Day(360), ASSET_VALUE);

        let atp_a = quote_atp(&ins_a);
        let atp_b = quote_atp(&ins_b);
        assert!(
            atp_a > atp_b,
            "ins_a (100% loss year) must have higher ATP than ins_b (benign year): {atp_a} vs {atp_b}"
        );
    }

    #[test]
    fn two_insurers_diverge_in_capacity_after_asymmetric_cat_loss() {
        // Both start at 15M USD capital with net_line_capacity=0.30.
        // ins_a is drained to ~5M USD → max_line = 0.30 × 5M = 1.5M < 25M sum_insured → declines.
        // ins_b is untouched → max_line = 0.30 × 15M = 4.5M < 25M → also declined?
        // Use larger capital so ins_b can still write the risk.
        // ins_b capital = 100M USD → max_line = 30M > ASSET_VALUE ✓
        // ins_a drained to 5M USD → max_line = 1.5M < ASSET_VALUE → MaxLineSizeExceeded
        let capital_b = 10_000_000_000i64; // 100M USD in cents
        let capital_a = 10_000_000_000i64;

        let mut ins_a = Insurer::new(InsurerId(1), capital_a, 0.239, 0.0, 0.70, 0.3, 0.0, 0.0, Some(0.30), Some(0.30), 0.252, 0.0, 0.0, 1.0, 0.30, 0.0, 0.0, 1.0, 1.0);
        let ins_b = Insurer::new(InsurerId(2), capital_b, 0.239, 0.0, 0.70, 0.3, 0.0, 0.0, Some(0.30), Some(0.30), 0.252, 0.0, 0.0, 1.0, 0.30, 0.0, 0.0, 1.0, 1.0);

        ins_a.on_policy_bound(PolicyId(1), ASSET_VALUE, 0, &[Peril::WindstormAtlantic], 1.0);

        // Drain ins_a to ~5M USD (500_000_000 cents) via cat claims
        let drain = capital_a - 500_000_000;
        let _ = ins_a.on_claim_settled(Day(10), drain as u64, Peril::WindstormAtlantic);
        assert!(ins_a.capital < 600_000_000, "ins_a must be nearly depleted: {}", ins_a.capital);

        // Submit identical 25M USD cat risk to both
        let risk = cat_risk();
        let (_, event_a) = first_event(ins_a.on_lead_quote_requested(Day(20), SubmissionId(1), InsuredId(1), &risk, 1.0));
        let (_, event_b) = first_event(ins_b.on_lead_quote_requested(Day(20), SubmissionId(2), InsuredId(2), &risk, 1.0));

        assert!(
            matches!(event_a, Event::LeadQuoteDeclined { reason: DeclineReason::MaxLineSizeExceeded, .. }),
            "depleted ins_a must decline with MaxLineSizeExceeded, got {event_a:?}"
        );
        assert!(
            matches!(event_b, Event::LeadQuoteIssued { .. }),
            "well-capitalised ins_b must issue a quote, got {event_b:?}"
        );
    }

    #[test]
    fn atp_divergence_grows_over_multiple_years() {
        // ins_a incurs 100% attritional loss each year; ins_b has benign years.
        // The ATP gap must widen year-over-year as EWMA credibility accumulates.
        let capital = ASSET_VALUE as i64 * 10;
        let mut ins_a = make_insurer(InsurerId(1), capital);
        let mut ins_b = make_insurer(InsurerId(2), capital);

        let mut gap_yr1 = 0i64;

        for year in 0..3u64 {
            let pid_a = PolicyId(year * 2 + 1);
            let pid_b = PolicyId(year * 2 + 2);

            ins_a.on_policy_bound(pid_a, ASSET_VALUE, 0, &[Peril::Attritional], 1.0);
            ins_b.on_policy_bound(pid_b, ASSET_VALUE, 0, &[Peril::Attritional], 1.0);

            let _ = ins_a.on_claim_settled(Day(0), ASSET_VALUE, Peril::Attritional);
            // ins_b: no claims

            let _ = ins_a.on_year_end(Day(360), ASSET_VALUE);
            let _ = ins_b.on_year_end(Day(360), ASSET_VALUE);

            let gap = quote_atp(&ins_a) as i64 - quote_atp(&ins_b) as i64;
            if year == 0 {
                gap_yr1 = gap;
            } else if year == 2 {
                assert!(
                    gap > gap_yr1,
                    "ATP gap after 3 years ({gap}) must exceed gap after year 1 ({gap_yr1}) — divergence compounds"
                );
            }
        }
    }

    // ── Per-insurer capital-state pricing ─────────────────────────────────────

    /// Helper: quote and return the premium (not ATP) for a standard attritional risk.
    fn quote_premium(ins: &Insurer, market_factor: f64) -> u64 {
        let risk = Risk {
            sum_insured: ASSET_VALUE,
            territory: "US-SE".to_string(),
            perils_covered: vec![Peril::Attritional],
        };
        let events = ins.on_lead_quote_requested(Day(0), SubmissionId(1), InsuredId(1), &risk, market_factor);
        let (_, event) = events.into_iter().next().unwrap();
        if let Event::LeadQuoteIssued { premium, .. } = event {
            premium
        } else {
            panic!("expected LeadQuoteIssued, got {event:?}")
        }
    }

    #[test]
    fn new_insurer_uses_market_factor_when_no_experience() {
        // own_years=0 → credibility=0 → insurer_ap_tp = market_factor exactly.
        // depletion_sensitivity=1.0; capital=initial → no depletion adj.
        let ins = Insurer::new(InsurerId(1), 1_000_000, 0.239, 0.0, 0.70, 0.3, 0.0, 0.0, None, None, 0.252, 1.0, 0.0, 1.0, 0.30, 0.0, 0.0, 1.0, 1.0);
        let market_factor = 1.20;
        let premium = quote_premium(&ins, market_factor);

        // TP = ATP × (1 + profit_loading=0) = ATP; expected = ATP × 1.20
        let atp = (0.239 * ASSET_VALUE as f64 / 0.70).round() as u64;
        let expected = (atp as f64 * market_factor).round() as u64;
        assert_eq!(premium, expected,
            "new entrant (own_years=0) must follow market factor exactly: got {premium}, expected {expected}");
    }

    #[test]
    fn depleted_insurer_quotes_above_market_with_full_credibility() {
        // 30% depletion, own_years=5 (credibility=1.0), no loss history → own_cr_signal=0.0
        // cap_depletion_adj = clamp(0.30 × 1.0, 0, 0.30) = 0.30
        // own_factor = 1.0 + 0.0 + 0.30 = 1.30
        // market_weight = max(1.0 − 1.0, 0.30) = 0.30 (floor — market always has a voice)
        // insurer_ap_tp = 0.70 × 1.30 + 0.30 × 1.0 = 0.91 + 0.30 = 1.21
        let initial_capital = 1_000_000i64;
        let current_capital = 700_000i64; // 30% depletion
        let mut ins = Insurer::new(InsurerId(1), initial_capital, 0.239, 0.0, 0.70, 0.3, 0.0, 0.0, None, None, 0.252, 1.0, 0.0, 1.0, 0.30, 0.0, 0.0, 1.0, 1.0);
        ins.capital = current_capital;
        ins.own_years = 5; // full credibility

        let premium = quote_premium(&ins, 1.0);
        let atp = (0.239 * ASSET_VALUE as f64 / 0.70).round() as u64;
        let expected = (atp as f64 * 1.21).round() as u64;
        assert_eq!(premium, expected,
            "depleted insurer with full credibility must quote at 0.70×1.30+0.30×1.0=1.21: got {premium}, expected {expected}");
    }

    #[test]
    fn own_cr_signal_elevated_after_loss_year_raises_own_factor() {
        // No capital depletion (capital=initial); credibility=1.0 (own_years=5).
        // Bind one policy, settle 200% claim relative to premium → LR=2.0.
        // own_cr_signal = clamp((2.0 + 0.0) − 1.0, −0.30, 0.80) = 0.80 (expense_ratio=0.0)
        // own_factor = 1.0 + 0.80 + 0.0 = 1.80
        // insurer_ap_tp > neutral market_factor=1.0
        let capital = 100_000_000i64;
        let mut ins = Insurer::new(InsurerId(1), capital, 0.239, 0.0, 0.70, 0.3, 0.0, 0.0, None, None, 0.252, 1.0, 0.0, 1.0, 0.30, 0.0, 0.0, 1.0, 1.0);
        ins.own_years = 5;

        // Record a very high-loss year: premium=P, claims=2P → LR=2.0
        let premium = 1_000_000u64;
        ins.on_policy_bound(PolicyId(1), ASSET_VALUE, premium, &[Peril::Attritional], 1.0);
        let _ = ins.on_claim_settled(Day(10), premium * 2, Peril::Attritional);
        let _ = ins.on_year_end(Day(360), ASSET_VALUE);

        // TP is computed from the *current* (post-EWMA) ATP. own_factor=1.40 > 1.0,
        // so premium = current_ATP × 1.40 > current_ATP × 1.0 = TP.
        let current_atp = quote_atp(&ins);
        let premium_quoted = quote_premium(&ins, 1.0);
        assert!(
            premium_quoted > current_atp,
            "elevated own CR signal must push premium above TP×1.0 (neutral): got {premium_quoted}, TP={current_atp}"
        );
    }

    #[test]
    fn on_year_end_increments_own_years_and_pushes_lr() {
        // After one YearEnd with premium written, own_years goes from 0 to 1.
        // A new insurer with own_years=0 follows market exactly;
        // after YearEnd (own_years=1, credibility=0.2), the blend shifts toward own_factor.
        let capital = 100_000_000i64;
        let mut ins = Insurer::new(InsurerId(1), capital, 0.239, 0.0, 0.70, 0.3, 0.0, 0.0, None, None, 0.252, 1.0, 0.0, 1.0, 0.30, 0.0, 0.0, 1.0, 1.0);

        // Before YearEnd: own_years=0
        assert_eq!(ins.own_years, 0);

        // Bind and push a high-loss year so own_factor will differ from market
        let premium = 1_000_000u64;
        ins.on_policy_bound(PolicyId(1), ASSET_VALUE, premium, &[Peril::Attritional], 1.0);
        let _ = ins.on_claim_settled(Day(10), premium * 4, Peril::Attritional);
        let _ = ins.on_year_end(Day(360), ASSET_VALUE);

        assert_eq!(ins.own_years, 1, "own_years must increment to 1 after one YearEnd");
        assert!(ins.own_cr_ewma.is_some(), "own_cr_ewma must be initialised after one YearEnd with premium");

        // With own_years=1 credibility=0.2; own_factor elevated (high LR).
        // At market_factor=0.90 (soft), premium must still be > market's pure TP × 0.90
        // because own experience is bad. With market=1.40 (hard), premium could be ≤ own factor.
        // Just assert own_years was incremented and LR was recorded (structural test).
    }

    #[test]
    fn partial_credibility_blends_own_and_market_factors() {
        // own_years=2 → credibility=0.4
        // Record LR=2.0 (claims=2×premium, expense_ratio=0.0 → CR=2.0)
        // own_cr_signal = clamp(2.0 − 1.0, −0.30, 0.80) = 0.80
        // cap_depletion_adj = 0.0 (depletion_sensitivity=0.0 — isolates credibility blending)
        // own_factor = 1.0 + 0.80 + 0.0 = 1.80
        // market_factor = 0.90
        // insurer_ap_tp = 0.4 × 1.80 + 0.6 × 0.90 = 0.72 + 0.54 = 1.26
        let capital = 100_000_000i64;
        let mut ins = Insurer::new(InsurerId(1), capital, 0.239, 0.0, 0.70, 0.3, 0.0, 0.0, None, None, 0.252, 0.0, 0.0, 1.0, 0.30, 0.0, 0.0, 1.0, 1.0);
        ins.own_years = 2;

        // Record one high-loss year: LR=2.0
        let premium = 1_000_000u64;
        ins.on_policy_bound(PolicyId(1), ASSET_VALUE, premium, &[Peril::Attritional], 1.0);
        let _ = ins.on_claim_settled(Day(10), premium * 2, Peril::Attritional);
        // Manually push LR into buffer without triggering another on_year_end increment
        // Use on_year_end which also increments own_years; compensate by pre-setting own_years=1
        ins.own_years = 1; // will become 2 after on_year_end
        let _ = ins.on_year_end(Day(360), ASSET_VALUE);
        assert_eq!(ins.own_years, 2, "own_years should be 2 after one more YearEnd");

        // Use post-EWMA ATP for the expected value; EWMA updated attritional_elf during on_year_end.
        // The blend factor should be 0.4×1.80 + 0.6×0.90 = 1.26 regardless of the ATP level.
        let current_atp = quote_atp(&ins);
        let expected = (current_atp as f64 * 1.26).round() as u64;
        let premium_quoted = quote_premium(&ins, 0.90);
        assert_eq!(premium_quoted, expected,
            "partial credibility blend: 0.4×1.80 + 0.6×0.90 = 1.26; got {premium_quoted}, expected {expected}");
    }

    // ── Capacity utilisation adjustment ───────────────────────────────────────

    #[test]
    fn capacity_adj_zero_when_no_solvency_fraction() {
        // solvency_capital_fraction=None → utilisation=0.0 → capacity_adj=0.0
        // regardless of capacity_sensitivity or cat_aggregate.
        let capital = 10_000_000_000i64; // 100M USD
        let mut ins = Insurer::new(
            InsurerId(1), capital, 0.239, 0.0, 0.70, 0.3, 0.0, 0.0,
            None, None, 0.252, 0.0, 1.0, 1.0, 0.30, 0.0, 0.0, // capacity_sensitivity=1.0 but scf=None → no adj
            1.0, 1.0,
        );
        // Simulate high cat load
        ins.on_policy_bound(PolicyId(1), ASSET_VALUE * 10, 0, &[Peril::WindstormAtlantic], 1.0);
        ins.own_years = 5;

        // Premium must equal TP (ATP × 1.0 × blend factor with capacity_adj=0)
        let risk = Risk {
            sum_insured: ASSET_VALUE,
            territory: "US-SE".to_string(),
            perils_covered: vec![Peril::Attritional],
        };
        let events = ins.on_lead_quote_requested(Day(0), SubmissionId(1), InsuredId(1), &risk, 1.0);
        let (_, event) = events.into_iter().next().unwrap();
        if let Event::LeadQuoteIssued { atp, premium, .. } = event {
            // own_cr_signal=0 (no history), cap_depletion_adj=0 (capital=initial), capacity_adj=0
            // own_factor=1.0, blend at credibility=1.0: 0.70×1.0+0.30×1.0=1.0
            assert_eq!(premium, atp, "with scf=None, capacity_adj=0: premium must equal ATP");
        } else {
            panic!("expected LeadQuoteIssued");
        }
    }

    #[test]
    fn capacity_adj_scales_with_utilisation() {
        // solvency_capital_fraction=Some(0.30), capacity_sensitivity=0.10
        // capital=10B cents (100M USD), pml=0.30 → effective_cat_limit = 0.30×10B/0.30 = 10B
        // Bind cat_aggregate = 8B → utilisation = 0.80
        // capacity_adj = clamp(0.80 × 0.10, 0.0, 0.20) = 0.08
        // own_years=5 (full credibility), no loss history → own_cr_signal=0, cap_depletion_adj=0
        // own_factor = 1.0 + 0.0 + 0.0 + 0.08 = 1.08
        // market_weight = 0.30 (floor), insurer_ap_tp = 0.70×1.08 + 0.30×1.0 = 0.756+0.30 = 1.056
        let capital = 10_000_000_000i64; // 100M USD in cents
        let pml = 0.30_f64; // calibrated so effective_cat_limit = capital
        let mut ins = Insurer::new(
            InsurerId(1), capital, 0.239, 0.0, 0.70, 0.3, 0.0, 0.0,
            None, Some(0.30), pml, 0.0, 0.10, 1.0, 0.30, 0.0, 0.0,
            1.0, 1.0,
        );
        ins.own_years = 5;

        // Bind cat_aggregate = 8B (80% of effective limit = 10B)
        ins.on_policy_bound(PolicyId(1), 8_000_000_000, 0, &[Peril::WindstormAtlantic], 1.0);
        assert_eq!(ins.cat_aggregate, 8_000_000_000);

        let risk = Risk {
            sum_insured: ASSET_VALUE,
            territory: "US-SE".to_string(),
            perils_covered: vec![Peril::Attritional],
        };
        let events = ins.on_lead_quote_requested(Day(0), SubmissionId(1), InsuredId(1), &risk, 1.0);
        let (_, event) = events.into_iter().next().unwrap();
        if let Event::LeadQuoteIssued { atp, premium, .. } = event {
            let expected = (atp as f64 * 1.056).round() as u64;
            assert_eq!(premium, expected,
                "at 80% utilisation capacity_adj=0.08 → factor=1.056: got {premium}, expected {expected}");
        } else {
            panic!("expected LeadQuoteIssued");
        }
    }

    #[test]
    fn capacity_adj_zero_when_sensitivity_zero() {
        // capacity_sensitivity=0.0 → adj=0 even at 100% utilisation.
        let capital = 10_000_000_000i64;
        let pml = 0.30_f64;
        let mut ins = Insurer::new(
            InsurerId(1), capital, 0.239, 0.0, 0.70, 0.3, 0.0, 0.0,
            None, Some(0.30), pml, 0.0, 0.0, 1.0, 0.30, 0.0, 0.0, // capacity_sensitivity=0.0
            1.0, 1.0,
        );
        ins.own_years = 5;
        // Load to 100% utilisation
        ins.on_policy_bound(PolicyId(1), capital as u64, 0, &[Peril::WindstormAtlantic], 1.0);

        let risk = Risk {
            sum_insured: ASSET_VALUE,
            territory: "US-SE".to_string(),
            perils_covered: vec![Peril::Attritional],
        };
        let events = ins.on_lead_quote_requested(Day(0), SubmissionId(1), InsuredId(1), &risk, 1.0);
        let (_, event) = events.into_iter().next().unwrap();
        if let Event::LeadQuoteIssued { atp, premium, .. } = event {
            assert_eq!(premium, atp,
                "with capacity_sensitivity=0, adj=0 even at full utilisation: premium must equal ATP");
        } else {
            panic!("expected LeadQuoteIssued");
        }
    }

    // ── Phase B: heterogeneous sensitivity parameters ─────────────────────────

    #[test]
    fn cr_sensitivity_scales_own_cr_signal() {
        // Two insurers: identical except cr_sensitivity (2.0 vs 0.5).
        // After one high-loss year, the insurer with cr_sensitivity=2.0 must quote
        // a higher premium than the one with cr_sensitivity=0.5.
        let capital = 100_000_000i64;
        // depletion_sensitivity=0.0, capacity_sensitivity=0.0 → isolate cr_sensitivity effect.
        // own_years pre-set to 5 → full credibility → market_weight = market_weight_floor.
        let mut ins_hi = Insurer::new(InsurerId(1), capital, 0.239, 0.0, 0.70, 0.3, 0.0, 0.0, None, None, 0.252, 0.0, 0.0, 2.0, 0.30, 0.0, 0.0, 1.0, 1.0);
        let mut ins_lo = Insurer::new(InsurerId(2), capital, 0.239, 0.0, 0.70, 0.3, 0.0, 0.0, None, None, 0.252, 0.0, 0.0, 0.5, 0.30, 0.0, 0.0, 1.0, 1.0);
        ins_hi.own_years = 5;
        ins_lo.own_years = 5;

        // Record a high-loss year: LR = 2.0 (claims = 2 × premium)
        let prem = 1_000_000u64;
        ins_hi.on_policy_bound(PolicyId(1), ASSET_VALUE, prem, &[Peril::Attritional], 1.0);
        ins_lo.on_policy_bound(PolicyId(2), ASSET_VALUE, prem, &[Peril::Attritional], 1.0);
        let _ = ins_hi.on_claim_settled(Day(10), prem * 2, Peril::Attritional);
        let _ = ins_lo.on_claim_settled(Day(10), prem * 2, Peril::Attritional);
        // own_years will increment from 5 → 6 for both
        let _ = ins_hi.on_year_end(Day(360), ASSET_VALUE);
        let _ = ins_lo.on_year_end(Day(360), ASSET_VALUE);

        let p_hi = quote_premium(&ins_hi, 1.0);
        let p_lo = quote_premium(&ins_lo, 1.0);
        assert!(
            p_hi > p_lo,
            "cr_sensitivity=2.0 must quote higher than cr_sensitivity=0.5 after bad CR year: {p_hi} vs {p_lo}"
        );
    }

    #[test]
    fn market_weight_floor_respected() {
        // Insurer with market_weight_floor=0.60 and full credibility (own_years=5):
        // market_weight = max(1 - 1.0, 0.60) = 0.60.
        // own_cr_signal=0 (no loss history), cap_depletion_adj=0, capacity_adj=0.
        // own_factor = 1.0; insurer_ap_tp = 0.40 × 1.0 + 0.60 × market_factor.
        // market_factor = 1.40 → expected = 0.40 × 1.0 + 0.60 × 1.40 = 0.40 + 0.84 = 1.24.
        let capital = 100_000_000i64;
        let mut ins = Insurer::new(InsurerId(1), capital, 0.239, 0.0, 0.70, 0.3, 0.0, 0.0, None, None, 0.252, 0.0, 0.0, 1.0, 0.60, 0.0, 0.0, 1.0, 1.0);
        ins.own_years = 5; // full credibility

        let atp = (0.239 * ASSET_VALUE as f64 / 0.70).round() as u64;
        let expected = (atp as f64 * 1.24).round() as u64;
        let premium = quote_premium(&ins, 1.40);
        assert_eq!(
            premium, expected,
            "market_weight_floor=0.60: blend = 0.40×1.0 + 0.60×1.40 = 1.24; got {premium}, expected {expected}"
        );
    }

    // ── Phase 5: line_size computation ────────────────────────────────────────

    #[test]
    fn line_size_soft_market() {
        // With no own history (own_years=0), market_weight=1.0, so insurer_ap_tp = market_factor.
        // market_factor = 0.90, floor_factor = 0.85 →
        //   pricing_line = (0.90 - 0.85) / (1.0 - 0.85) = 0.05 / 0.15 ≈ 0.333
        // No capacity limit → capacity_line = 1.0; line_size = 0.333.
        let ins = Insurer::new(
            InsurerId(1), 10_000_000_000, 0.0, 0.0, 1.0, 0.3,
            0.0, 0.0, None, None, 0.252, 0.0, 0.0, 1.0, 0.30, 0.85, 0.0,
            1.0, 1.0,
        );
        use crate::types::SubmissionId;
        let risk = Risk {
            sum_insured: ASSET_VALUE,
            territory: "US-SE".to_string(),
            perils_covered: vec![],
        };
        let events = ins.on_lead_quote_requested(Day(1), SubmissionId(1), InsuredId(1), &risk, 0.90);
        let line_size = events.iter().find_map(|(_, e)| {
            if let Event::LeadQuoteIssued { line_size, .. } = e { Some(*line_size) } else { None }
        });
        let expected = (0.90_f64 - 0.85) / (1.0 - 0.85); // ≈ 0.333
        match line_size {
            Some(ls) => assert!(
                (ls - expected).abs() < 1e-9,
                "soft market: expected pricing_line ≈ {expected:.4}, got {ls:.4}"
            ),
            None => panic!("expected LeadQuoteIssued, got: {events:?}"),
        }
    }

    #[test]
    fn line_size_hard_market() {
        // own_ap_tp_factor >= 1.0 (market_factor = 1.10) → pricing_line = 1.0
        let ins = Insurer::new(
            InsurerId(1), 10_000_000_000, 0.0, 0.0, 1.0, 0.3,
            0.0, 0.0, None, None, 0.252, 0.0, 0.0, 1.0, 0.30, 0.85, 0.0,
            1.0, 1.0,
        );
        use crate::types::SubmissionId;
        let risk = Risk {
            sum_insured: ASSET_VALUE,
            territory: "US-SE".to_string(),
            perils_covered: vec![],
        };
        let events = ins.on_lead_quote_requested(Day(1), SubmissionId(1), InsuredId(1), &risk, 1.10);
        let line_size = events.iter().find_map(|(_, e)| {
            if let Event::LeadQuoteIssued { line_size, .. } = e { Some(*line_size) } else { None }
        });
        match line_size {
            Some(ls) => assert!(
                (ls - 1.0).abs() < 1e-9,
                "hard market: expected line_size = 1.0, got {ls}"
            ),
            None => panic!("expected LeadQuoteIssued, got: {events:?}"),
        }
    }

    #[test]
    fn insurer_books_line_share() {
        // on_policy_bound with line_share=0.5: cat_aggregate += sum_insured * 0.5
        // and capital increases by premium * 0.5 * (1 - expense_ratio).
        let expense_ratio = 0.30;
        let mut ins = Insurer::new(
            InsurerId(1), 10_000_000_000, 0.0, 0.0, 1.0, 0.3,
            expense_ratio, 0.0, None, None, 0.252, 0.0, 0.0, 1.0, 0.30, 0.0, 0.0,
            1.0, 1.0,
        );
        let sum_insured = 100_00u64; // 100 cents
        let premium = 20_00u64;      // 20 cents
        let initial_capital = ins.capital;
        ins.on_policy_bound(
            crate::types::PolicyId(1), sum_insured, premium,
            &[crate::events::Peril::WindstormAtlantic], 0.5,
        );
        let premium_share = (premium as f64 * 0.5).round() as i64;
        let net_premium = (premium_share as f64 * (1.0 - expense_ratio)).round() as i64;
        assert_eq!(
            ins.capital, initial_capital + net_premium,
            "capital must increase by net premium share"
        );
        assert_eq!(
            ins.cat_aggregate, (sum_insured as f64 * 0.5).round() as u64,
            "cat_aggregate must be scaled by line_share"
        );
    }

    #[test]
    fn floor_factor_zero_gives_full_line() {
        // floor_factor=0.0 → pricing_line = (factor - 0) / (1 - 0) = factor → clamp to 1.0
        // At any market_factor >= 1.0, line_size = 1.0.
        let ins = Insurer::new(
            InsurerId(1), 10_000_000_000, 0.0, 0.0, 1.0, 0.3,
            0.0, 0.0, None, None, 0.252, 0.0, 0.0, 1.0, 0.30, 0.0, 0.0,
            1.0, 1.0,
        );
        use crate::types::SubmissionId;
        let risk = Risk {
            sum_insured: ASSET_VALUE,
            territory: "US-SE".to_string(),
            perils_covered: vec![],
        };
        let events = ins.on_lead_quote_requested(Day(1), SubmissionId(1), InsuredId(1), &risk, 1.0);
        let line_size = events.iter().find_map(|(_, e)| {
            if let Event::LeadQuoteIssued { line_size, .. } = e { Some(*line_size) } else { None }
        });
        match line_size {
            Some(ls) => assert!(
                (ls - 1.0).abs() < 1e-9,
                "floor_factor=0.0 must yield line_size=1.0 at market_factor=1.0, got {ls}"
            ),
            None => panic!("expected LeadQuoteIssued, got: {events:?}"),
        }
    }

    // ── Volume-weighted EWMA ──────────────────────────────────────────────────

    #[test]
    fn vol_weight_is_one_for_stable_book_size() {
        // Year 1 establishes exposure_ewma = 0.3 × 10×AV.
        // Year 2 has same 10 policies → ytd.exposure (10×AV) > exposure_ewma (3×AV) → vol_weight = 1.0.
        // With vol_weight = 1.0, effective_alpha = ewma_credibility × 1.0 → standard EWMA formula.
        let mut ins = make_insurer(InsurerId(1), ASSET_VALUE as i64 * 100);

        // Year 1: 10 policies, small claim (realized_lf = 0.01).
        for i in 0..10u64 {
            ins.on_policy_bound(PolicyId(i + 1), ASSET_VALUE, 0, &[Peril::Attritional], 1.0);
        }
        let _ = ins.on_claim_settled(Day(100), ASSET_VALUE / 10, Peril::Attritional);
        let _ = ins.on_year_end(Day(360), ASSET_VALUE);

        // Year 2: same 10 policies, same claim.
        for i in 0..10u64 {
            ins.on_policy_bound(PolicyId(100 + i + 1), ASSET_VALUE, 0, &[Peril::Attritional], 1.0);
        }
        let _ = ins.on_claim_settled(Day(460), ASSET_VALUE / 10, Peril::Attritional);
        let _ = ins.on_year_end(Day(720), ASSET_VALUE);

        // realized_lf = (ASSET_VALUE/10) / (10×ASSET_VALUE) = 0.01
        // Year 1: effective_alpha = 0.3 (vol_weight=1.0, first year), elf_y1 = 0.3×0.01 + 0.7×0.239
        // Year 2: effective_alpha = 0.3 (vol_weight=1.0, exposure grows above EWMA norm)
        //         elf_y2 = 0.3×0.01 + 0.7×elf_y1 → standard formula unchanged
        let realized_lf: f64 = (ASSET_VALUE as f64 / 10.0) / (10.0 * ASSET_VALUE as f64);
        let elf_y1 = 0.3 * realized_lf + 0.7 * 0.239;
        let elf_y2 = 0.3 * realized_lf + 0.7 * elf_y1;
        let expected_atp = (elf_y2 * ASSET_VALUE as f64 / 0.70).round() as u64;
        assert_eq!(
            quote_atp(&ins), expected_atp,
            "stable volume → vol_weight = 1.0 → standard EWMA formula preserved"
        );
    }

    #[test]
    fn own_cr_ewma_spike_suppressed_on_book_shrinkage() {
        // Build up exposure_ewma over 3 benign years (20 policies each).
        // In the spike year: only 1 policy, LR ≈ 1500%.
        // Without vol_weight: CR EWMA shift ≈ 5.0; with vol_weight ≈ 0.098: shift < 1.0.
        let initial_capital = ASSET_VALUE as i64 * 200;
        let premium_per_policy = 1_000_000_u64; // 100k USD per policy
        let mut ins = Insurer::new(
            InsurerId(1), initial_capital, 0.239, 0.0, 0.70, 0.3,
            0.344, 0.0, None, None, 0.252, 0.0, 0.0, 1.0, 0.30, 0.0, 0.0, 1.0, 1.0,
        );

        // 3 benign years: 20 policies each, no claims → own_cr = expense_ratio = 0.344.
        for year in 0..3u32 {
            for i in 0..20u64 {
                ins.on_policy_bound(
                    PolicyId(year as u64 * 100 + i + 1), ASSET_VALUE,
                    premium_per_policy, &[Peril::Attritional], 1.0,
                );
            }
            let _ = ins.on_year_end(Day(360 * (year + 1) as u64), ASSET_VALUE);
        }
        // exposure_ewma ≈ 10.2×ASSET_VALUE after 3 years.

        let cr_ewma_before = ins.own_cr_ewma().expect("own_cr_ewma must be set after 3 benign years");

        // Spike year: 1 policy, enormous claim (LR ≈ 1500%).
        ins.on_policy_bound(PolicyId(9999), ASSET_VALUE, premium_per_policy, &[Peril::Attritional], 1.0);
        let _ = ins.on_claim_settled(Day(1081), premium_per_policy * 15, Peril::Attritional);
        let _ = ins.on_year_end(Day(1440), ASSET_VALUE);

        let cr_ewma_after = ins.own_cr_ewma().expect("own_cr_ewma must still be set after spike year");
        let shift = cr_ewma_after - cr_ewma_before;
        // Without vol_weight: shift ≈ OWN_CR_EWMA_ALPHA × 15.0 ≈ 5.0
        // With vol_weight ≈ 0.098: shift ≈ 0.333 × 0.098 × 15.0 ≈ 0.49 → well below 1.0
        assert!(
            shift < 1.0,
            "own_cr_ewma spike suppressed by vol_weight: shift={shift:.4}, before={cr_ewma_before:.4}, after={cr_ewma_after:.4}"
        );
    }

    #[test]
    fn attritional_elf_stable_after_single_policy_spike() {
        // 2 benign years with 20 policies → exposure_ewma ≈ 10.2×AV.
        // Spike year: 1 policy, realized_lf = 50%.
        // Without vol_weight: att_elf movement = 0.3 × (0.5 − old_elf).
        // With vol_weight ≈ 0.098: movement < 0.3 × (0.5 − old_elf) × 0.1.
        let mut ins = make_insurer(InsurerId(1), ASSET_VALUE as i64 * 200);

        // 2 benign years: 20 policies each, no claims.
        for year in 0..2u32 {
            for i in 0..20u64 {
                ins.on_policy_bound(
                    PolicyId(year as u64 * 100 + i + 1), ASSET_VALUE,
                    0, &[Peril::Attritional], 1.0,
                );
            }
            let _ = ins.on_year_end(Day(360 * (year + 1) as u64), ASSET_VALUE);
        }
        // exposure_ewma ≈ 10.2×AV; elf ≈ 0.239×0.7^2 ≈ 0.117

        let atp_before = quote_atp(&ins);
        let elf_before = atp_before as f64 * 0.70 / ASSET_VALUE as f64;

        // Spike year: 1 policy, realized_lf = 50%.
        ins.on_policy_bound(PolicyId(9999), ASSET_VALUE, 0, &[Peril::Attritional], 1.0);
        let _ = ins.on_claim_settled(Day(721), ASSET_VALUE / 2, Peril::Attritional);
        let _ = ins.on_year_end(Day(1080), ASSET_VALUE);

        let atp_after = quote_atp(&ins);
        let elf_after = atp_after as f64 * 0.70 / ASSET_VALUE as f64;
        let actual_movement = elf_after - elf_before;

        // vol_weight = 1*AV / 10.2*AV ≈ 0.098 → effective_alpha ≈ 0.029
        // Bound: vol_weight < 0.1 → movement < 0.1 × full_ewma_movement
        let full_ewma_movement = 0.3_f64 * (0.5 - elf_before);
        assert!(
            actual_movement < full_ewma_movement * 0.1,
            "single-policy spike damped by vol_weight: actual={actual_movement:.6}, bound={:.6}",
            full_ewma_movement * 0.1
        );
    }

    #[test]
    fn vol_weight_does_not_affect_first_year() {
        // First year: exposure_ewma = 0 → vol_weight = 1.0.
        // EWMA behaves exactly as without vol_weight (existing test coverage preserved).
        let mut ins = make_insurer(InsurerId(1), ASSET_VALUE as i64 * 10);
        ins.on_policy_bound(PolicyId(1), ASSET_VALUE, 0, &[Peril::Attritional], 1.0);
        let _ = ins.on_claim_settled(Day(100), ASSET_VALUE / 2, Peril::Attritional);
        let _ = ins.on_year_end(Day(360), ASSET_VALUE);

        // Expected: standard EWMA, realized_lf = 0.5, α = 0.3.
        // new_elf = 0.3×0.5 + 0.7×0.239 = 0.3173
        let expected_elf = 0.3 * 0.5 + 0.7 * 0.239;
        let expected_atp = (expected_elf * ASSET_VALUE as f64 / 0.70).round() as u64;
        assert_eq!(
            quote_atp(&ins), expected_atp,
            "first year (exposure_ewma=0) → vol_weight=1.0 → standard EWMA formula unchanged"
        );

        // For own_cr_ewma: first year with premium → vol_weight=1.0 → own_cr_ewma = observed CR.
        let premium = 1_000_000_u64;
        let mut ins2 = Insurer::new(
            InsurerId(2), ASSET_VALUE as i64 * 10,
            0.239, 0.0, 0.70, 0.3, 0.344, 0.0, None, None, 0.252,
            0.0, 0.0, 1.0, 0.30, 0.0, 0.0, 1.0, 1.0,
        );
        ins2.on_policy_bound(PolicyId(1), ASSET_VALUE, premium, &[Peril::Attritional], 1.0);
        let _ = ins2.on_claim_settled(Day(100), premium * 5, Peril::Attritional);
        let _ = ins2.on_year_end(Day(360), ASSET_VALUE);

        // vol_weight = 1.0 (first year) → None case → vol_weight×own_cr + (1-vol_weight)×1.0 = own_cr.
        let expected_lr = (premium * 5) as f64 / premium as f64; // 5.0
        let expected_cr = expected_lr + 0.344;
        let actual = ins2.own_cr_ewma().expect("own_cr_ewma must be set after year 1 with premium");
        assert!(
            (actual - expected_cr).abs() < 1e-9,
            "first year: own_cr_ewma must equal observed CR (no dampening): actual={actual}, expected={expected_cr}"
        );
    }

}
