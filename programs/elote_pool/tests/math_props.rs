//! Property tests over `math`, the pure arithmetic the instructions build on. Each property is an
//! invariant the pool's accounting relies on; `proptest` searches the input space for a
//! counter-example and shrinks it when it finds one.

use elote_pool::math::{
    arrears, assets_for_redeem, cumulative_due, installment_amount, installments_covered, installments_due,
    late_interest, shares_for_deposit, Arrears, ScheduleTerms,
};
use elote_pool::state::MAX_INSTALLMENTS;
use proptest::prelude::*;

const HALF: u64 = u64::MAX / 2;

proptest! {
    #![proptest_config(ProptestConfig::with_cases(2_000))]

    /// A deposit followed by a redemption of the shares it minted never pays out more than it
    /// put in: both legs round down in the pool's favour.
    #[test]
    fn deposit_then_redeem_never_profits(assets in 1..HALF, supply in 1..HALF, nav in 1..HALF) {
        if let Some(shares) = shares_for_deposit(assets, supply, nav) {
            // The mint itself refuses a supply above u64::MAX; nothing to check past that.
            let Some(supply_after) = supply.checked_add(shares) else { return Ok(()) };
            let out = assets_for_redeem(shares, supply_after, nav + assets).expect("in range");
            prop_assert!(out <= assets, "deposited {assets}, redeemed {out}");
        }
    }

    /// Burning at most the whole supply never pays out more than the NAV.
    #[test]
    fn redeem_never_exceeds_nav(shares in 0..HALF, supply in 1..HALF, nav in 0..HALF) {
        let shares = shares.min(supply);
        let out = assets_for_redeem(shares, supply, nav).expect("in range");
        prop_assert!(out <= nav);
    }

    /// Late interest on an amount never exceeds the amount, whatever the rate and the time.
    #[test]
    fn late_interest_is_capped_at_the_amount(amount in any::<u64>(), late_bps in 0..=10_000u16, seconds in any::<u64>()) {
        let late = late_interest(amount, late_bps, seconds).expect("never overflows");
        prop_assert!(late <= amount);
    }

    /// Installments partition the total exactly and the cumulative amount due is monotone.
    #[test]
    fn installments_partition_the_total(total in 1..=u64::MAX, n in 1..=MAX_INSTALLMENTS) {
        prop_assume!(total >= u64::from(n));
        let base = installment_amount(total, n).expect("n > 0");
        let mut prev = 0;
        for k in 0..=n {
            let c = cumulative_due(total, n, k).expect("in range");
            prop_assert!(c >= prev);
            if k < n {
                prop_assert_eq!(c, base * u64::from(k));
            }
            prev = c;
        }
        prop_assert_eq!(prev, total);
        prop_assert!(total - cumulative_due(total, n, n - 1).unwrap() >= base, "the last installment carries the remainder");
    }

    /// Installments fall due monotonically with time and never beyond the schedule.
    #[test]
    fn installments_due_is_monotone(drawn_at in 0..1i64 << 40, interval in 1..1i64 << 30, n in 1..=MAX_INSTALLMENTS, a in 0..1i64 << 41, b in 0..1i64 << 41) {
        let (lo, hi) = (a.min(b), a.max(b));
        let d_lo = installments_due(lo, drawn_at, interval, n);
        let d_hi = installments_due(hi, drawn_at, interval, n);
        prop_assert!(d_lo <= d_hi);
        prop_assert!(d_hi <= n);
    }

    /// Paying more never covers fewer installments, and a full payment covers them all.
    #[test]
    fn covered_is_monotone_in_paid(total in 1..=u64::MAX, n in 1..=MAX_INSTALLMENTS, a in any::<u64>(), b in any::<u64>()) {
        prop_assume!(total >= u64::from(n));
        let (lo, hi) = (a.min(b), a.max(b));
        let c_lo = installments_covered(lo, total, n).expect("n > 0");
        let c_hi = installments_covered(hi, total, n).expect("n > 0");
        prop_assert!(c_lo <= c_hi);
        prop_assert!(c_hi <= n);
        prop_assert_eq!(installments_covered(total, total, n), Some(n));
    }

    /// The arrears of any valid line are consistent: counts bounded by the schedule, the overdue
    /// part bounded by what is unpaid, late interest bounded by the overdue part and by what is
    /// left of the lifetime cap, and a first missed due date exactly when something is missed.
    #[test]
    fn arrears_are_consistent(
        total in 1..=1u64 << 50,
        n in 1..=MAX_INSTALLMENTS,
        interest_share in 0..=10_000u64,
        drawn_at in 1_000_000_000i64..2_000_000_000,
        interval in 1..=10_000_000i64,
        late_bps in 0..=10_000u16,
        grace in 0..=2_000_000u32,
        paid_num in 0..=12_000u64,
        collected_share in 0..=10_000u64,
        watermark in 0..=10_000i64,
        now_num in -100..=130i64,
    ) {
        prop_assume!(total >= u64::from(n));
        let interest = total / 10_000 * interest_share;
        let principal = total - interest;
        let late_collected = principal / 10_000 * collected_share;
        let paid = (total / 10_000).max(1) * paid_num; // up to 120% of the total
        let now = drawn_at + interval * now_num; // from before the draw to past the last due date
        let late_paid_until = if watermark == 0 { 0 } else { drawn_at + (now - drawn_at) * watermark / 10_000 };
        let t = ScheduleTerms {
            total, drawn_at, interval, installments: n, late_bps, grace, late_paid_until, principal, late_collected,
        };
        let Arrears { due, covered, missed, overdue, late_due, first_missed_due } =
            arrears(&t, paid, now).expect("valid terms never overflow");
        prop_assert!(due <= n);
        prop_assert!(covered <= n);
        prop_assert_eq!(missed, due.saturating_sub(covered));
        prop_assert!(overdue <= total.saturating_sub(paid));
        prop_assert!(late_due <= overdue);
        prop_assert!(late_due <= principal - late_collected);
        prop_assert_eq!(first_missed_due.is_some(), missed > 0);
        prop_assert_eq!(overdue > 0, missed > 0);
        if late_bps == 0 {
            prop_assert_eq!(late_due, 0);
        }
        if let Some(first) = first_missed_due {
            prop_assert!(first > drawn_at && first <= now, "a missed due date lies between the draw and now");
        }
    }
}
