//! An independent reference for ordinary interest (decision 48), written without the program: a
//! line is a list of `(time, principal change)` events, and the interest accrued at `t` is
//! `rate_bps * Σ balance * Δt / (10_000 * 365 days)` over the segments before `min(t, end)`,
//! rounded down once. `end` is the last due date, or the acceleration if earlier. Payments split
//! here, independently of the program: late interest (given by the caller), then the ordinary
//! interest accrued and unpaid, then principal, never more than is owed.
#![allow(dead_code)]

const BPS: u128 = 10_000;
const YEAR: u128 = 365 * 86_400;

#[derive(Clone, Debug)]
pub struct RefLine {
    pub rate_bps: u16,
    /// Ordinary interest stops here.
    pub end: i64,
    /// `(time, principal change)`: a draw adds, a principal repayment subtracts.
    pub events: Vec<(i64, i128)>,
    pub drawn: u128,
    pub principal_repaid: u128,
    pub interest_paid: u128,
}

impl RefLine {
    pub fn new(rate_bps: u16, end: i64) -> Self {
        RefLine { rate_bps, end, events: Vec::new(), drawn: 0, principal_repaid: 0, interest_paid: 0 }
    }

    pub fn draw(&mut self, at: i64, amount: u64) {
        self.events.push((at, amount as i128));
        self.drawn += amount as u128;
    }

    pub fn balance(&self) -> u128 {
        self.drawn - self.principal_repaid
    }

    /// `Σ balance * seconds` over `[first event, min(at, end)]`, segment by segment.
    pub fn balance_seconds(&self, at: i64) -> u128 {
        let stop = at.min(self.end);
        let mut events = self.events.clone();
        events.sort_by_key(|e| e.0);
        let mut area: u128 = 0;
        let mut balance: i128 = 0;
        for (i, &(t, delta)) in events.iter().enumerate() {
            balance += delta;
            assert!(balance >= 0, "the reference never repays more than was drawn");
            let next = events.get(i + 1).map_or(i64::MAX, |e| e.0);
            let from = t.min(stop);
            let to = next.min(stop);
            if to > from {
                area += balance as u128 * (to - from) as u128;
            }
        }
        area
    }

    /// The ordinary interest accrued by `at`, rounded down once.
    pub fn interest_accrued(&self, at: i64) -> u128 {
        self.rate_bps as u128 * self.balance_seconds(at) / (BPS * YEAR)
    }

    pub fn interest_unpaid(&self, at: i64) -> u128 {
        self.interest_accrued(at) - self.interest_paid
    }

    /// What a payment of `amount` at `at` settles once `late` (the late interest due, which the
    /// caller states) is taken: `(late, interest, principal)`.
    pub fn pay(&mut self, at: i64, amount: u64, late: u64) -> (u64, u64, u64) {
        let amount = amount as u128;
        let late = late as u128;
        assert!(amount >= late, "a payment covers the late interest");
        let rest = amount - late;
        let interest = rest.min(self.interest_unpaid(at));
        let principal = (rest - interest).min(self.balance());
        self.interest_paid += interest;
        if principal > 0 {
            self.events.push((at, -(principal as i128)));
            self.principal_repaid += principal;
        }
        (late as u64, interest as u64, principal as u64)
    }

    /// What a payment would have to bring at `at` to owe nothing more, late interest aside.
    pub fn payoff(&self, at: i64) -> u64 {
        (self.balance() + self.interest_unpaid(at)) as u64
    }
}

#[test]
fn the_reference_on_hand_numbers() {
    // 60,000.00 at 12% for a twelfth of a year: 600.00.
    let month = (YEAR / 12) as i64;
    let mut r = RefLine::new(1_200, 6 * month);
    r.draw(0, 6_000_000);
    assert_eq!(r.interest_accrued(month), 60_000);
    assert_eq!(r.interest_accrued(6 * month), 360_000);
    assert_eq!(r.interest_accrued(10 * month), 360_000, "nothing after the end");
    assert_eq!(r.pay(month, 1_060_000, 0), (0, 60_000, 1_000_000));
    assert_eq!(r.interest_accrued(2 * month), 60_000 + 50_000);
    // One centavo a second on 262,800,000 at 12%: exactly one centavo of interest a second.
    let mut r = RefLine::new(1_200, 10 * month);
    r.draw(0, 262_800_000);
    assert_eq!(r.interest_accrued(3_600), 3_600);
}
