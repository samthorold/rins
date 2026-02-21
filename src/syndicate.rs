use std::collections::HashMap;

use rand::Rng;

use crate::events::{Event, Peril, Risk};
use crate::types::{Day, SubmissionId, SyndicateId, Year};

/// Calibration parameters for the actuarial pricing channel.
pub struct ActuarialParams {
    /// Prior expected loss ratio per line of business.
    pub base_loss_ratios: HashMap<String, f64>,
    /// Catastrophe loading multipliers per territory.
    pub territory_factors: HashMap<String, f64>,
    /// Severity loading multipliers per peril.
    pub peril_factors: HashMap<Peril, f64>,
    /// Half-credibility volume (Bühlmann): observations needed for 50% credibility weight.
    pub credibility_k: f64,
    /// EWMA decay weight on the most recent observation.
    pub ewma_alpha: f64,
}

impl Default for ActuarialParams {
    fn default() -> Self {
        let base_loss_ratios = [
            ("property".to_string(), 0.60),
            ("marine".to_string(), 0.65),
            ("casualty".to_string(), 0.70),
        ]
        .into_iter()
        .collect();

        let territory_factors = [
            ("US-SE".to_string(), 1.4),
            ("US-CA".to_string(), 1.3),
            ("EU".to_string(), 1.1),
            ("UK".to_string(), 1.0),
        ]
        .into_iter()
        .collect();

        let peril_factors = [
            (Peril::WindstormAtlantic, 1.5),
            (Peril::WindstormEuropean, 1.3),
            (Peril::EarthquakeUS, 1.6),
            (Peril::EarthquakeJapan, 1.6),
            (Peril::Flood, 1.2),
            (Peril::Attritional, 0.8),
        ]
        .into_iter()
        .collect();

        ActuarialParams {
            base_loss_ratios,
            territory_factors,
            peril_factors,
            credibility_k: 50.0,
            ewma_alpha: 0.3,
        }
    }
}

/// Mutable EWMA state per line of business.
struct LineExperience {
    ewma_loss_ratio: f64,
    volume: u64,
}

pub struct Syndicate {
    pub id: SyndicateId,
    pub capital: u64,
    pub initial_capital: u64,
    pub is_active: bool,
    pub aggregate_written_premium: u64,
    /// Per-submission premium reserved for in-flight quotes that have been issued but not yet bound.
    /// Keyed by SubmissionId; value = atp/n_eligible for that quote.
    /// Released exactly at bind (on_policy_bound_as_panelist) or cleared at year-end.
    pub quoted_exposure: HashMap<SubmissionId, u64>,
    pub solvency_floor_pct: f64,
    pub max_premium_ratio: f64,
    pub max_single_risk_pct: f64,
    pub rate_on_line_bps: u32, // basis points, e.g. 500 = 5% rate on line
    pub actuarial: ActuarialParams,
    experience: HashMap<String, LineExperience>,
}

impl Syndicate {
    pub fn new(id: SyndicateId, initial_capital: u64, rate_on_line_bps: u32) -> Self {
        Syndicate {
            id,
            capital: initial_capital,
            initial_capital,
            is_active: true,
            aggregate_written_premium: 0,
            quoted_exposure: HashMap::new(),
            solvency_floor_pct: 0.20,
            max_premium_ratio: 0.50,
            max_single_risk_pct: 0.30,
            rate_on_line_bps,
            actuarial: ActuarialParams::default(),
            experience: HashMap::new(),
        }
    }

    pub fn with_actuarial(mut self, params: ActuarialParams) -> Self {
        self.actuarial = params;
        self
    }

    /// Returns true if this syndicate is active and the risk's limit fits within
    /// the per-risk exposure limit. Used by dispatch to compute n_eligible.
    pub fn is_eligible_for_risk(&self, risk: &Risk) -> bool {
        if !self.is_active {
            return false;
        }
        let max_loss = (self.initial_capital as f64 * self.max_single_risk_pct) as u64;
        risk.limit <= max_loss
    }

    /// Compute the Actuarial Technical Price for a risk.
    ///
    /// `industry_benchmark` is the fallback loss ratio for unknown lines.
    pub fn atp(&self, risk: &Risk, industry_benchmark: f64) -> u64 {
        if risk.limit == 0 {
            return 0;
        }

        let lob = &risk.line_of_business;

        // 1. Base loss ratio from params, fallback to industry benchmark.
        let base = *self
            .actuarial
            .base_loss_ratios
            .get(lob)
            .unwrap_or(&industry_benchmark);

        // 2. Own experience (ewma, volume); default to (base, 0) if unseen.
        let (ewma, volume) = self
            .experience
            .get(lob)
            .map(|e| (e.ewma_loss_ratio, e.volume))
            .unwrap_or((base, 0));

        // 3. Credibility weight.
        let z = volume as f64 / (volume as f64 + self.actuarial.credibility_k);

        // 4. Blended loss ratio.
        let blended = z * ewma + (1.0 - z) * industry_benchmark;

        // 5. Territory factor.
        let territory_f = *self
            .actuarial
            .territory_factors
            .get(&risk.territory)
            .unwrap_or(&1.0);

        // 6. Peril factor — max across covered perils; default 1.0 when list is empty.
        // "Worst-peril dominates": perils are treated as alternative event triggers,
        // so only the most hazardous peril drives the loading. If perils are instead
        // independent severity contributors, this should be multiplicative or additive.
        // Calibration-sensitive choice — revisit when multi-peril claims data is available.
        let peril_f = risk
            .perils_covered
            .iter()
            .map(|p| *self.actuarial.peril_factors.get(p).unwrap_or(&1.0))
            .reduce(f64::max)
            .unwrap_or(1.0);

        // 7. Layer factor (pro-rata by layer position, Lloyd's market-mechanics §1).
        // Approximates the fraction of expected loss falling in this layer relative
        // to the total tower. `sum_insured` is intentionally ignored; a first-dollar
        // cover always gets layer_f=1.0 regardless of the overall exposure size.
        // This is the standard Lloyd's simplification acceptable for MVP.
        let layer_f = risk.limit as f64 / (risk.attachment as f64 + risk.limit as f64);

        // 8. ATP.
        (risk.limit as f64 * blended * territory_f * peril_f * layer_f).round() as u64
    }

    /// Update the EWMA loss ratio for a line of business.
    ///
    /// Called at year-end once loss ratios are known. Currently used only by tests;
    /// will be wired into `on_year_end` when the simulation is ready.
    pub fn observe_line_loss_ratio(&mut self, line: &str, loss_ratio: f64) {
        let base = *self
            .actuarial
            .base_loss_ratios
            .get(line)
            .unwrap_or(&loss_ratio);
        let alpha = self.actuarial.ewma_alpha;
        let entry = self.experience.entry(line.to_string()).or_insert(LineExperience {
            ewma_loss_ratio: base,
            volume: 0,
        });
        entry.ewma_loss_ratio = alpha * loss_ratio + (1.0 - alpha) * entry.ewma_loss_ratio;
        entry.volume += 1;
    }

    /// Price and issue (or decline) a quote for a submission.
    /// Premium is set by the actuarial channel (ATP).
    /// `industry_benchmark` is the market-wide loss ratio from the previous year's YearStats.
    /// `n_eligible` is the number of syndicates that pass the per-risk eligibility check;
    /// used to compute the expected panel-share premium for capacity reservation.
    /// Returns QuoteDeclined if inactive or annual capacity would be breached.
    pub fn on_quote_requested(
        &mut self,
        day: Day,
        submission_id: SubmissionId,
        risk: &Risk,
        is_lead: bool,
        lead_premium: Option<u64>,
        industry_benchmark: f64,
        n_eligible: usize,
    ) -> (Day, Event) {
        // Belt-and-suspenders: submission filtering should exclude inactive syndicates,
        // but guard here too.
        if !self.is_active {
            return (day, Event::QuoteDeclined { submission_id, syndicate_id: self.id });
        }

        // Per-risk size limit: decline if the risk's limit exceeds our maximum single-risk exposure.
        let max_single_risk_loss = (self.initial_capital as f64 * self.max_single_risk_pct) as u64;
        if risk.limit > max_single_risk_loss {
            return (day, Event::QuoteDeclined { submission_id, syndicate_id: self.id });
        }

        let atp = self.atp(risk, industry_benchmark);

        // Capacity check: decline if this risk would push annual premium over the cap.
        // `quoted_exposure` tracks in-flight quotes not yet bound (prevents
        // over-commitment when many submissions are quoted concurrently).
        let max_capacity = (self.initial_capital as f64 * self.max_premium_ratio) as u64;
        let in_flight: u64 = self.quoted_exposure.values().sum();
        if self.aggregate_written_premium + in_flight + atp > max_capacity {
            return (day, Event::QuoteDeclined { submission_id, syndicate_id: self.id });
        }

        // Reserve the expected panel-share premium for this quote.
        // Expected share = 1/n_eligible (equal split across eligible syndicates).
        let n = n_eligible.max(1) as u64;
        self.quoted_exposure.insert(submission_id, atp / n);

        // is_lead and lead_premium are inputs to the underwriter channel — deferred.
        let _ = (is_lead, lead_premium);
        (
            day,
            Event::QuoteIssued {
                submission_id,
                syndicate_id: self.id,
                premium: atp,
                is_lead,
            },
        )
    }

    /// Deduct a settled claim from capital.
    /// Returns `true` if capital has dropped below the solvency floor after deduction.
    pub fn on_claim_settled(&mut self, amount: u64) -> bool {
        self.capital = self.capital.saturating_sub(amount);
        self.capital < (self.initial_capital as f64 * self.solvency_floor_pct) as u64
    }

    /// Record that a policy was bound with this syndicate on the panel.
    /// Removes the in-flight reservation for this submission and records the written premium.
    pub fn on_policy_bound_as_panelist(&mut self, submission_id: SubmissionId, premium: u64) {
        self.quoted_exposure.remove(&submission_id);
        self.aggregate_written_premium += premium;
    }

    /// Called by the coordinator at year-end.
    /// Updates the EWMA loss ratio for each line from the market-wide realised ratios.
    /// `line_loss_ratios` is keyed by line of business and computed by the coordinator
    /// from YTD premiums and claims; only lines with sufficient premium volume are present.
    pub fn on_year_end(
        &mut self,
        _year: Year,
        line_loss_ratios: &std::collections::HashMap<String, f64>,
        _rng: &mut impl Rng,
    ) {
        // Annual policies expire at year-end; release written-premium capacity for next year.
        self.aggregate_written_premium = 0;
        self.quoted_exposure.clear();
        for (line, &loss_ratio) in line_loss_ratios {
            self.observe_line_loss_ratio(line, loss_ratio);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::events::Peril;
    use crate::types::SubmissionId;
    use proptest::prelude::*;

    fn make_risk(limit: u64) -> Risk {
        Risk {
            line_of_business: "property".to_string(),
            sum_insured: limit * 2,
            territory: "US-SE".to_string(),
            limit,
            attachment: 0,
            perils_covered: vec![Peril::WindstormAtlantic],
        }
    }

    fn fresh_syndicate() -> Syndicate {
        Syndicate::new(SyndicateId(1), 10_000_000, 500)
    }

    // ── Existing tests ─────────────────────────────────────────────────────

    #[test]
    fn capital_depletes_by_exact_claim_amount() {
        let mut s = Syndicate::new(SyndicateId(1), 10_000_000, 500);
        s.on_claim_settled(300_000);
        assert_eq!(s.capital, 9_700_000);
    }

    #[test]
    fn capital_saturates_at_zero() {
        let mut s = Syndicate::new(SyndicateId(1), 100_000, 500);
        s.on_claim_settled(500_000);
        assert_eq!(s.capital, 0);
    }

    // 0. on_quote_requested returns the same premium as atp() called directly.
    #[test]
    fn on_quote_requested_uses_atp() {
        let mut s = fresh_syndicate();
        let risk = make_risk(1_000_000);
        let benchmark = 0.65;
        let atp = s.atp(&risk, benchmark);
        let (_, event) =
            s.on_quote_requested(crate::types::Day(0), SubmissionId(1), &risk, true, None, benchmark, 1);
        match event {
            Event::QuoteIssued { premium, .. } => {
                assert_eq!(premium, atp);
            }
            _ => panic!("expected QuoteIssued"),
        }
    }

    // ── Actuarial channel tests ────────────────────────────────────────────

    // 1. Fresh syndicate (volume=0) → z=0 → ATP purely from industry benchmark.
    #[test]
    fn atp_fresh_syndicate_uses_benchmark() {
        let s = fresh_syndicate();
        let risk = Risk {
            line_of_business: "property".to_string(),
            sum_insured: 2_000_000,
            territory: "UK".to_string(), // factor 1.0
            limit: 1_000_000,
            attachment: 0,
            perils_covered: vec![Peril::Attritional], // factor 0.8
        };
        let benchmark = 0.60;
        // layer_f = 1_000_000 / (0 + 1_000_000) = 1.0
        // blended = 0 * ewma + 1.0 * 0.60 = 0.60
        // atp = 1_000_000 * 0.60 * 1.0 * 0.8 * 1.0 = 480_000
        assert_eq!(s.atp(&risk, benchmark), 480_000);
    }

    // 2. Territory factor scales linearly (US-SE vs UK).
    #[test]
    fn atp_territory_factor_scales_linearly() {
        let s = fresh_syndicate();
        let base_risk = Risk {
            line_of_business: "property".to_string(),
            sum_insured: 2_000_000,
            territory: "UK".to_string(),
            limit: 1_000_000,
            attachment: 0,
            perils_covered: vec![Peril::Attritional],
        };
        let us_risk = Risk {
            territory: "US-SE".to_string(),
            ..base_risk.clone()
        };
        let atp_uk = s.atp(&base_risk, 0.60) as f64;
        let atp_us = s.atp(&us_risk, 0.60) as f64;
        let ratio = atp_us / atp_uk;
        // 1.4 / 1.0 = 1.4
        assert!((ratio - 1.4).abs() < 0.01, "ratio={ratio}");
    }

    // 3. Peril factor uses the maximum across covered perils.
    #[test]
    fn atp_peril_factor_uses_max() {
        let s = fresh_syndicate();
        let risk_multi = Risk {
            line_of_business: "property".to_string(),
            sum_insured: 2_000_000,
            territory: "UK".to_string(),
            limit: 1_000_000,
            attachment: 0,
            perils_covered: vec![Peril::WindstormAtlantic, Peril::Attritional],
        };
        let risk_single = Risk {
            perils_covered: vec![Peril::WindstormAtlantic],
            ..risk_multi.clone()
        };
        // Both should produce the same ATP: max(1.5, 0.8) = 1.5
        assert_eq!(s.atp(&risk_multi, 0.60), s.atp(&risk_single, 0.60));
    }

    // 4. Zero limit returns zero.
    #[test]
    fn atp_zero_limit_returns_zero() {
        let s = fresh_syndicate();
        let risk = Risk {
            line_of_business: "property".to_string(),
            sum_insured: 0,
            territory: "UK".to_string(),
            limit: 0,
            attachment: 0,
            perils_covered: vec![Peril::Attritional],
        };
        assert_eq!(s.atp(&risk, 0.60), 0);
    }

    // 5. Attachment halves the layer factor when attachment == limit.
    #[test]
    fn atp_attachment_reduces_exposure() {
        let s = fresh_syndicate();
        let risk_no_attach = Risk {
            line_of_business: "property".to_string(),
            sum_insured: 3_000_000,
            territory: "UK".to_string(),
            limit: 1_000_000,
            attachment: 0,
            perils_covered: vec![Peril::Attritional],
        };
        let risk_with_attach = Risk {
            attachment: 1_000_000,
            ..risk_no_attach.clone()
        };
        let atp_no = s.atp(&risk_no_attach, 0.60) as f64;
        let atp_with = s.atp(&risk_with_attach, 0.60) as f64;
        // layer_f: 1.0 vs 0.5 → ratio should be ~2.0
        assert!((atp_no / atp_with - 2.0).abs() < 0.01, "no={atp_no} with={atp_with}");
    }

    // 6. Very high attachment → ATP near zero.
    #[test]
    fn atp_high_attachment_near_zero() {
        let s = fresh_syndicate();
        let limit = 1_000_000u64;
        let risk = Risk {
            line_of_business: "property".to_string(),
            sum_insured: 100_000_000,
            territory: "UK".to_string(),
            limit,
            attachment: limit * 99,
            perils_covered: vec![Peril::Attritional],
        };
        // layer_f = 1_000_000 / (99_000_000 + 1_000_000) = 0.01
        // atp = 1_000_000 * 0.60 * 1.0 * 0.8 * 0.01 = 4_800
        let atp = s.atp(&risk, 0.60);
        assert!(atp < 10_000, "atp={atp}");
    }

    // 7. Unknown territory → factor defaults to 1.0, no panic.
    #[test]
    fn atp_unknown_territory_no_loading() {
        let s = fresh_syndicate();
        let known = Risk {
            line_of_business: "property".to_string(),
            sum_insured: 2_000_000,
            territory: "UK".to_string(),
            limit: 1_000_000,
            attachment: 0,
            perils_covered: vec![Peril::Attritional],
        };
        let unknown = Risk {
            territory: "MARS".to_string(),
            ..known.clone()
        };
        // UK has factor 1.0, MARS defaults to 1.0 — should produce same ATP.
        assert_eq!(s.atp(&known, 0.60), s.atp(&unknown, 0.60));
    }

    // 9. EWMA converges toward observed loss ratio after many observations.
    #[test]
    fn ewma_converges_toward_observed_ratio() {
        let mut s = fresh_syndicate();
        for _ in 0..30 {
            s.observe_line_loss_ratio("property", 0.80);
        }
        let exp = s.experience.get("property").unwrap();
        assert!(
            (exp.ewma_loss_ratio - 0.80).abs() < 0.01,
            "ewma={} expected ~0.80",
            exp.ewma_loss_ratio
        );
    }

    // 10. Single EWMA update is arithmetically correct.
    #[test]
    fn ewma_single_update_correct() {
        let mut s = fresh_syndicate();
        s.observe_line_loss_ratio("property", 0.80);
        // base = 0.60, alpha = 0.3 → 0.3 * 0.80 + 0.7 * 0.60 = 0.24 + 0.42 = 0.66
        let exp = s.experience.get("property").unwrap();
        assert!(
            (exp.ewma_loss_ratio - 0.66).abs() < 1e-10,
            "ewma={} expected 0.66",
            exp.ewma_loss_ratio
        );
    }

    // 11. At volume == credibility_k, blended is halfway between ewma and benchmark.
    #[test]
    fn credibility_increases_with_volume() {
        let mut s = fresh_syndicate(); // credibility_k = 50
        // Push ewma to ~0.80 with 50 observations.
        for _ in 0..50 {
            s.observe_line_loss_ratio("property", 0.80);
        }
        let exp = s.experience.get("property").unwrap();
        assert_eq!(exp.volume, 50);
        let z = 50.0_f64 / (50.0 + 50.0); // 0.5
        assert!((z - 0.5).abs() < 1e-10);
        // blended = 0.5 * ewma + 0.5 * benchmark — both weights equal.
        let blended = z * exp.ewma_loss_ratio + (1.0 - z) * 0.60;
        let expected_mid = (exp.ewma_loss_ratio + 0.60) / 2.0;
        assert!((blended - expected_mid).abs() < 1e-10);
    }

    // 12. Low volume → benchmark dominates.
    #[test]
    fn credibility_low_volume_benchmark_dominant() {
        let mut s = fresh_syndicate(); // credibility_k = 50
        for _ in 0..5 {
            s.observe_line_loss_ratio("property", 0.90);
        }
        let exp = s.experience.get("property").unwrap();
        let z = 5.0_f64 / (5.0 + 50.0); // ~0.0909
        // Benchmark weight = 1 - z > 0.90
        assert!(1.0 - z > 0.90, "benchmark weight={}", 1.0 - z);
        let _ = exp;
    }

    // 13. At high volume, ATP ≈ limit × ewma × territory_f × peril_f × layer_f.
    #[test]
    fn atp_blends_own_experience_at_high_volume() {
        let mut s = fresh_syndicate();
        for _ in 0..500 {
            s.observe_line_loss_ratio("property", 0.75);
        }
        let risk = Risk {
            line_of_business: "property".to_string(),
            sum_insured: 2_000_000,
            territory: "UK".to_string(),
            limit: 1_000_000,
            attachment: 0,
            perils_covered: vec![Peril::Attritional],
        };
        let exp = s.experience.get("property").unwrap();
        let z = 500.0_f64 / (500.0 + 50.0);
        let blended = z * exp.ewma_loss_ratio + (1.0 - z) * 0.60;
        // territory_f=1.0, peril_f=0.8, layer_f=1.0
        let expected = (1_000_000.0 * blended * 1.0 * 0.8 * 1.0).round() as u64;
        let atp = s.atp(&risk, 0.60);
        let err = (atp as f64 - expected as f64).abs() / expected as f64;
        assert!(err < 0.01, "atp={atp} expected={expected} err={err:.4}");
    }

    // 14. Observing a high loss ratio increases ATP relative to a fresh syndicate.
    #[test]
    fn atp_reflects_bad_year_experience() {
        let mut experienced = fresh_syndicate();
        for _ in 0..20 {
            experienced.observe_line_loss_ratio("property", 0.95);
        }
        let fresh = fresh_syndicate();
        let risk = make_risk(1_000_000);
        let benchmark = 0.60;
        assert!(
            experienced.atp(&risk, benchmark) > fresh.atp(&risk, benchmark),
            "experienced ATP should exceed fresh ATP after high-loss observations"
        );
    }

    // 15. ATP is monotone non-increasing as attachment increases (proptest).
    proptest! {
        #[test]
        fn atp_decreases_as_attachment_increases(
            limit in 1_000_u64..10_000_000_u64,
            attach_a in 0_u64..10_000_000_u64,
            attach_b in 0_u64..10_000_000_u64,
        ) {
            let s = fresh_syndicate();
            let make = |attachment: u64| Risk {
                line_of_business: "property".to_string(),
                sum_insured: limit * 2 + attachment,
                territory: "UK".to_string(),
                limit,
                attachment,
                perils_covered: vec![Peril::Attritional],
            };
            let (lo, hi) = if attach_a <= attach_b {
                (attach_a, attach_b)
            } else {
                (attach_b, attach_a)
            };
            let atp_lo = s.atp(&make(lo), 0.60);
            let atp_hi = s.atp(&make(hi), 0.60);
            prop_assert!(atp_lo >= atp_hi,
                "attachment lo={lo} atp={atp_lo} > attachment hi={hi} atp={atp_hi}");
        }
    }

    // 17. on_year_end drives the same EWMA update as observe_line_loss_ratio directly.
    #[test]
    fn on_year_end_updates_ewma() {
        use rand::SeedableRng;
        let mut s = fresh_syndicate();
        let mut rng = rand_chacha::ChaCha20Rng::seed_from_u64(0);
        let mut line_ratios = std::collections::HashMap::new();
        line_ratios.insert("property".to_string(), 0.80_f64);
        s.on_year_end(crate::types::Year(1), &line_ratios, &mut rng);
        let exp = s.experience.get("property").unwrap();
        // base=0.60, alpha=0.3 → ewma = 0.3*0.80 + 0.7*0.60 = 0.24+0.42 = 0.66
        assert!(
            (exp.ewma_loss_ratio - 0.66).abs() < 1e-10,
            "ewma={} expected 0.66",
            exp.ewma_loss_ratio
        );
        assert_eq!(exp.volume, 1);
    }

    // 18. Finding 3: after 1 observation, blended is a convex combination of
    //     ewma and benchmark (never outside their range).
    //     Demonstrates that the volume=0→1 transition is bounded.
    #[test]
    fn blended_at_volume_one_is_convex_combination_of_ewma_and_benchmark() {
        // base=0.60, benchmark=0.80, observation=0.70 (between the two)
        // ewma after 1 obs: 0.3*0.70 + 0.7*0.60 = 0.63
        // z = 1/(1+50) ≈ 0.0196
        // blended = 0.0196*0.63 + 0.9804*0.80 ≈ 0.797  — between 0.63 and 0.80
        let mut s = fresh_syndicate(); // credibility_k=50, base["property"]=0.60
        s.observe_line_loss_ratio("property", 0.70);
        let exp = s.experience.get("property").unwrap();
        assert_eq!(exp.volume, 1);

        let benchmark = 0.80_f64;
        let z = exp.volume as f64 / (exp.volume as f64 + s.actuarial.credibility_k);
        let blended = z * exp.ewma_loss_ratio + (1.0 - z) * benchmark;

        // blended is always a convex combination: lo ≤ blended ≤ hi.
        let lo = benchmark.min(exp.ewma_loss_ratio);
        let hi = benchmark.max(exp.ewma_loss_ratio);
        assert!(
            blended >= lo && blended <= hi,
            "blended={blended:.6} not in [{lo:.4}, {hi:.4}]: not a convex combination"
        );
    }

    // --- Claim-capital tests ---

    #[test]
    fn capital_after_sequential_claims() {
        let mut s = fresh_syndicate(); // capital = 10_000_000
        s.on_claim_settled(300_000);
        s.on_claim_settled(200_000);
        assert_eq!(s.capital, 9_500_000);
    }

    #[test]
    fn end_to_end_loss_reduces_syndicate_capital() {
        use crate::events::{Panel, PanelEntry, Peril, Risk};
        use crate::market::Market;
        use crate::types::{Day, SubmissionId};

        // Build synthetic policy: US-SE, WindstormAtlantic, limit=1_000_000, attach=100_000.
        let risk = Risk {
            line_of_business: "property".to_string(),
            sum_insured: 2_000_000,
            territory: "US-SE".to_string(),
            limit: 1_000_000,
            attachment: 100_000,
            perils_covered: vec![Peril::WindstormAtlantic],
        };
        // Single-syndicate panel: Syn 1 takes 100%.
        let panel = Panel {
            entries: vec![PanelEntry {
                syndicate_id: SyndicateId(1),
                share_bps: 10_000,
                premium: 50_000,
            }],
        };

        let mut market = Market::new();
        market.on_policy_bound(SubmissionId(1), risk, panel, crate::types::Year(1));

        // Fire a loss: severity 600_000 → net_loss = 600_000 - 100_000 = 500_000.
        let claim_events =
            market.on_loss_event(Day(0), "US-SE", Peril::WindstormAtlantic, 600_000);

        // Apply ClaimSettled events to the syndicate.
        let mut syn = fresh_syndicate(); // SyndicateId(1), capital = 10_000_000
        for (_, ev) in &claim_events {
            if let crate::events::Event::ClaimSettled { syndicate_id, amount, .. } = ev {
                if *syndicate_id == SyndicateId(1) {
                    syn.on_claim_settled(*amount);
                }
            }
        }

        // net_loss = 500_000; share = 10_000 / 10_000 → deduction = 500_000.
        assert_eq!(syn.capital, 9_500_000, "capital should be initial - net_loss");
    }

    // ── Per-risk size limit tests ──────────────────────────────────────────────

    #[test]
    fn single_risk_limit_causes_decline() {
        // Small syndicate: 8_000_000_000 capital, 0.30 → max single-risk = 2_400_000_000.
        let mut s = Syndicate::new(SyndicateId(1), 8_000_000_000, 500);
        assert_eq!(s.max_single_risk_pct, 0.30);

        // Risk with limit 3_000_000_000 > 2_400_000_000 → must decline.
        let over_limit = make_risk(3_000_000_000);
        let (_, event) =
            s.on_quote_requested(crate::types::Day(0), SubmissionId(1), &over_limit, false, None, 0.65, 1);
        assert!(
            matches!(event, Event::QuoteDeclined { .. }),
            "expected QuoteDeclined for oversized risk, got {event:?}"
        );

        // Risk with limit 2_000_000_000 < 2_400_000_000 → must issue.
        let under_limit = make_risk(2_000_000_000);
        let (_, event) =
            s.on_quote_requested(crate::types::Day(0), SubmissionId(2), &under_limit, false, None, 0.65, 1);
        assert!(
            matches!(event, Event::QuoteIssued { .. }),
            "expected QuoteIssued for acceptable risk, got {event:?}"
        );
    }

    // ── Capacity and solvency tests ────────────────────────────────────────────

    /// Verifies that quoted_exposure tracks each in-flight quote by submission ID
    /// and that binding one quote removes it while recording written premium.
    #[test]
    fn quoted_exposure_tracks_per_submission() {
        // Syndicate with 10_000_000 capital → max_capacity = 5_000_000.
        // With n_eligible=3, each reservation = ATP/3. Three quotes together well under capacity.
        let mut s = Syndicate::new(SyndicateId(1), 10_000_000, 500);
        let risk = make_risk(1_000_000);
        let day = crate::types::Day(0);
        let benchmark = 0.65;

        // Issue 3 quotes with distinct submission IDs.
        let mut premiums = vec![];
        for i in 1u64..=3 {
            let (_, event) =
                s.on_quote_requested(day, SubmissionId(i), &risk, false, None, benchmark, 3);
            match event {
                Event::QuoteIssued { premium, .. } => premiums.push(premium),
                _ => panic!("expected QuoteIssued for submission {i}, got {event:?}"),
            }
        }
        assert_eq!(s.quoted_exposure.len(), 3, "should track 3 in-flight quotes");

        // Bind the first submission.
        s.on_policy_bound_as_panelist(SubmissionId(1), premiums[0]);
        assert_eq!(s.quoted_exposure.len(), 2, "binding one quote should remove it from quoted_exposure");
        assert_eq!(
            s.aggregate_written_premium, premiums[0],
            "written premium should equal first policy's premium"
        );
    }

    /// Verifies that quoted_exposure from in-flight quotes prevents over-commitment
    /// when many submissions are quoted concurrently.
    #[test]
    fn quoted_exposure_blocks_over_commitment() {
        // Syndicate with 10_000_000 capital → max_capacity = 5_000_000.
        // Each quote with n_eligible=1 reserves the full ATP (~1_365_000 for make_risk(1_000_000)).
        // After 3 quotes: sum(quoted_exposure) ≈ 4_095_000. 4th quote must be declined.
        let mut s = Syndicate::new(SyndicateId(1), 10_000_000, 500);
        let risk = make_risk(1_000_000);
        let day = crate::types::Day(0);

        // First 3 quotes should be issued.
        for i in 1..=3 {
            let (_, event) = s.on_quote_requested(day, SubmissionId(i), &risk, false, None, 0.65, 1);
            assert!(
                matches!(event, Event::QuoteIssued { .. }),
                "quote {i} should be issued, got {event:?}"
            );
        }
        // 4th quote should be declined due to quoted_exposure exhausting capacity.
        let (_, event) = s.on_quote_requested(day, SubmissionId(4), &risk, false, None, 0.65, 1);
        assert!(
            matches!(event, Event::QuoteDeclined { .. }),
            "4th quote should be declined when quoted_exposure exhausts capacity, got {event:?}"
        );
    }

    #[test]
    fn at_capacity_syndicate_declines_quote() {
        let mut s = Syndicate::new(SyndicateId(1), 10_000_000, 500);
        // max_capacity = 10_000_000 * 0.50 = 5_000_000
        // ATP for this risk ≈ 1_365_000 → 4_999_999 + 1_365_000 > 5_000_000
        s.aggregate_written_premium = 4_999_999;
        let risk = make_risk(1_000_000);
        let (_, event) =
            s.on_quote_requested(crate::types::Day(0), SubmissionId(1), &risk, true, None, 0.65, 1);
        assert!(
            matches!(event, Event::QuoteDeclined { .. }),
            "expected QuoteDeclined when at capacity, got {event:?}"
        );
    }

    #[test]
    fn solvency_breach_returns_true() {
        let mut s = Syndicate::new(SyndicateId(1), 10_000_000, 500);
        // floor = 10_000_000 * 0.20 = 2_000_000
        // capital after claim = 10_000_000 - 8_500_000 = 1_500_000 < 2_000_000
        let breached = s.on_claim_settled(8_500_000);
        assert!(breached, "should return true when capital drops below solvency floor");
        assert_eq!(s.capital, 1_500_000);
    }

    #[test]
    fn above_floor_returns_false() {
        let mut s = Syndicate::new(SyndicateId(1), 10_000_000, 500);
        // floor = 2_000_000; claim of 1_000_000 → capital = 9_000_000 > 2_000_000
        let breached = s.on_claim_settled(1_000_000);
        assert!(!breached, "should return false when capital remains above solvency floor");
    }

    #[test]
    fn policy_bound_increments_exposure() {
        let mut s = Syndicate::new(SyndicateId(1), 10_000_000, 500);
        s.on_policy_bound_as_panelist(SubmissionId(1), 100);
        assert_eq!(s.aggregate_written_premium, 100);
    }

    #[test]
    fn year_end_resets_exposure() {
        use rand::SeedableRng;
        let mut s = Syndicate::new(SyndicateId(1), 10_000_000, 500);
        s.on_policy_bound_as_panelist(SubmissionId(1), 500_000);
        assert_eq!(s.aggregate_written_premium, 500_000);
        let mut rng = rand_chacha::ChaCha20Rng::seed_from_u64(0);
        s.on_year_end(crate::types::Year(1), &std::collections::HashMap::new(), &mut rng);
        assert_eq!(s.aggregate_written_premium, 0);
    }

    // 16. ATP is monotone non-decreasing after observing higher loss ratios (proptest).
    proptest! {
        #[test]
        fn atp_increases_with_loss_ratio(
            lr_lo in 0.0_f64..1.0_f64,
            lr_hi in 0.0_f64..1.0_f64,
        ) {
            let (lr_lo, lr_hi) = if lr_lo <= lr_hi { (lr_lo, lr_hi) } else { (lr_hi, lr_lo) };
            let mut s_lo = fresh_syndicate();
            let mut s_hi = fresh_syndicate();
            for _ in 0..30 {
                s_lo.observe_line_loss_ratio("property", lr_lo);
                s_hi.observe_line_loss_ratio("property", lr_hi);
            }
            let risk = make_risk(1_000_000);
            let atp_lo = s_lo.atp(&risk, 0.60);
            let atp_hi = s_hi.atp(&risk, 0.60);
            prop_assert!(atp_hi >= atp_lo,
                "lr_hi={lr_hi:.4} atp={atp_hi} should be >= lr_lo={lr_lo:.4} atp={atp_lo}");
        }
    }
}
