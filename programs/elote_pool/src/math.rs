//! Share accounting and installment arithmetic. Products go through `u128`. Share conversions
//! round down, in the pool's favour on both deposit and redemption; interest rounds down, in the
//! borrower's favour.

use crate::state::{BPS, SECONDS_PER_YEAR};

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

/// Ordinary interest on a tranche of `amount` outstanding for `seconds` at an annual `rate_bps`:
/// `amount * rate_bps * seconds / (10_000 * SECONDS_PER_YEAR)`, rounded down once per tranche.
pub fn tranche_interest(amount: u64, rate_bps: u16, seconds: u64) -> Option<u64> {
    (amount as u128)
        .checked_mul(rate_bps as u128)?
        .checked_mul(seconds as u128)?
        .checked_div((BPS as u128).checked_mul(SECONDS_PER_YEAR as u128)?)
        .and_then(|v| u64::try_from(v).ok())
}

/// The ordinary interest a bullet line has accrued by `mark`: every tranche from its draw to the
/// mark, `rate_bps * (drawn * mark - draw_time_weight) / (10_000 * SECONDS_PER_YEAR)` rounded down
/// once, where `draw_time_weight = Σ amount_k * drawn_at_k`. `None` on overflow or a mark before the
/// draw.
///
/// Rounded once over the sum, it can exceed the line's `interest_due`, which `draw` rounds down per
/// tranche (`tranche_interest` to the last due date): with the mark at the last due date it is
/// `interest_due` plus up to one unit per tranche but the first. `accelerated_balance` (in
/// `instructions/lines.rs`) therefore clamps it with `.min(interest_due)`: an acceleration never
/// keeps more ordinary interest than the draws fixed.
pub fn bullet_accrued_interest(drawn: u64, draw_time_weight: u128, rate_bps: u16, mark: i64) -> Option<u64> {
    let span = (drawn as u128).checked_mul(u128::try_from(mark).ok()?)?.checked_sub(draw_time_weight)?;
    span.checked_mul(rate_bps as u128)?
        .checked_div((BPS as u128).checked_mul(SECONDS_PER_YEAR as u128)?)
        .and_then(|v| u64::try_from(v).ok())
}

// -----------------------------------------------------------------------------------------------
// Installments and arrears
// -----------------------------------------------------------------------------------------------

/// Installments whose due date has arrived: installment `k` (1-based) is due at
/// `anchor + k * interval`, and at that very second it already counts as due. Zero before the
/// first due date, never more than `installments`. The anchor is one interval before the first
/// due date.
pub fn installments_due(now: i64, anchor: i64, interval: i64, installments: u16) -> u16 {
    if interval <= 0 || now <= anchor {
        return 0;
    }
    let Some(elapsed) = now.checked_sub(anchor) else { return 0 };
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

/// The seconds after which late interest at `late_bps` a year reaches the amount itself: the
/// per-part cap on the accrual period, which also bounds every product below `u128::MAX`. Zero
/// when there is no late interest.
fn seconds_cap(late_bps: u16) -> u64 {
    if late_bps == 0 {
        return 0;
    }
    // Rounded up, so a full run reaches the amount; constants, late_bps > 0.
    (BPS * SECONDS_PER_YEAR).div_ceil(u64::from(late_bps))
}

fn capped_seconds(late_bps: u16, seconds: u64) -> u64 {
    seconds.min(seconds_cap(late_bps))
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
    /// The principal share of `overdue`: what late interest accrues on.
    pub overdue_principal: u64,
    /// Late interest owed now and not yet settled.
    pub late_due: u64,
    /// When the oldest unpaid installment became exigible (its due date, or the acceleration if
    /// that came first); `None` when nothing is missed.
    pub first_missed_due: Option<i64>,
}

/// Terms of a scheduled line, as the arrears computation needs them.
#[derive(Clone, Copy, Debug)]
pub struct ScheduleTerms {
    /// `drawn + interest_due`: what the installments add up to.
    pub total: u64,
    /// One interval before the first due date: installment `k` is due at `anchor + k * interval`.
    pub anchor: i64,
    pub interval: i64,
    pub installments: u16,
    pub late_bps: u16,
    /// Seconds after a due date before late interest accrues. The program passes zero (there is
    /// no grace period); the arithmetic keeps the term so the pinned vectors stay valid.
    pub grace: u32,
    /// Late interest is settled up to this time (zero: never).
    pub late_paid_until: i64,
    /// The principal the installments repay (the line's `drawn`): the share of each installment
    /// that late interest accrues on, and the lifetime cap on late interest.
    pub principal: u64,
    /// The principal still unpaid (`drawn - principal_repaid`): an overdue part carries no more
    /// principal than its share of what is left (decision 46).
    pub principal_left: u64,
    /// Late interest collected so far.
    pub late_collected: u64,
    /// When the balance was accelerated (`mark_in_collection`); zero when it was not.
    pub accelerated_at: i64,
}

/// The ordinary interest kept and forgiven at the acceleration: the line owes the interest of the
/// installments due at the mark, `interest_due * due_count / installments` rounded down, and the
/// rest (with the rounding remainder) is forgiven. `(kept, forgiven)`; `None` on overflow or when
/// `due_count > installments`.
pub fn accelerated_interest(interest_due: u64, due_count: u16, installments: u16) -> Option<(u64, u64)> {
    if due_count > installments || installments == 0 {
        return None;
    }
    let kept = (interest_due as u128)
        .checked_mul(u128::from(due_count))?
        .checked_div(u128::from(installments))
        .and_then(|v| u64::try_from(v).ok())?;
    Some((kept, interest_due.checked_sub(kept)?))
}

/// The principal share of a `part` of the installments: `part * principal / total`, rounded down.
/// Every installment repays principal and interest in the proportion of the whole, so the share is
/// the same whatever the order the repayment allocation follows.
fn principal_share(part: u64, principal: u64, total: u64) -> Option<u64> {
    (part as u128)
        .checked_mul(principal as u128)?
        .checked_div(total as u128)
        .and_then(|v| u64::try_from(v).ok())
}

/// Arrears of a scheduled line that has repaid `paid` (principal plus interest) by `now`.
///
/// Late interest accrues on the principal share of each overdue part separately (no interest on
/// overdue interest): the smaller of the installment's own proportion (`principal / total`) and
/// the proportion of what is left unpaid (`principal_left / (total - paid)`), since a payment on
/// account may have repaid principal ahead of the schedule (decision 46). The unpaid part of
/// installment `k` accrues from
/// `start_k = max(due_k + grace, late_paid_until)` until `now`, at `late_bps` a year, and never
/// beyond its own amount. Once the balance is accelerated (`accelerated_at > 0`, set by
/// `mark_in_collection`) every installment counts as due, so `overdue` is the whole unpaid balance,
/// and installment `k` is exigible at `min(due_k, accelerated_at)` and accrues from
/// `max(min(due_k + grace, accelerated_at), late_paid_until)`: one already overdue keeps its own
/// start, one not yet due starts at the mark with no further grace. The sum is divided once:
///
/// `late_due = Σ principal_share(part_k) * min(now - start_k, cap) * late_bps / (10_000 * SECONDS_PER_YEAR)`,
/// capped at `overdue_principal` for this settlement and at `principal - late_collected` over the
/// life of the line. The per-part cap applies to one settlement: after a repayment the watermark
/// moves and the same unpaid part can accrue again, which is what the lifetime cap bounds.
pub fn arrears(t: &ScheduleTerms, paid: u64, now: i64) -> Option<Arrears> {
    let accelerated = t.accelerated_at > 0;
    let due = if accelerated { t.installments } else { installments_due(now, t.anchor, t.interval, t.installments) };
    let covered = installments_covered(paid, t.total, t.installments)?;
    let missed = due.saturating_sub(covered); // a count, zero when paid ahead
    let mut overdue: u64 = 0;
    let mut overdue_principal: u64 = 0;
    let mut weighted: u128 = 0;
    let mut first_missed_due = None;
    // Every installment but the first overdue one (partly paid) and the last (the rounding
    // remainder) is the regular installment, so its principal share is computed once.
    let base = installment_amount(t.total, t.installments)?;
    // The proportion of principal in an overdue part: the schedule's, or that of what is left
    // unpaid when smaller (decision 46). One proportion for every part, chosen once.
    let unpaid = t.total.saturating_sub(paid);
    let (share_of, share_total) = if (t.principal_left as u128) * (t.total as u128) < (t.principal as u128) * (unpaid as u128) {
        (t.principal_left, unpaid)
    } else {
        (t.principal, t.total)
    };
    let base_share = principal_share(base, share_of, share_total)?;
    let cap = seconds_cap(t.late_bps);
    let mut k = covered;
    // What was due before installment k is what was due through k - 1: carried, not recomputed.
    let mut before = cumulative_due(t.total, t.installments, covered)?;
    while k < due {
        k += 1; // k <= due <= installments
        let upto = cumulative_due(t.total, t.installments, k)?;
        let part = upto.checked_sub(paid.max(before))?;
        before = upto;
        let part_principal = if part == base { base_share } else { principal_share(part, share_of, share_total)? };
        let due_at = t.anchor.checked_add(t.interval.checked_mul(i64::from(k))?)?;
        let grace_end = due_at.checked_add(i64::from(t.grace))?;
        let (exigible_at, accrual_from) =
            if accelerated { (due_at.min(t.accelerated_at), grace_end.min(t.accelerated_at)) } else { (due_at, grace_end) };
        if first_missed_due.is_none() {
            first_missed_due = Some(exigible_at);
        }
        let start = accrual_from.max(t.late_paid_until);
        if now > start {
            let seconds = now.checked_sub(start)? as u64; // positive
            let s = seconds.min(cap);
            weighted = weighted.checked_add((part_principal as u128).checked_mul(s as u128)?)?;
        }
        overdue = overdue.checked_add(part)?;
        overdue_principal = overdue_principal.checked_add(part_principal)?;
    }
    let lifetime_left = t.principal.checked_sub(t.late_collected)?;
    let late_due = late_from_weighted(weighted, t.late_bps, overdue_principal)?.min(lifetime_left);
    Some(Arrears { due, covered, missed, overdue, overdue_principal, late_due, first_missed_due })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bullet_interest_accrues_per_tranche_to_the_mark() {
        let year = SECONDS_PER_YEAR as i64;
        // 4.0 drawn at t = 0 and 2.0 at half a year, at 12%; marked at one year.
        let weight = 2_000_000u128 * (year as u128 / 2); // the first tranche, drawn at t = 0, adds nothing
        let kept = bullet_accrued_interest(6_000_000, weight, 1_200, year).unwrap();
        assert_eq!(kept, 480_000 + 120_000);
        assert_eq!(kept, tranche_interest(4_000_000, 1_200, year as u64).unwrap() + tranche_interest(2_000_000, 1_200, year as u64 / 2).unwrap());
        assert_eq!(bullet_accrued_interest(2_000_000, 2_000_000u128 * year as u128, 1_200, 0), None, "a mark before every draw");
        assert_eq!(bullet_accrued_interest(0, 0, 1_200, year), Some(0));
    }

    /// Three tranches of a bullet line, the mark at the last due date: accrued once over the sum,
    /// the interest is two units above the `interest_due` the three draws fixed (each rounded
    /// down), and the clamp `accelerated_balance` applies keeps exactly `interest_due`.
    #[test]
    fn bullet_interest_at_the_last_due_date_is_clamped_to_the_interest_due() {
        let year = SECONDS_PER_YEAR as i64;
        let start = 1_800_000_000i64;
        let last_due = start + year;
        let tranches = [(1_000_003u64, start + 1), (2_000_001, start + year / 3 + 1_589), (3_000_007, start + 2 * year / 3 + 398)];
        let interest_due: u64 = tranches.iter().map(|&(a, t)| tranche_interest(a, 1_200, (last_due - t) as u64).unwrap()).sum();
        let drawn: u64 = tranches.iter().map(|&(a, _)| a).sum();
        let weight: u128 = tranches.iter().map(|&(a, t)| a as u128 * t as u128).sum();
        let accrued = bullet_accrued_interest(drawn, weight, 1_200, last_due).unwrap();
        assert_eq!((interest_due, accrued), (399_982, 399_984));
        let kept = accrued.min(interest_due);
        assert_eq!(kept, interest_due);
    }

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
    }

    // -- installments ----------------------------------------------------------------------------

    #[test]
    fn none_due_before_the_first_interval() {
        assert_eq!(installments_due(100, 0, 1_000, 6), 0);
        assert_eq!(installments_due(0, 0, 1_000, 6), 0);
        assert_eq!(installments_due(-5, 0, 1_000, 6), 0);
    }

    // At its due date to the second, the first installment already counts as due.
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

    // -- late interest ---------------------------------------------------------------------------

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

    #[test]
    fn tranche_interest_is_proportional_to_time_and_rounded_down() {
        // 6.0 at 12% a year for a whole year: 0.72.
        assert_eq!(tranche_interest(6_000_000, 1_200, YEAR), Some(720_000));
        // Half the time, half the interest; a second of a 1-unit tranche rounds to nothing.
        assert_eq!(tranche_interest(6_000_000, 1_200, YEAR / 2), Some(360_000));
        assert_eq!(tranche_interest(1, 1_200, 1), Some(0));
        assert_eq!(tranche_interest(6_000_000, 1_200, 0), Some(0));
        assert_eq!(tranche_interest(6_000_000, 0, YEAR), Some(0));
        // 180 of 365 days at 12% on 6.0: 6_000_000 * 1_200 * 15_552_000 / (10_000 * 31_536_000).
        assert_eq!(tranche_interest(6_000_000, 1_200, 180 * 86_400), Some(355_068));
        assert_eq!(tranche_interest(u64::MAX, 10_000, YEAR), Some(u64::MAX), "no overflow at the extremes");
        assert_eq!(tranche_interest(u64::MAX, 10_000, 2 * YEAR), None, "but a result above u64 is refused");
    }

    fn terms(total: u64, n: u16, late_bps: u16, grace: u32) -> ScheduleTerms {
        ScheduleTerms {
            total,
            anchor: 1_000,
            interval: 100,
            installments: n,
            late_bps,
            grace,
            late_paid_until: 0,
            principal: total,
            principal_left: total,
            late_collected: 0,
            accelerated_at: 0,
        }
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
        let now = t.anchor + 2_000 + 10 + 100; // k=1 due at +1000, k=2 at +2000
        let a = arrears(&t, 0, now).unwrap();
        assert_eq!(a.missed, 2);
        // One division over the weighted sum: part * (1_100 + 100) / YEAR.
        assert_eq!(a.late_due, (part as u128 * 1_200 / YEAR as u128) as u64);
        assert_eq!(a.late_due, 438);
        // Within grace nothing accrues.
        let a = arrears(&t, 0, t.anchor + 1_000 + 10).unwrap();
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
    fn arrears_late_interest_never_exceeds_the_principal_over_the_life_of_the_line() {
        let mut t = terms(600, 6, 10_000, 0);
        t.principal = 500;
        t.late_collected = 450;
        let a = arrears(&t, 0, 1_000 + 600 + 50 * YEAR as i64).unwrap();
        assert_eq!(a.late_due, 50);
        t.late_collected = 500;
        assert_eq!(arrears(&t, 0, 1_000 + 600 + 50 * YEAR as i64).unwrap().late_due, 0);
        t.late_collected = 501;
        assert_eq!(arrears(&t, 0, 1_700).map(|a| a.late_due), None, "an impossible state is refused");
    }

    #[test]
    fn arrears_partial_payment_lowers_the_overdue_part() {
        // Two installments of 100 due, 60 paid: 40 of the first and all of the second are overdue.
        let t = terms(600, 6, 0, 0);
        let a = arrears(&t, 60, 1_200).unwrap();
        assert_eq!((a.due, a.covered, a.missed, a.overdue, a.first_missed_due), (2, 0, 2, 140, Some(1_100)));
        assert_eq!(a.overdue_principal, 140, "principal == total here, so every part is principal");
    }

    /// Late interest accrues on the principal share of each overdue part, never on
    /// the interest share. With 500 of principal in a total of 600, an overdue installment of 100
    /// carries 83 of principal (rounded down), and the late interest and the per-settlement cap
    /// both follow that figure.
    #[test]
    fn arrears_late_interest_accrues_on_the_principal_share_only() {
        let mut t = terms(600, 6, 10_000, 0);
        t.principal = 500;
        t.interval = YEAR as i64; // yearly installments, so a tenth of a year later only one is due
        // One installment overdue for a tenth of a year at 100%: 8.3 of late interest on the
        // principal share, i.e. 8 after rounding; 10 would be the figure on the whole part.
        let first_due = t.anchor + YEAR as i64;
        let a = arrears(&t, 0, first_due + (YEAR / 10) as i64).unwrap();
        assert_eq!((a.due, a.overdue, a.overdue_principal), (1, 100, 83));
        assert_eq!(a.late_due, (83u128 * (YEAR / 10) as u128 / YEAR as u128) as u64);
        assert_eq!(a.late_due, 8);
        // A partial payment of 60 leaves 40 overdue, of which 33 is principal.
        let a = arrears(&t, 60, first_due + (YEAR / 10) as i64).unwrap();
        assert_eq!((a.overdue, a.overdue_principal), (40, 33));
        // Far in the future the per-settlement cap is the overdue principal, not the overdue total.
        let a = arrears(&t, 0, t.anchor + 60 * YEAR as i64).unwrap();
        assert_eq!((a.overdue, a.overdue_principal, a.late_due), (600, 498, 498));
    }

    /// The interest kept at the mark is the share of the installments then due; the rest is
    /// forgiven. Rounded down, nothing forgiven when every installment was due, everything but the
    /// due share otherwise.
    #[test]
    fn accelerated_interest_keeps_the_share_of_the_installments_due() {
        assert_eq!(accelerated_interest(720_000, 3, 6), Some((360_000, 360_000)));
        assert_eq!(accelerated_interest(720_000, 6, 6), Some((720_000, 0)));
        assert_eq!(accelerated_interest(720_000, 0, 6), Some((0, 720_000)));
        assert_eq!(accelerated_interest(100, 1, 3), Some((33, 67)), "rounded down, the forgiven part carries the remainder");
        assert_eq!(accelerated_interest(u64::MAX, 119, 120), Some((u64::MAX / 120 * 119 + (u64::MAX % 120) * 119 / 120, u64::MAX - (u64::MAX / 120 * 119 + (u64::MAX % 120) * 119 / 120))));
        assert_eq!(accelerated_interest(100, 7, 6), None);
        assert_eq!(accelerated_interest(100, 0, 0), None);
    }

    /// A bullet line is one installment due at the term (anchor `term_end - 1`, interval 1): not
    /// accelerated, it is due at `term_end` and late interest runs from `term_end + grace`;
    /// accelerated before the term, it runs from the mark.
    #[test]
    fn arrears_of_a_bullet_line_run_from_the_term_plus_grace_or_from_the_mark() {
        let term_end = 100_000_000;
        let grace = 100u32;
        let t = ScheduleTerms {
            total: 4_400_000,
            anchor: term_end - 1,
            interval: 1,
            installments: 1,
            late_bps: 3_650,
            grace,
            late_paid_until: 0,
            principal: 4_000_000,
            principal_left: 4_000_000,
            late_collected: 0,
            accelerated_at: 0,
        };
        let a = arrears(&t, 0, term_end - 1).unwrap();
        assert_eq!((a.due, a.missed, a.overdue, a.late_due), (0, 0, 0, 0), "before the term nothing is due");
        let a = arrears(&t, 0, term_end).unwrap();
        assert_eq!((a.due, a.missed, a.overdue, a.first_missed_due), (1, 1, 4_400_000, Some(term_end)));
        let start = term_end + i64::from(grace);
        assert_eq!(arrears(&t, 0, start).unwrap().late_due, 0, "nothing within the grace");
        let a = arrears(&t, 0, start + YEAR as i64).unwrap();
        assert_eq!((a.overdue_principal, a.late_due), (4_000_000, 1_460_000), "a year at 36.5% on the principal");
        // Accelerated a year before the term: the mark rules, with no grace after it.
        let mut acc = t;
        acc.accelerated_at = term_end - YEAR as i64;
        assert_eq!(arrears(&acc, 0, term_end).unwrap().late_due, 1_460_000);
        // Paid in full: nothing.
        assert_eq!(arrears(&t, 4_400_000, start + YEAR as i64).unwrap().late_due, 0);
    }

    /// Once accelerated, every installment is due, the whole balance is
    /// overdue, and an installment not yet due accrues from the acceleration (no further grace)
    /// while one already overdue keeps its own start.
    #[test]
    fn arrears_after_acceleration_count_every_installment_and_accrue_from_the_mark() {
        // Six installments of 100 due at 1_100, 1_200, ... 1_600; 100% a year; grace 10.
        let mut t = terms(600, 6, 10_000, 10);
        let now = 1_350;
        let plain = arrears(&t, 100, now).unwrap();
        assert_eq!((plain.due, plain.covered, plain.missed, plain.overdue), (3, 1, 2, 200));
        // Accelerated at 1_250, after the second installment's grace, before the third due date.
        t.accelerated_at = 1_250;
        let a = arrears(&t, 100, now).unwrap();
        assert_eq!((a.due, a.covered, a.missed), (6, 1, 5));
        assert_eq!((a.overdue, a.overdue_principal), (500, 500), "the whole balance");
        assert_eq!(a.first_missed_due, Some(1_200), "the oldest unpaid installment, already due");
        // Installment 2 accrues from 1_210 (its own due + grace), 3 to 6 from 1_250 (the mark).
        let weighted: u128 = 100 * (now - 1_210) as u128 + 4 * 100 * (now - 1_250) as u128;
        assert_eq!(a.late_due, (weighted * 10_000 / (10_000 * YEAR as u128)) as u64);
        // Paying the two installments that were overdue before the mark cures nothing.
        let a = arrears(&t, 300, now).unwrap();
        assert_eq!((a.covered, a.missed, a.overdue), (3, 3, 300));
        assert_eq!(a.first_missed_due, Some(1_250), "an installment due after the mark is due at the mark");
        // The watermark still rules the start.
        t.late_paid_until = 1_300;
        let a = arrears(&t, 100, now).unwrap();
        let weighted: u128 = 5 * 100 * (now - 1_300) as u128;
        assert_eq!(a.late_due, (weighted * 10_000 / (10_000 * YEAR as u128)) as u64);
        // Fully paid: nothing is overdue, accelerated or not.
        let a = arrears(&t, 600, now).unwrap();
        assert_eq!((a.covered, a.missed, a.overdue, a.late_due, a.first_missed_due), (6, 0, 0, 0, None));
    }
}
