//! Property tests over `math`, the pure arithmetic the instructions build on. Each property is an
//! invariant the pool's accounting relies on; `proptest` searches the input space for a
//! counter-example and shrinks it when it finds one.

mod reference;

use anchor_lang::prelude::Pubkey;
use elote_pool::math::{
    allocate, arrears, assets_for_redeem, cumulative_due, installment_amount, installments_covered, installments_due,
    late_interest, shares_for_deposit, Allocation, Arrears, ScheduleTerms,
};
use elote_pool::state::{CreditLine, LineStatus, CURRENCY_MXN, MAX_INSTALLMENTS};
use proptest::prelude::*;
use reference::RefLine;

/// A bullet line at `rate_bps` whose ordinary interest stops at `end`, nothing drawn.
fn line(rate_bps: u16, end: i64) -> CreditLine {
    CreditLine {
        pool: Pubkey::default(),
        line_id: 0,
        borrower: Pubkey::default(),
        sponsor: Pubkey::default(),
        sponsor_role: 0,
        agreement_hash: [0; 32],
        expediente_root: [0; 32],
        status: LineStatus::Active,
        principal: u64::MAX,
        rate_bps,
        term_end: end,
        availability_end: end,
        drawn: 0,
        draws: 0,
        principal_seconds: 0,
        accrued_at: 0,
        principal_repaid: 0,
        interest_repaid: 0,
        pagares_paid: 0,
        first_due_at: 0,
        interval_seconds: 0,
        installments: 0,
        late_bps: 0,
        max_missed: 0,
        late_collected: 0,
        late_paid_until: 0,
        drawn_at: 0,
        closed_at: 0,
        accelerated_at: 0,
        bump: 0,
        currency: CURRENCY_MXN,
        requested_at: 0,
        approved_at: 0,
        terms_hash: [0; 32],
        pagare_template_hash: [0; 32],
        payer: Pubkey::default(),
        availability_end_saved: 0,
        acceleration_due_at: 0,
        usdc_drawn: 0,
        usdc_cost_repaid: 0,
        reserved: [0; 64],
    }
}

/// A step of a line's life for the accrual properties: a draw, a payment, or a read alone.
#[derive(Clone, Debug)]
enum Step {
    Draw(u64),
    Pay(u64),
    Look,
}

fn step() -> impl Strategy<Value = Step> {
    prop_oneof![
        (1u64..=1u64 << 40).prop_map(Step::Draw),
        prop_oneof![1u64..=100, 1u64..=1u64 << 41].prop_map(Step::Pay),
        Just(Step::Look),
    ]
}

/// Applies a payment of `amount` at `now` as `repay` does: `allocate` the credit (no late interest
/// here), accrue, then move the counters.
fn pay(l: &mut CreditLine, now: i64, amount: u64) -> Allocation {
    let a = allocate(amount, 0, l.interest_unpaid(now).unwrap(), l.outstanding().unwrap()).unwrap();
    l.accrue(now).unwrap();
    l.principal_repaid += a.principal;
    l.interest_repaid += a.interest;
    a
}

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
    fn installments_due_is_monotone(anchor in 0..1i64 << 40, interval in 1..1i64 << 30, n in 1..=MAX_INSTALLMENTS, a in 0..1i64 << 41, b in 0..1i64 << 41) {
        let (lo, hi) = (a.min(b), a.max(b));
        let d_lo = installments_due(lo, anchor, interval, n);
        let d_hi = installments_due(hi, anchor, interval, n);
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
    /// principal bounded by what is unrepaid, late interest bounded by it and by what is left of the
    /// lifetime cap, and a first missed due date exactly when something is missed.
    #[test]
    fn arrears_are_consistent(
        principal in 1..=1u64 << 50,
        n in 1..=MAX_INSTALLMENTS,
        anchor in 1_000_000_000i64..2_000_000_000,
        interval in 1..=10_000_000i64,
        late_bps in 0..=10_000u16,
        paid_num in 0..=12_000u64,
        collected_share in 0..=10_000u64,
        watermark in 0..=10_000i64,
        now_num in -100..=130i64,
    ) {
        prop_assume!(principal >= u64::from(n));
        let late_collected = principal / 10_000 * collected_share;
        let paid = (principal / 10_000).max(1) * paid_num; // up to 120% of the principal
        let now = anchor + interval * now_num; // from before the first due date to past the last
        let late_paid_until = if watermark == 0 { 0 } else { anchor + (now - anchor) * watermark / 10_000 };
        let t = ScheduleTerms { principal, anchor, interval, installments: n, late_bps, late_paid_until, late_collected, accelerated_at: 0 };
        let Arrears { due, covered, missed, overdue, late_due, first_missed_due } =
            arrears(&t, paid, now).expect("valid terms never overflow");
        prop_assert!(due <= n);
        prop_assert!(covered <= n);
        prop_assert_eq!(missed, due.saturating_sub(covered));
        prop_assert!(overdue <= principal.saturating_sub(paid));
        prop_assert!(late_due <= overdue);
        prop_assert!(late_due <= principal - late_collected);
        prop_assert_eq!(first_missed_due.is_some(), missed > 0);
        prop_assert_eq!(overdue > 0, missed > 0);
        if late_bps == 0 {
            prop_assert_eq!(late_due, 0);
        }
        if let Some(first) = first_missed_due {
            prop_assert!(first > anchor && first <= now, "a missed due date lies between the anchor and now");
        }
    }

    /// The payment order (decision 48): a credit below the late interest is refused; otherwise
    /// late interest, then interest, then principal, each within what is owed, never more than
    /// the credit; principal only once the interest is cleared.
    #[test]
    fn allocate_follows_the_payment_order(credit in any::<u64>(), late in any::<u64>(), interest in any::<u64>(), principal in any::<u64>()) {
        match allocate(credit, late, interest, principal) {
            None => prop_assert!(credit < late),
            Some(a) => {
                prop_assert_eq!(a.late, late);
                prop_assert!(a.interest <= interest && a.principal <= principal);
                prop_assert!(u128::from(a.late) + u128::from(a.interest) + u128::from(a.principal) <= u128::from(credit));
                prop_assert_eq!(u128::from(a.total()), u128::from(a.late) + u128::from(a.interest) + u128::from(a.principal));
                if a.principal > 0 {
                    prop_assert_eq!(a.interest, interest, "principal only once the interest is cleared");
                }
                let rest = credit - late;
                prop_assert_eq!(u128::from(a.interest) + u128::from(a.principal), u128::from(rest).min(u128::from(interest) + u128::from(principal)));
            }
        }
    }

    /// Decision 48 (a, b, h at the pure level): over random draws, payments of any size, reads and
    /// gaps, the program's accrual equals the reference's `rate x ∫ balance dt` rounded once, at
    /// every step and at the end, whatever the number of operations; principal and interest repaid
    /// stay within what was drawn and accrued; and a payment that applies principal leaves no
    /// interest unpaid.
    #[test]
    fn the_accrual_is_the_reference_under_any_sequence(
        rate in 0u16..=10_000,
        span in 1i64..=(3 * 365 * 86_400),
        steps in proptest::collection::vec((step(), 0i64..=40 * 86_400), 1..60),
    ) {
        let t0 = 1_800_000_000i64;
        let end = t0 + span;
        let mut l = line(rate, end);
        let mut r = RefLine::new(rate, end);
        let mut now = t0;
        for (s, gap) in steps {
            now += gap;
            match s {
                Step::Draw(amount) => {
                    l.accrue(now).unwrap();
                    l.drawn += amount;
                    r.draw(now, amount);
                }
                Step::Pay(amount) => {
                    let a = pay(&mut l, now, amount);
                    let (_, i, p) = r.pay(now, amount, 0);
                    prop_assert_eq!((a.interest, a.principal), (i, p));
                    if a.principal > 0 {
                        prop_assert_eq!(l.interest_unpaid(now), Some(0));
                    }
                }
                Step::Look => l.accrue(now).unwrap(),
            }
            prop_assert_eq!(u128::from(l.interest_accrued(now).unwrap()), r.interest_accrued(now));
            prop_assert!(l.principal_repaid <= l.drawn);
            prop_assert!(l.interest_repaid <= l.interest_accrued(now).unwrap());
            prop_assert_eq!(l.owed(now).map(u128::from), Some(r.payoff(now) as u128));
        }
        let later = now + 400 * 86_400;
        prop_assert_eq!(u128::from(l.interest_accrued(later).unwrap()), r.interest_accrued(later));
    }

    /// Decision 48 (c): a payment split in any number of parts in the same second applies what one
    /// payment of the sum applies.
    #[test]
    fn a_payment_split_in_one_second_applies_what_one_applies(
        rate in 1u16..=10_000,
        drawn in 1u64..=1u64 << 40,
        elapsed in 0i64..=2 * 365 * 86_400,
        parts in proptest::collection::vec(1u64..=1u64 << 38, 1..8),
    ) {
        let t0 = 1_800_000_000i64;
        let now = t0 + elapsed;
        let mut one = line(rate, t0 + 3 * 365 * 86_400);
        one.accrue(t0).unwrap();
        one.drawn = drawn;
        let mut split = one.clone();
        let total: u64 = parts.iter().sum();
        let whole = pay(&mut one, now, total);
        let (mut interest, mut principal) = (0, 0);
        for part in parts {
            let a = pay(&mut split, now, part);
            interest += a.interest;
            principal += a.principal;
        }
        prop_assert_eq!((interest, principal), (whole.interest, whole.principal));
        prop_assert_eq!((split.principal_repaid, split.interest_repaid), (one.principal_repaid, one.interest_repaid));
        prop_assert_eq!(split.interest_accrued(now + 86_400), one.interest_accrued(now + 86_400));
    }
}
