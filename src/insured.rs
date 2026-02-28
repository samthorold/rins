use crate::config::ASSET_VALUE;
use crate::events::{Event, Peril, Risk};
use crate::types::{Day, InsuredId, InsurerId, SubmissionId};

/// Uplift added to acceptance threshold per unit of damage fraction suffered.
const UPLIFT_FACTOR: f64 = 0.5;
/// Multiplier applied to `rol_uplift` at each YearEnd (~1.5yr half-life).
const UPLIFT_DECAY: f64 = 0.65;
/// Maximum additional acceptance headroom above `base_max_rate_on_line`.
const MAX_UPLIFT: f64 = 0.50;

pub struct Insured {
    pub id: InsuredId,
    /// The asset this insured holds and seeks coverage for.
    pub risk: Risk,
    /// Baseline reservation price (set at construction, never mutated).
    base_max_rate_on_line: f64,
    /// Additional acceptance headroom accumulated from recent losses; decays each year.
    rol_uplift: f64,
}

impl Insured {
    pub fn new(id: InsuredId, territory: String, perils_covered: Vec<Peril>, max_rate_on_line: f64) -> Self {
        Self {
            id,
            risk: Risk { sum_insured: ASSET_VALUE, territory, perils_covered },
            base_max_rate_on_line: max_rate_on_line,
            rol_uplift: 0.0,
        }
    }

    pub fn sum_insured(&self) -> u64 {
        self.risk.sum_insured
    }

    /// Effective acceptance threshold: base + accumulated uplift from recent losses.
    pub fn effective_max_rol(&self) -> f64 {
        self.base_max_rate_on_line + self.rol_uplift
    }

    /// Called when an `AssetDamage` event hits this insured.
    /// Increases acceptance threshold proportionally to severity; capped at `MAX_UPLIFT`.
    pub fn on_asset_damage(&mut self, damage_fraction: f64) {
        self.rol_uplift = (self.rol_uplift + UPLIFT_FACTOR * damage_fraction).min(MAX_UPLIFT);
    }

    /// Called at each `YearEnd`. Decays the uplift so memories fade over ~1.5 years.
    pub fn on_year_end(&mut self) {
        self.rol_uplift *= UPLIFT_DECAY;
    }

    /// The insured decides whether to accept the quote based on its reservation price.
    /// Emits `QuoteRejected` if `premium / sum_insured > effective_max_rol()`; `QuoteAccepted` otherwise.
    pub fn on_quote_presented(
        &self,
        day: Day,
        submission_id: SubmissionId,
        insurer_id: InsurerId,
        premium: u64,
    ) -> Vec<(Day, Event)> {
        let rate = premium as f64 / self.risk.sum_insured as f64;
        if rate > self.effective_max_rol() {
            vec![(day, Event::QuoteRejected { submission_id, insured_id: self.id })]
        } else {
            vec![(
                day,
                Event::QuoteAccepted {
                    submission_id,
                    insured_id: self.id,
                    insurer_id,
                    premium,
                },
            )]
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_insured(id: u64) -> Insured {
        Insured::new(
            InsuredId(id),
            "US-SE".to_string(),
            vec![Peril::WindstormAtlantic, Peril::Attritional],
            1.0, // accepts all quotes
        )
    }

    // ── post-loss demand uplift ───────────────────────────────────────────────

    #[test]
    fn on_asset_damage_raises_effective_max_rol() {
        let mut insured = Insured::new(
            InsuredId(1), "US-SE".to_string(),
            vec![Peril::WindstormAtlantic], 0.10,
        );
        insured.on_asset_damage(0.20);
        // uplift = 0.5 × 0.20 = 0.10; effective = 0.10 + 0.10 = 0.20
        assert!((insured.effective_max_rol() - 0.20).abs() < 1e-9);
    }

    #[test]
    fn zero_damage_does_not_change_uplift() {
        let mut insured = Insured::new(
            InsuredId(2), "US-SE".to_string(),
            vec![Peril::WindstormAtlantic], 0.10,
        );
        insured.on_asset_damage(0.0);
        assert!((insured.effective_max_rol() - 0.10).abs() < 1e-9);
    }

    #[test]
    fn on_year_end_decays_rol_uplift() {
        let mut insured = Insured::new(
            InsuredId(3), "US-SE".to_string(),
            vec![Peril::WindstormAtlantic], 0.10,
        );
        insured.on_asset_damage(0.40); // uplift = 0.20
        insured.on_year_end();
        // after decay: 0.20 × 0.65 = 0.13; effective = 0.10 + 0.13 = 0.23
        let expected = 0.10 + 0.20 * UPLIFT_DECAY;
        assert!((insured.effective_max_rol() - expected).abs() < 1e-9);
    }

    #[test]
    fn uplift_decays_toward_zero_over_years() {
        let mut insured = Insured::new(
            InsuredId(4), "US-SE".to_string(),
            vec![Peril::WindstormAtlantic], 0.10,
        );
        insured.on_asset_damage(1.0); // uplift = 0.50 (capped)
        for _ in 0..10 { insured.on_year_end(); }
        // 0.50 × 0.65^10 ≈ 0.013 — well below 0.01 is borderline; check < 0.02
        assert!(insured.effective_max_rol() - 0.10 < 0.02, "uplift should be near zero after 10 years");
    }

    #[test]
    fn uplift_capped_at_max_uplift() {
        let mut insured = Insured::new(
            InsuredId(5), "US-SE".to_string(),
            vec![Peril::WindstormAtlantic], 0.10,
        );
        for _ in 0..5 { insured.on_asset_damage(0.50); } // uncapped sum = 1.25
        assert!((insured.effective_max_rol() - (0.10 + MAX_UPLIFT)).abs() < 1e-9);
    }

    #[test]
    fn quote_accepted_above_base_after_large_loss() {
        // Base=0.10, quote at 18% RoL, damage fraction=0.50 → uplift=0.25, effective=0.35 → accept
        let mut insured = Insured::new(
            InsuredId(6), "US-SE".to_string(),
            vec![Peril::WindstormAtlantic, Peril::Attritional], 0.10,
        );
        insured.on_asset_damage(0.50); // uplift = 0.25
        let premium = (ASSET_VALUE as f64 * 0.18) as u64;
        let events = insured.on_quote_presented(Day(1), SubmissionId(1), InsurerId(1), premium);
        assert!(matches!(events[0].1, Event::QuoteAccepted { .. }),
            "quote at 18% RoL should be accepted after uplift to 35%, got {:?}", events[0].1);
    }

    #[test]
    fn quote_rejected_above_effective_after_small_loss() {
        // Base=0.10, uplift=0.02 (damage=0.04), effective=0.12; quote at 13% → reject
        let mut insured = Insured::new(
            InsuredId(7), "US-SE".to_string(),
            vec![Peril::WindstormAtlantic, Peril::Attritional], 0.10,
        );
        insured.on_asset_damage(0.04); // uplift = 0.5 × 0.04 = 0.02
        let premium = (ASSET_VALUE as f64 * 0.13) as u64;
        let events = insured.on_quote_presented(Day(1), SubmissionId(2), InsurerId(1), premium);
        assert!(matches!(events[0].1, Event::QuoteRejected { .. }),
            "quote at 13% should be rejected when effective threshold is 12%");
    }

    #[test]
    fn uplift_accumulates_across_multiple_losses() {
        let mut insured = Insured::new(
            InsuredId(8), "US-SE".to_string(),
            vec![Peril::WindstormAtlantic], 0.10,
        );
        insured.on_asset_damage(0.10); // uplift = 0.05
        insured.on_asset_damage(0.10); // uplift = 0.10
        assert!((insured.effective_max_rol() - 0.20).abs() < 1e-9);
    }

    #[test]
    fn asset_sum_insured() {
        let insured = Insured::new(InsuredId(1), "US-SE".to_string(), vec![Peril::WindstormAtlantic], 1.0);
        assert_eq!(insured.sum_insured(), ASSET_VALUE);
    }

    // ── on_quote_presented ────────────────────────────────────────────────────

    #[test]
    fn on_quote_presented_accepts_below_threshold() {
        // max_rate_on_line=0.10; premium at 8% RoL → accepts.
        let insured = Insured::new(
            InsuredId(1), "US-SE".to_string(),
            vec![Peril::WindstormAtlantic, Peril::Attritional], 0.10,
        );
        let premium = (ASSET_VALUE as f64 * 0.08) as u64; // 8% RoL < 10%
        let events = insured.on_quote_presented(Day(3), SubmissionId(1), InsurerId(1), premium);
        assert_eq!(events.len(), 1);
        assert!(
            matches!(events[0].1, Event::QuoteAccepted { .. }),
            "quote at 8% RoL must be accepted when threshold is 10%, got {:?}", events[0].1
        );
    }

    #[test]
    fn on_quote_presented_accepts_at_threshold() {
        // max_rate_on_line=0.10; premium exactly at 10% RoL → accepts (≤ threshold).
        let insured = Insured::new(
            InsuredId(1), "US-SE".to_string(),
            vec![Peril::WindstormAtlantic, Peril::Attritional], 0.10,
        );
        let premium = (ASSET_VALUE as f64 * 0.10) as u64;
        let events = insured.on_quote_presented(Day(3), SubmissionId(1), InsurerId(1), premium);
        assert!(matches!(events[0].1, Event::QuoteAccepted { .. }), "at-threshold quote must be accepted");
    }

    #[test]
    fn on_quote_presented_rejects_above_threshold() {
        // max_rate_on_line=0.05; premium at 6% RoL → rejects.
        let insured = Insured::new(
            InsuredId(1), "US-SE".to_string(),
            vec![Peril::WindstormAtlantic, Peril::Attritional], 0.05,
        );
        let premium = (ASSET_VALUE as f64 * 0.06) as u64; // 6% RoL > 5%
        let events = insured.on_quote_presented(Day(3), SubmissionId(10), InsurerId(2), premium);
        assert_eq!(events.len(), 1);
        assert!(
            matches!(events[0].1, Event::QuoteRejected { .. }),
            "quote at 6% RoL must be rejected when threshold is 5%, got {:?}", events[0].1
        );
    }

    #[test]
    fn on_quote_rejected_carries_correct_ids() {
        let insured = Insured::new(
            InsuredId(42), "US-SE".to_string(),
            vec![Peril::WindstormAtlantic, Peril::Attritional], 0.01,
        );
        let premium = ASSET_VALUE; // 100% RoL — always rejected
        let events = insured.on_quote_presented(Day(5), SubmissionId(99), InsurerId(3), premium);
        if let Event::QuoteRejected { submission_id, insured_id } = events[0].1 {
            assert_eq!(submission_id, SubmissionId(99));
            assert_eq!(insured_id, InsuredId(42));
        } else {
            panic!("expected QuoteRejected");
        }
    }

    #[test]
    fn on_quote_presented_accepted_same_day() {
        let insured = make_insured(1);
        let day = Day(7);
        let events = insured.on_quote_presented(day, SubmissionId(1), InsurerId(1), 1_000);
        assert_eq!(events[0].0, day, "QuoteAccepted must fire on the same day as QuotePresented");
    }

    #[test]
    fn on_quote_presented_carries_correct_fields() {
        let insured = make_insured(42);
        let events =
            insured.on_quote_presented(Day(5), SubmissionId(99), InsurerId(3), 75_000);
        if let Event::QuoteAccepted { submission_id, insured_id, insurer_id, premium } =
            events[0].1
        {
            assert_eq!(submission_id, SubmissionId(99));
            assert_eq!(insured_id, InsuredId(42));
            assert_eq!(insurer_id, InsurerId(3));
            assert_eq!(premium, 75_000);
        } else {
            panic!("expected QuoteAccepted");
        }
    }
}
