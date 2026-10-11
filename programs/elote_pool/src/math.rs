//! Share accounting and installment arithmetic. Products go through `u128`; every division
//! rounds down, which favours the pool on both deposit and redemption.

use crate::state::{BPS, SECONDS_PER_YEAR};

/// Net asset value: vault balance plus principal lent out and not yet repaid or written off.
pub fn nav(vault_balance: u64, outstanding_principal: u64) -> Option<u64> {
    vault_balance.checked_add(outstanding_principal)
}

/// Shares created for `assets` at the current `supply` and `nav`; one-to-one while `supply == 0`.
pub fn shares_for_deposit(assets: u64, supply: u64, nav: u64) -> Option<u64> {
    if supply == 0 {
        return Some(assets);
    }
    (assets as u128)
        .checked_mul(supply as u128)?
        .checked_div(nav as u128)
        .and_then(|v| u64::try_from(v).ok())
}

/// Assets paid out for burning `shares` at the current `supply` and `nav`.
pub fn assets_for_redeem(shares: u64, supply: u64, nav: u64) -> Option<u64> {
    (shares as u128)
        .checked_mul(nav as u128)?
        .checked_div(supply as u128)
        .and_then(|v| u64::try_from(v).ok())
}

/// Fixed interest on `principal` at `rate_bps`.
pub fn interest_due(principal: u64, rate_bps: u16) -> Option<u64> {
    (principal as u128)
        .checked_mul(rate_bps as u128)?
        .checked_div(BPS as u128)
        .and_then(|v| u64::try_from(v).ok())
}

// -----------------------------------------------------------------------------------------------
// Installments and arrears
// -----------------------------------------------------------------------------------------------

/// Installments whose due date has arrived: installment `k` (1-based) is due at
/// `drawn_at + k * interval`, and at that very second it already counts as due. Zero before the
/// first due date, never more than `installments`.
pub fn installments_due(now: i64, drawn_at: i64, interval: i64, installments: u16) -> u16 {
    if interval <= 0 || now <= drawn_at {
        return 0;
    }
    let Some(elapsed) = now.checked_sub(drawn_at) else { return 0 };
    let periods = elapsed / interval;
    if periods >= i64::from(installments) {
        installments
    } else {
        periods as u16 // 0 <= periods < installments <= u16::MAX
    }
}

/// The regular installment, `total / installments` rounded down; the last one also carries the
/// remainder, so the installments add up to `total` exactly.
pub fn installment_amount(total: u64, installments: u16) -> Option<u64> {
    total.checked_div(u64::from(installments))
}

/// What is due once installments `1..=k` have fallen due: `k * installment`, and `total` from
/// the last installment on.
pub fn cumulative_due(total: u64, installments: u16, k: u16) -> Option<u64> {
    if k >= installments {
        return Some(total);
    }
    installment_amount(total, installments)?.checked_mul(u64::from(k))
}

/// Installments fully paid by `paid` (principal and ordinary interest; late interest never pays
/// an installment).
pub fn installments_covered(paid: u64, total: u64, installments: u16) -> Option<u16> {
    if paid >= total {
        return Some(installments);
    }
    let base = installment_amount(total, installments)?;
    let full = paid.checked_div(base)?;
    let below_last = u64::from(installments).checked_sub(1)?;
    Some(full.min(below_last) as u16) // <= installments - 1
}

/// Late interest on `amount` overdue for `seconds`, at `late_bps` a year:
///
/// `min(amount, amount * late_bps * min(seconds, cap) / (10_000 * SECONDS_PER_YEAR))`, rounded
/// down, with `cap = ceil(10_000 * SECONDS_PER_YEAR / late_bps)`, the time after which it would
/// exceed `amount`.
/// The result is never more than `amount`.
pub fn late_interest(amount: u64, late_bps: u16, seconds: u64) -> Option<u64> {
    let weighted = (amount as u128).checked_mul(capped_seconds(late_bps, seconds) as u128)?;
    late_from_weighted(weighted, late_bps, amount)
}

/// `seconds` clamped to the time after which late interest would exceed the amount it accrues
/// on; this also bounds every product below `u128::MAX`.
fn capped_seconds(late_bps: u16, seconds: u64) -> u64 {
    if late_bps == 0 {
        return 0;
    }
    // Rounded up, so a full run reaches the amount; constants, late_bps > 0.
    let cap = (BPS * SECONDS_PER_YEAR).div_ceil(u64::from(late_bps));
    seconds.min(cap)
}

/// `weighted * late_bps / (10_000 * SECONDS_PER_YEAR)`, capped at `cap`.
fn late_from_weighted(weighted: u128, late_bps: u16, cap: u64) -> Option<u64> {
    let late = weighted
        .checked_mul(late_bps as u128)?
        .checked_div((BPS as u128).checked_mul(SECONDS_PER_YEAR as u128)?)?;
    u64::try_from(late.min(cap as u128)).ok()
}

/// The state of a scheduled line at `now`, computed from its terms and its repayments; nothing
/// of it is stored.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Arrears {
    /// Installments fallen due.
    pub due: u16,
    /// Installments fully paid.
    pub covered: u16,
    /// `due - covered` when positive.
    pub missed: u16,
    /// Principal and interest fallen due and not paid.
    pub overdue: u64,
    /// Late interest owed now and not yet settled.
    pub late_due: u64,
    /// Due date of the oldest unpaid installment that has fallen due; `None` when nothing is missed.
    pub first_missed_due: Option<i64>,
}

/// Terms of a scheduled line, as the arrears computation needs them.
#[derive(Clone, Copy, Debug)]
pub struct ScheduleTerms {
    /// `principal + interest_due`.
    pub total: u64,
    pub drawn_at: i64,
    pub interval: i64,
    pub installments: u16,
    pub late_bps: u16,
    /// The pool's grace period.
    pub grace: u32,
    /// Late interest is settled up to this time (zero: never).
    pub late_paid_until: i64,
}

/// Arrears of a scheduled line that has repaid `paid` (principal plus interest) by `now`.
///
/// Late interest accrues on each overdue part separately: the unpaid part of installment `k`
/// accrues from `start_k = max(due_k + grace, late_paid_until)` until `now`, at `late_bps` a year,
/// and never beyond its own amount. The sum is divided once:
///
/// `late_due = Σ part_k * min(now - start_k, cap) * late_bps / (10_000 * SECONDS_PER_YEAR)`,
/// capped at `overdue`.
pub fn arrears(t: &ScheduleTerms, paid: u64, now: i64) -> Option<Arrears> {
    let due = installments_due(now, t.drawn_at, t.interval, t.installments);
    let covered = installments_covered(paid, t.total, t.installments)?;
    let missed = due.saturating_sub(covered); // a count, zero when paid ahead
    let mut overdue: u64 = 0;
    let mut weighted: u128 = 0;
    let mut first_missed_due = None;
    let mut k = covered;
    while k < due {
        k += 1; // k <= due <= installments
        let upto = cumulative_due(t.total, t.installments, k)?;
        let before = cumulative_due(t.total, t.installments, k - 1)?;
        let part = upto.checked_sub(paid.max(before))?;
        let due_at = t.drawn_at.checked_add(t.interval.checked_mul(i64::from(k))?)?;
        if first_missed_due.is_none() {
            first_missed_due = Some(due_at);
        }
        let start = due_at.checked_add(i64::from(t.grace))?.max(t.late_paid_until);
        if now > start {
            let seconds = now.checked_sub(start)? as u64; // positive
            let s = capped_seconds(t.late_bps, seconds);
            weighted = weighted.checked_add((part as u128).checked_mul(s as u128)?)?;
        }
        overdue = overdue.checked_add(part)?;
    }
    let late_due = late_from_weighted(weighted, t.late_bps, overdue)?;
    Some(Arrears { due, covered, missed, overdue, late_due, first_missed_due })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rounding_favours_the_pool() {
        assert_eq!(shares_for_deposit(1, 3, 2), Some(1));
        assert_eq!(assets_for_redeem(1, 3, 2), Some(0));
        assert_eq!(shares_for_deposit(1_000_000, 10_000_000, 10_400_000), Some(961_538));
    }

    #[test]
    fn first_deposit_is_one_to_one() {
        assert_eq!(shares_for_deposit(1_234_567, 0, 0), Some(1_234_567));
        assert_eq!(shares_for_deposit(1_234_567, 0, 99), Some(1_234_567));
    }

    #[test]
    fn zero_nav_or_supply_is_refused() {
        assert_eq!(shares_for_deposit(1, 1, 0), None);
        assert_eq!(assets_for_redeem(1, 0, 1), None);
    }

    #[test]
    fn large_values_do_not_overflow_the_product() {
        assert_eq!(shares_for_deposit(u64::MAX, u64::MAX, u64::MAX), Some(u64::MAX));
        assert_eq!(assets_for_redeem(u64::MAX, u64::MAX, u64::MAX), Some(u64::MAX));
        assert_eq!(shares_for_deposit(u64::MAX, u64::MAX, 1), None, "result above u64");
        assert_eq!(nav(u64::MAX, 1), None);
    }

    #[test]
    fn interest_is_rounded_down() {
        assert_eq!(interest_due(4_000_000, 1_000), Some(400_000));
        assert_eq!(interest_due(6_000_000_000, 1_200), Some(720_000_000));
        assert_eq!(interest_due(9_999, 1), Some(0));
        assert_eq!(interest_due(u64::MAX, 10_000), Some(u64::MAX));
    }

    // -- installments: ported from the old `missed_payments` cases ------------------------------

    #[test]
    fn none_due_before_the_first_interval() {
        assert_eq!(installments_due(100, 0, 1_000, 6), 0);
        assert_eq!(installments_due(0, 0, 1_000, 6), 0);
        assert_eq!(installments_due(-5, 0, 1_000, 6), 0);
    }

    // No "-1": at exactly one interval after the draw the first installment is due.
    #[test]
    fn one_due_at_the_first_due_date() {
        assert_eq!(installments_due(1_000, 0, 1_000, 6), 1);
    }

    #[test]
    fn one_due_after_a_partial_second_period() {
        assert_eq!(installments_due(1_500, 0, 1_000, 6), 1);
    }

    #[test]
    fn three_due_after_three_periods_and_never_more_than_installments() {
        assert_eq!(installments_due(3_500, 0, 1_000, 6), 3);
        assert_eq!(installments_due(1_000_000, 0, 1_000, 6), 6);
        assert_eq!(installments_due(i64::MAX, i64::MIN, 1, 6), 0, "elapsed overflow counts nothing");
        assert_eq!(installments_due(10, 0, 0, 6), 0, "zero interval");
    }

    #[test]
    fn installments_add_up_exactly() {
        for (total, n) in [(1_000_003u64, 6u16), (6_720_000, 6), (7, 7), (u64::MAX, 120), (119, 120 - 1)] {
            let base = installment_amount(total, n).unwrap();
            let last = total - cumulative_due(total, n, n - 1).unwrap();
            assert_eq!(base * u64::from(n - 1) + last, total);
            assert!(last >= base && last - base < u64::from(n));
            assert_eq!(cumulative_due(total, n, n).unwrap(), total);
        }
        assert_eq!(installment_amount(1_000_003, 6), Some(166_667));
        assert_eq!(cumulative_due(1_000_003, 6, 5), Some(833_335));
        assert_eq!(installment_amount(1, 0), None);
    }

    #[test]
    fn covered_counts_whole_installments() {
        assert_eq!(installments_covered(0, 600, 6), Some(0));
        assert_eq!(installments_covered(99, 600, 6), Some(0));
        assert_eq!(installments_covered(100, 600, 6), Some(1));
        assert_eq!(installments_covered(599, 600, 6), Some(5));
        assert_eq!(installments_covered(600, 600, 6), Some(6));
        // 1_000_003 in six: 166_667 x 5 + 166_668. Paying 6 x 166_667 is not the last one.
        assert_eq!(installments_covered(1_000_002, 1_000_003, 6), Some(5));
        assert_eq!(installments_covered(1_000_003, 1_000_003, 6), Some(6));
    }

    // -- late interest: ported from the old late-payment cases, annualized ---------------------

    const YEAR: u64 = SECONDS_PER_YEAR;

    #[test]
    fn late_zero_without_time_or_rate() {
        assert_eq!(late_interest(1_000, 500, 0), Some(0));
        assert_eq!(late_interest(1_000, 0, YEAR), Some(0));
    }

    #[test]
    fn late_full_year_at_full_rate_equals_the_amount() {
        assert_eq!(late_interest(1_000, 10_000, YEAR), Some(1_000));
    }

    #[test]
    fn late_half_year_and_low_rate() {
        assert_eq!(late_interest(1_000, 10_000, YEAR / 2), Some(500));
        assert_eq!(late_interest(10_000, 500, 2 * YEAR), Some(1_000));
    }

    #[test]
    fn late_one_second_exact_accrual() {
        // amount == SECONDS_PER_YEAR at 100% a year accrues exactly one unit a second.
        assert_eq!(late_interest(YEAR, 10_000, 1), Some(1));
    }

    #[test]
    fn late_is_capped_at_the_amount() {
        assert_eq!(late_interest(1_000, 10_000, 20 * YEAR), Some(1_000));
        assert_eq!(late_interest(1_000, 1, 100_000 * YEAR), Some(1_000));
        assert_eq!(late_interest(u64::MAX, u16::MAX, u64::MAX), Some(u64::MAX), "no overflow at the extremes");
    }

    #[test]
    fn late_large_in_range_value_is_exact() {
        let large = u64::MAX / 1_000_000;
        assert_eq!(late_interest(large, 10_000, YEAR), Some(large));
    }

    // -- arrears -------------------------------------------------------------------------------

    fn terms(total: u64, n: u16, late_bps: u16, grace: u32) -> ScheduleTerms {
        ScheduleTerms { total, drawn_at: 1_000, interval: 100, installments: n, late_bps, grace, late_paid_until: 0 }
    }

    #[test]
    fn arrears_count_missed_over_time() {
        let t = terms(600, 6, 0, 0);
        let at = |paid, now| arrears(&t, paid, now).unwrap();
        assert_eq!(at(0, 1_099).missed, 0);
        let a = at(0, 1_100);
        assert_eq!((a.due, a.covered, a.missed, a.overdue, a.first_missed_due), (1, 0, 1, 100, Some(1_100)));
        let a = at(150, 1_300);
        assert_eq!((a.due, a.covered, a.missed, a.overdue, a.first_missed_due), (3, 1, 2, 150, Some(1_200)));
        let a = at(400, 1_300);
        assert_eq!((a.missed, a.overdue, a.first_missed_due), (0, 0, None), "paid ahead");
        let a = at(0, 9_999);
        assert_eq!((a.due, a.missed, a.overdue), (6, 6, 600));
    }

    #[test]
    fn arrears_late_interest_per_part_from_due_plus_grace() {
        // Two installments of 365 overdue at 100% a year, grace 10: one has run 100 s past grace,
        // the other 0 s.
        let mut t = terms(365 * 3 * YEAR / 1_000, 3, 10_000, 10);
        t.interval = 1_000;
        let part = t.total / 3; // 365 * YEAR / 1_000
        let now = t.drawn_at + 2_000 + 10 + 100; // k=1 due at +1000, k=2 at +2000
        let a = arrears(&t, 0, now).unwrap();
        assert_eq!(a.missed, 2);
        // One division over the weighted sum: part * (1_100 + 100) / YEAR.
        assert_eq!(a.late_due, (part as u128 * 1_200 / YEAR as u128) as u64);
        assert_eq!(a.late_due, 438);
        // Within grace nothing accrues.
        let a = arrears(&t, 0, t.drawn_at + 1_000 + 10).unwrap();
        assert_eq!((a.missed, a.late_due), (1, 0));
        // The watermark stops a second charge for the same time.
        t.late_paid_until = now;
        assert_eq!(arrears(&t, 0, now).unwrap().late_due, 0);
        let a = arrears(&t, 0, now + 10).unwrap();
        assert_eq!(a.late_due, (2 * part as u128 * 10 / YEAR as u128) as u64);
    }

    #[test]
    fn arrears_late_interest_is_capped_at_the_overdue_amount() {
        let t = terms(600, 6, 10_000, 0);
        let a = arrears(&t, 0, 1_000 + 600 + 50 * YEAR as i64).unwrap();
        assert_eq!((a.overdue, a.late_due), (600, 600));
    }

    #[test]
    fn arrears_partial_payment_lowers_the_overdue_part() {
        // Two installments of 100 due, 60 paid: 40 of the first and all of the second are overdue.
        let t = terms(600, 6, 0, 0);
        let a = arrears(&t, 60, 1_200).unwrap();
        assert_eq!((a.due, a.covered, a.missed, a.overdue, a.first_missed_due), (2, 0, 2, 140, Some(1_100)));
    }
}
